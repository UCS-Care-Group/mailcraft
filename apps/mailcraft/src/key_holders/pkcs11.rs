// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! PKCS#11 tokens on Linux and FreeBSD (PKCS#11 v2.40 / v3.0), through `cryptoki`'s safe API:
//! smartcards and USB keys through OpenSC, and whatever p11-kit's proxy gathers (the module
//! every desktop registers its tokens with).
//!
//! Loading a module runs its code in this process, so the module is named by the owner alone
//! (the settings, or `MAILCRAFT_PKCS11_MODULE` in the environment the owner started the
//! program with), by an absolute path, never by a command's params; with nothing named, p11-kit's
//! proxy is looked for in its usual places, each tried until one loads ([`open_default`]).
//! A module is loaded and initialised once for the life of the program and never let go of:
//! `cryptoki` doesn't call `C_Finalize` when it is dropped, and unloading a module whose
//! threads still run would crash the program outside any guard ([`Pkcs11Holder::open`]).
//!
//! A certificate's private key is the token's private-key object with the same `CKA_ID`
//! (PKCS#11 v2.40 §4.6.3) whose public half is the certificate's: the RSA modulus or the EC
//! point, read from the key or else from the public-key object with that id. A key that doesn't
//! match is never used. A key paired through a public-key object (which may be a stale one of
//! another key) has each signature checked against the certificate before it is given back
//! ([`signed_by`], with RustCrypto's verifiers); what it unwraps can't be checked so, and a wrong key's unwrapping fails when
//! the message is decrypted with it.
//!
//! Logging in: a token with a protected authentication path (a PIN pad) is logged in to with no
//! PIN and asks on the reader; it never has the window ask for one. Any other token that needs a
//! login ends the call with [`KeyHolderError::PinNeeded`], naming the token by slot, serial
//! number and label; the window asks the owner and calls again with that [`TokenRef`] and the
//! [`Pin`], which then goes to that slot only, and only while the slot, read after a session
//! was opened on it, shows the token with that serial number (a token without one: that label) and the certificate
//! ([`KeyHolderError::TokenChanged`] otherwise). A key marked `CKA_ALWAYS_AUTHENTICATE` (a PIV
//! card's signing key) needs a second, context-specific login between `C_SignInit` and
//! `C_Sign`, which `cryptoki`'s single-part `sign` and `decrypt` don't leave room for: such a key
//! is refused as unsupported for now (`docs/gaps.md`).
//!
//! The logic is written against [`Module`] and [`TokenSession`], which `cryptoki` implements
//! here and a fake token implements in the tests.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error, RvError};
use cryptoki::mechanism::rsa::{PkcsMgfType, PkcsOaepParams, PkcsOaepSource, PkcsPssParams};
use cryptoki::mechanism::{Mechanism, MechanismType};
use cryptoki::object::{Attribute, AttributeType, CertificateType, KeyType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;
use x509_cert::der::Decode;
use x509_cert::der::asn1::OctetString;

use super::certificate::{HeldCertificate, clean_name, sha256, without_leading_zeros};
use super::encoding::{digest_info, ecdsa_raw_to_der};
use super::{
    DigestAlgorithm, KeyHolderError, KeyKind, MAX_CERTIFICATE_LEN, MAX_CERTIFICATES, MAX_SCANNED, Pin, RsaPadding, SignatureScheme, TokenRef,
    UnwrappedKey, check_for_decrypting, check_for_signing, check_wrapped, now,
};

/// The environment variable that names a module, for an owner who starts the program with it.
pub const MODULE_VARIABLE: &str = "MAILCRAFT_PKCS11_MODULE";

/// Where p11-kit's proxy module usually is, the 64-bit and multiarch places first (on a system
/// with both, `/usr/lib` may hold the 32-bit one): Fedora and openSUSE, Debian and Ubuntu on
/// each architecture, Arch, then FreeBSD's ports.
pub const PROXY_PLACES: &[&str] = &[
    "/usr/lib64/p11-kit-proxy.so",
    "/usr/lib/x86_64-linux-gnu/p11-kit-proxy.so",
    "/usr/lib/aarch64-linux-gnu/p11-kit-proxy.so",
    "/usr/lib/riscv64-linux-gnu/p11-kit-proxy.so",
    "/usr/lib/powerpc64le-linux-gnu/p11-kit-proxy.so",
    "/usr/lib/s390x-linux-gnu/p11-kit-proxy.so",
    "/usr/lib/arm-linux-gnueabihf/p11-kit-proxy.so",
    "/usr/lib/i386-linux-gnu/p11-kit-proxy.so",
    "/usr/lib/p11-kit-proxy.so",
    "/usr/local/lib/p11-kit-proxy.so",
];

/// The most slots looked through.
const MAX_SLOTS: usize = 64;
/// The most key objects with one `CKA_ID` looked at.
const MAX_KEYS_PER_ID: usize = 16;
/// The most module files tried in one run (each that loads is kept for good): the proxy's
/// places and a few the owner names.
const MAX_MODULES: usize = 16;
/// The longest token label or serial number kept, in characters.
const MAX_TOKEN_TEXT: usize = 64;

/// The module the owner named in the environment, else the first of p11-kit's proxy's places
/// that loads.
pub fn open_default() -> Result<Arc<Pkcs11Holder>, KeyHolderError> {
    choose_module(std::env::var_os(MODULE_VARIABLE), Path::is_file, Pkcs11Holder::open)
}

/// The module `named` (the environment's; empty is as unset) and that one alone: one that
/// fails, or isn't an absolute path, ends the search. With none named, the first of
/// [`PROXY_PLACES`] that `exists` and `open` opens.
pub fn choose_module<T, E, F>(named: Option<OsString>, exists: E, open: F) -> Result<T, KeyHolderError>
where
    E: Fn(&Path) -> bool,
    F: FnMut(&Path) -> Result<T, KeyHolderError>,
{
    let mut open = open;
    if let Some(named) = named.filter(|n| !n.is_empty()) {
        let named = PathBuf::from(named);
        if !named.is_absolute() {
            return Err(KeyHolderError::Unavailable(format!("{MODULE_VARIABLE} is not an absolute path")));
        }
        return open(&named);
    }
    first_that_opens(PROXY_PLACES.iter().map(PathBuf::from).filter(|p| exists(p)), open)
}

/// The first of `candidates` that `open` opens, else the last refusal.
pub fn first_that_opens<T, I, F>(candidates: I, mut open: F) -> Result<T, KeyHolderError>
where
    I: IntoIterator<Item = PathBuf>,
    F: FnMut(&Path) -> Result<T, KeyHolderError>,
{
    let mut last = KeyHolderError::Unavailable("no PKCS#11 module was found".into());
    for candidate in candidates {
        match open(&candidate) {
            Ok(opened) => return Ok(opened),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// One module file's place in a [`Registry`]: empty while it loads, then kept.
type Place<T> = Arc<Mutex<Option<Arc<T>>>>;
/// Each module file loaded or loading: a loaded one is never let go of, so that no module is
/// unloaded; one that fails to load gives its place back.
pub type Registry<T> = Mutex<Vec<(PathBuf, Place<T>)>>;

fn modules() -> &'static Registry<Pkcs11Holder> {
    static MODULES: OnceLock<Registry<Pkcs11Holder>> = OnceLock::new();
    MODULES.get_or_init(|| Mutex::new(Vec::new()))
}

/// How a module file is known in the registry: its canonical path (links and `..` resolved), or
/// the path as given when it can't be resolved.
pub fn module_key(module: &Path) -> PathBuf {
    std::fs::canonicalize(module).unwrap_or_else(|_| module.to_path_buf())
}

/// The one `T` kept in `registry` for the file `module`, made by `make` (given the file's
/// [`module_key`]) the first time; at most `max` modules loaded or loading. A failure is not
/// kept and gives its place back, so a later call tries again. The registry's lock is held only
/// to find or free the file's place: a module that hangs while loading holds up the calls for
/// that file alone.
pub fn shared<T, F>(registry: &Registry<T>, module: &Path, max: usize, make: F) -> Result<Arc<T>, KeyHolderError>
where
    F: FnOnce(&Path) -> Result<T, KeyHolderError>,
{
    let key = module_key(module);
    let registered = |place: &Place<T>| registry.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|(_, p)| Arc::ptr_eq(p, place));
    // A place a failed load gave back while this call waited for it is looked for again.
    for _ in 0..4 {
        let place: Place<T> = {
            let mut kept = registry.lock().unwrap_or_else(PoisonError::into_inner);
            match kept.iter().find(|(path, _)| *path == key) {
                Some((_, place)) => Arc::clone(place),
                None if kept.len() >= max => return Err(KeyHolderError::Unavailable("too many PKCS#11 modules".into())),
                None => {
                    let place = Place::default();
                    kept.push((key.clone(), Arc::clone(&place)));
                    place
                }
            }
        };
        // Lock order: a place, then the registry; never the other way round.
        let mut held = place.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = held.as_ref() {
            return Ok(Arc::clone(found));
        }
        if !registered(&place) {
            continue;
        }
        return match make(&key) {
            Ok(made) => {
                let made = Arc::new(made);
                *held = Some(Arc::clone(&made));
                Ok(made)
            }
            Err(e) => {
                registry.lock().unwrap_or_else(PoisonError::into_inner).retain(|(_, p)| !Arc::ptr_eq(p, &place));
                Err(e)
            }
        };
    }
    Err(KeyHolderError::Unavailable("the PKCS#11 module is being loaded".into()))
}

/// What the logic here needs of a PKCS#11 module: its slots with a token, and a read-only
/// session on one.
pub trait Module {
    /// A session on one token.
    type Session: TokenSession;
    /// The slots with a token, at most [`MAX_SLOTS`].
    fn slots(&self) -> Result<Vec<u64>, KeyHolderError>;
    /// A read-only session on the token in `slot`.
    fn open(&self, slot: u64) -> Result<Self::Session, KeyHolderError>;
}

/// A read-only session on the token in one slot.
pub trait TokenSession {
    /// A handle on a key object.
    type Key: Copy;
    /// The token in the session's slot (`C_GetTokenInfo` on the slot, read after the session was
    /// opened: what guards a swap is the session, whose every later call fails once its token is
    /// taken out).
    fn facts(&self) -> Result<TokenFacts, KeyHolderError>;
    /// The X.509 certificates on the token with their `CKA_ID`, at most [`MAX_SCANNED`].
    fn certificates(&self) -> Vec<(Vec<u8>, Vec<u8>)>;
    /// The key objects of `class` with this `CKA_ID`, at most [`MAX_KEYS_PER_ID`].
    fn keys(&self, class: ObjectClass, id: &[u8]) -> Vec<Self::Key>;
    /// What the token says of a key object's public half.
    fn key_facts(&self, key: Self::Key) -> KeyFacts;
    /// Whether the key needs a login at every use; a token that can't say is taken at its word
    /// that it doesn't.
    fn always_authenticate(&self, key: Self::Key) -> bool;
    /// `C_Login` as the user: with the owner's PIN, or none for a PIN pad.
    fn login(&self, pin: Option<&Pin>) -> Result<(), Error>;
    /// `C_SignInit` and `C_Sign`.
    fn sign(&self, mechanism: &Mechanism<'_>, key: Self::Key, input: &[u8]) -> Result<Vec<u8>, Error>;
    /// `C_DecryptInit` and `C_Decrypt`.
    fn decrypt(&self, mechanism: &Mechanism<'_>, key: Self::Key, input: &[u8]) -> Result<Vec<u8>, Error>;
}

/// A token: how it is named, whether it has a PIN pad, whether it needs a login.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenFacts {
    pub token: TokenRef,
    pub protected_path: bool,
    pub login_required: bool,
}

/// A certificate on a token, with what it takes to use its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenCertificate {
    /// The certificate.
    pub certificate: HeldCertificate,
    /// The token, as the window names it when asking for its PIN.
    pub token: TokenRef,
    /// Whether the token asks for its PIN on the reader (a PIN pad).
    pub protected_path: bool,
    /// Whether the token needs a login before its key can be used.
    pub login_required: bool,
}

/// The certificates on `module`'s tokens that have a private key and may be used for mail,
/// expired ones included (marked).
pub fn list<M: Module>(module: &M, now: Option<u64>) -> Result<Vec<TokenCertificate>, KeyHolderError> {
    let mut out: Vec<TokenCertificate> = Vec::new();
    for slot in module.slots()? {
        let Ok(session) = module.open(slot) else { continue };
        let Ok(facts) = session.facts() else { continue };
        for (der, id) in session.certificates() {
            for certificate in super::usable([der], now) {
                // A certificate without an id has no key to pair with: never listed, so no PIN
                // is asked for it. Without a login a token may hide its private keys: a
                // certificate with an id is listed then, and its key is looked for when it is
                // used; a token that shows them lists only a certificate whose key it holds.
                if id.is_empty() || (!facts.login_required && find_key(&session, &id, &certificate).is_none()) {
                    continue;
                }
                if out.len() < MAX_CERTIFICATES && !out.iter().any(|c| c.certificate.fingerprint == certificate.fingerprint) {
                    out.push(TokenCertificate {
                        certificate,
                        token: facts.token.clone(),
                        protected_path: facts.protected_path,
                        login_required: facts.login_required,
                    });
                }
            }
        }
    }
    Ok(out)
}

/// Signs `digest` with the key of the certificate whose SHA-256 is `fingerprint`; `pin` is the
/// PIN the owner gave for the token a [`KeyHolderError::PinNeeded`] named. The result is the
/// raw RSA signature, or DER `Ecdsa-Sig-Value` for ECDSA.
pub fn sign_digest<M: Module>(
    module: &M,
    fingerprint: &[u8; 32],
    scheme: SignatureScheme,
    algorithm: DigestAlgorithm,
    digest: &[u8],
    pin: Option<(&TokenRef, &Pin)>,
) -> Result<Vec<u8>, KeyHolderError> {
    let digest = algorithm.check(digest)?;
    let found = key_for(module, fingerprint, pin, |certificate| check_for_signing(certificate, scheme))?;
    let (mechanism, input) = match scheme {
        SignatureScheme::RsaPkcs1v15 => (Mechanism::RsaPkcs, digest_info(algorithm, digest)?),
        SignatureScheme::RsaPss => {
            let (hash_alg, mgf) = hash_and_mgf(algorithm);
            let s_len = u64::try_from(algorithm.digest_len()).map_err(|_| KeyHolderError::BadInput("digest length"))?;
            (Mechanism::RsaPkcsPss(PkcsPssParams { hash_alg, mgf, s_len: s_len.into() }), digest.to_vec())
        }
        SignatureScheme::Ecdsa => (Mechanism::Ecdsa, digest.to_vec()),
    };
    let signature = found.session.sign(&mechanism, found.key, &input).map_err(|e| token_error(e, &found.facts, found.logged_in))?;
    if found.paired_by_public_object && !signed_by(&found.certificate, scheme, algorithm, digest, &signature) {
        return Err(KeyHolderError::NotAllowed("the token signed with a key that isn't the certificate's"));
    }
    match scheme {
        SignatureScheme::Ecdsa => ecdsa_raw_to_der(&signature, found.certificate.key),
        _ => Ok(signature),
    }
}

/// Unwraps an RSA-encrypted content-encryption key with the key of the certificate whose
/// SHA-256 is `fingerprint` (an expired certificate's too); `pin` as for signing.
pub fn decrypt_key<M: Module>(
    module: &M,
    fingerprint: &[u8; 32],
    padding: RsaPadding,
    wrapped: &[u8],
    pin: Option<(&TokenRef, &Pin)>,
) -> Result<UnwrappedKey, KeyHolderError> {
    let wrapped = check_wrapped(wrapped)?;
    let found = key_for(module, fingerprint, pin, check_for_decrypting)?;
    let mechanism = match padding {
        RsaPadding::Pkcs1v15 => Mechanism::RsaPkcs,
        RsaPadding::OaepSha1 => Mechanism::RsaPkcsOaep(PkcsOaepParams::new(MechanismType::SHA1, PkcsMgfType::MGF1_SHA1, PkcsOaepSource::empty())),
        RsaPadding::OaepSha256 => {
            Mechanism::RsaPkcsOaep(PkcsOaepParams::new(MechanismType::SHA256, PkcsMgfType::MGF1_SHA256, PkcsOaepSource::empty()))
        }
    };
    found.session.decrypt(&mechanism, found.key, wrapped).map(UnwrappedKey::new).map_err(|e| token_error(e, &found.facts, found.logged_in))
}

struct FoundKey<S: TokenSession> {
    session: S,
    key: S::Key,
    certificate: HeldCertificate,
    facts: TokenFacts,
    /// Whether the session is known to be logged in, by a login in this call.
    logged_in: bool,
    /// Whether the key was matched to the certificate through its public-key object, so that
    /// what it signs is checked against the certificate.
    paired_by_public_object: bool,
}

/// The session, token and private key for the certificate `fingerprint`, after `check` allowed
/// the certificate for this use, logged in. With a PIN, only the slot it was asked for is
/// looked at, and only while it still holds that token.
fn key_for<M: Module>(
    module: &M,
    fingerprint: &[u8; 32],
    pin: Option<(&TokenRef, &Pin)>,
    check: impl Fn(&HeldCertificate) -> Result<(), KeyHolderError>,
) -> Result<FoundKey<M::Session>, KeyHolderError> {
    let named = pin.map(|(token, _)| token);
    let not_there = || if named.is_some() { KeyHolderError::TokenChanged } else { KeyHolderError::NotFound };
    for slot in module.slots()? {
        if named.is_some_and(|n| n.slot != slot) {
            continue;
        }
        // The token's facts are read for the slot (`C_GetTokenInfo`), after the session is
        // opened; the session is what guards a swap. A token taken out after this point closes
        // its sessions, so the certificate search, the login and the use that follow on this
        // session fail (`TokenChanged`) and the PIN never reaches another token; a token put in
        // between the opening and the reading shows a serial number, label or certificates that
        // aren't the ones the PIN was asked for.
        let Ok(session) = module.open(slot) else { continue };
        let Ok(facts) = session.facts() else { continue };
        let Some((der, id)) = session.certificates().into_iter().find(|(der, _)| sha256(der) == *fingerprint) else { continue };
        // A PIN goes only to the token it was asked for.
        check_same_token(named, &facts.token)?;
        let certificate = HeldCertificate::read(der, now())?;
        check(&certificate)?;
        let mut logged_in = false;
        if facts.login_required || pin.is_some() {
            let result = if facts.protected_path {
                session.login(None)
            } else {
                let Some((_, pin)) = pin else { return Err(KeyHolderError::PinNeeded(facts.token)) };
                session.login(Some(pin))
            };
            match result {
                Ok(()) | Err(Error::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => logged_in = true,
                Err(e) => return Err(token_error(e, &facts, false)),
            }
        }
        let (key, paired_by_public_object) = find_key(&session, &id, &certificate).ok_or(KeyHolderError::NotFound)?;
        if session.always_authenticate(key) {
            return Err(KeyHolderError::Unsupported("a key that asks for its PIN at every use"));
        }
        return Ok(FoundKey { session, key, certificate, facts, logged_in, paired_by_public_object });
    }
    Err(not_there())
}

/// A PIN given for `named` goes only to the token that is still that one: the same slot and
/// serial number; two tokens without a serial number are told apart by their labels (and the
/// caller has found the certificate on it).
pub fn check_same_token(named: Option<&TokenRef>, found: &TokenRef) -> Result<(), KeyHolderError> {
    match named {
        Some(n) if n.slot != found.slot || n.serial != found.serial || (n.serial.is_empty() && n.label != found.label) => {
            Err(KeyHolderError::TokenChanged)
        }
        _ => Ok(()),
    }
}

fn token_text(text: &str) -> String {
    clean_name(text.trim()).chars().take(MAX_TOKEN_TEXT).collect()
}

/// What a token says of a key's public half.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyFacts {
    /// `CKK_RSA`, `CKK_EC` or something else.
    pub kind: Option<KeyKindOnToken>,
    /// `CKA_MODULUS` of an RSA key.
    pub modulus: Option<Vec<u8>>,
    /// `CKA_EC_POINT` of an EC key (DER `OCTET STRING`, or the bare point some tokens give).
    pub ec_point: Option<Vec<u8>>,
}

/// The kinds of key a token says it holds that we use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKindOnToken {
    Rsa,
    Ec,
}

/// Whether the key the token describes is the certificate's.
pub fn key_matches(certificate: &HeldCertificate, facts: &KeyFacts) -> bool {
    match certificate.key {
        KeyKind::Rsa { .. } => {
            facts.kind == Some(KeyKindOnToken::Rsa)
                && facts
                    .modulus
                    .as_deref()
                    .is_some_and(|m| !certificate.public_key.is_empty() && without_leading_zeros(m) == certificate.public_key.as_slice())
        }
        KeyKind::EcP256 | KeyKind::EcP384 => {
            facts.kind == Some(KeyKindOnToken::Ec)
                && facts.ec_point.as_deref().is_some_and(|p| !certificate.public_key.is_empty() && same_point(p, &certificate.public_key))
        }
        KeyKind::Other => false,
    }
}

/// `CKA_EC_POINT` as DER `OCTET STRING` (PKCS#11 v2.40 §2.3.3) or bare, against the bare point.
fn same_point(on_token: &[u8], point: &[u8]) -> bool {
    on_token == point || OctetString::from_der(on_token).is_ok_and(|inner| inner.as_bytes() == point)
}

/// The private key with this `CKA_ID` whose public half is the certificate's, and whether that
/// public half came from the public-key object (see [`public_half`]).
fn find_key<S: TokenSession>(session: &S, id: &[u8], certificate: &HeldCertificate) -> Option<(S::Key, bool)> {
    if id.is_empty() {
        return None;
    }
    session.keys(ObjectClass::PRIVATE_KEY, id).into_iter().take(MAX_KEYS_PER_ID).find_map(|key| {
        let (facts, from_public_object) = public_half(session, key, id);
        key_matches(certificate, &facts).then_some((key, from_public_object))
    })
}

/// What the token says of `key`'s public half, and whether its modulus or point came from the
/// public-key object. What the private key doesn't say (its type, its modulus or point) is taken
/// from the public-key object with the same id, when there is exactly one; a type still unsaid
/// is the one its modulus or point shows.
fn public_half<S: TokenSession>(session: &S, key: S::Key, id: &[u8]) -> (KeyFacts, bool) {
    let mut facts = session.key_facts(key);
    let mut from_public_object = false;
    if (facts.kind.is_none() || (facts.modulus.is_none() && facts.ec_point.is_none()))
        && let [public] = session.keys(ObjectClass::PUBLIC_KEY, id).as_slice()
    {
        let public = session.key_facts(*public);
        from_public_object = (facts.modulus.is_none() && public.modulus.is_some()) || (facts.ec_point.is_none() && public.ec_point.is_some());
        facts.kind = facts.kind.or(public.kind);
        facts.modulus = facts.modulus.or(public.modulus);
        facts.ec_point = facts.ec_point.or(public.ec_point);
    }
    if facts.kind.is_none() {
        facts.kind = match (&facts.modulus, &facts.ec_point) {
            (Some(_), None) => Some(KeyKindOnToken::Rsa),
            (None, Some(_)) => Some(KeyKindOnToken::Ec),
            _ => None,
        };
    }
    (facts, from_public_object)
}

/// Whether `signature` (raw RSA, or ECDSA's `r || s`) is the certificate's key's over `digest`,
/// checked with RustCrypto's verifiers (public keys only).
pub fn signed_by(certificate: &HeldCertificate, scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8], signature: &[u8]) -> bool {
    use rsa::signature::hazmat::PrehashVerifier;
    match (certificate.key, scheme) {
        (KeyKind::Rsa { .. }, SignatureScheme::RsaPkcs1v15 | SignatureScheme::RsaPss) => {
            let Some(key) = rsa_public_key(&certificate.der) else { return false };
            if scheme == SignatureScheme::RsaPkcs1v15 {
                let Ok(signature) = rsa::pkcs1v15::Signature::try_from(signature) else { return false };
                match algorithm {
                    DigestAlgorithm::Sha256 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new(key).verify_prehash(digest, &signature),
                    DigestAlgorithm::Sha384 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha384>::new(key).verify_prehash(digest, &signature),
                    DigestAlgorithm::Sha512 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha512>::new(key).verify_prehash(digest, &signature),
                }
                .is_ok()
            } else {
                // The salt as long as the hash, as tokens are asked to sign here.
                let Ok(signature) = rsa::pss::Signature::try_from(signature) else { return false };
                match algorithm {
                    DigestAlgorithm::Sha256 => rsa::pss::VerifyingKey::<sha2::Sha256>::new(key).verify_prehash(digest, &signature),
                    DigestAlgorithm::Sha384 => rsa::pss::VerifyingKey::<sha2::Sha384>::new(key).verify_prehash(digest, &signature),
                    DigestAlgorithm::Sha512 => rsa::pss::VerifyingKey::<sha2::Sha512>::new(key).verify_prehash(digest, &signature),
                }
                .is_ok()
            }
        }
        (KeyKind::EcP256, SignatureScheme::Ecdsa) => {
            let (Ok(key), Ok(signature)) =
                (p256::ecdsa::VerifyingKey::from_sec1_bytes(&certificate.public_key), p256::ecdsa::Signature::from_slice(signature))
            else {
                return false;
            };
            key.verify_prehash(digest, &signature).is_ok()
        }
        (KeyKind::EcP384, SignatureScheme::Ecdsa) => {
            let (Ok(key), Ok(signature)) =
                (p384::ecdsa::VerifyingKey::from_sec1_bytes(&certificate.public_key), p384::ecdsa::Signature::from_slice(signature))
            else {
                return false;
            };
            key.verify_prehash(digest, &signature).is_ok()
        }
        _ => false,
    }
}

/// The RSA public key of the certificate `der`.
fn rsa_public_key(der: &[u8]) -> Option<rsa::PublicKey> {
    use rsa::pkcs1::DecodeRsaPublicKey;
    let cert = x509_cert::Certificate::from_der(der).ok()?;
    rsa::PublicKey::from_pkcs1_der(cert.tbs_certificate().subject_public_key_info().subject_public_key.raw_bytes()).ok()
}

fn hash_and_mgf(algorithm: DigestAlgorithm) -> (MechanismType, PkcsMgfType) {
    match algorithm {
        DigestAlgorithm::Sha256 => (MechanismType::SHA256, PkcsMgfType::MGF1_SHA256),
        DigestAlgorithm::Sha384 => (MechanismType::SHA384, PkcsMgfType::MGF1_SHA384),
        DigestAlgorithm::Sha512 => (MechanismType::SHA512, PkcsMgfType::MGF1_SHA512),
    }
}

/// A token's error for the token `facts` describes; `logged_in` says whether this call logged
/// the session in. "Not logged in" after a login is a key that wants one at every use, and on a
/// PIN pad a login the token didn't say it needed: neither asks the window for a PIN, so a call
/// never asks twice. Its text never carries the PIN (`cryptoki` is never given it in a form it
/// prints).
pub fn token_error(error: Error, facts: &TokenFacts, logged_in: bool) -> KeyHolderError {
    match error {
        Error::Pkcs11(RvError::PinIncorrect | RvError::PinInvalid | RvError::PinLenRange, _) => KeyHolderError::PinIncorrect,
        Error::Pkcs11(RvError::PinLocked, _) => KeyHolderError::PinLocked,
        Error::Pkcs11(RvError::PinExpired, _) => KeyHolderError::PinExpired,
        Error::Pkcs11(RvError::FunctionCanceled, _) => KeyHolderError::Cancelled,
        Error::Pkcs11(RvError::TokenNotPresent | RvError::DeviceRemoved | RvError::SessionHandleInvalid | RvError::SessionClosed, _) => {
            KeyHolderError::TokenChanged
        }
        Error::Pkcs11(RvError::UserNotLoggedIn, _) if logged_in => KeyHolderError::Unsupported("a key that asks for its PIN at every use"),
        Error::Pkcs11(RvError::UserNotLoggedIn, _) if facts.protected_path => {
            KeyHolderError::Unsupported("a PIN-pad token that wants a login it didn't say it needed")
        }
        Error::Pkcs11(RvError::UserNotLoggedIn, _) => KeyHolderError::PinNeeded(facts.token.clone()),
        Error::Pkcs11(
            RvError::MechanismInvalid | RvError::MechanismParamInvalid | RvError::KeyTypeInconsistent | RvError::KeyFunctionNotPermitted,
            _,
        ) => KeyHolderError::Unsupported("the token doesn't do that with this key"),
        other => KeyHolderError::System(other.to_string()),
    }
}

/// A loaded PKCS#11 module, kept for the life of the program.
pub struct Pkcs11Holder {
    context: Pkcs11,
}

impl Pkcs11Holder {
    /// The module at `module`, an absolute path the owner chose: loaded and initialised the
    /// first time, the same one after (the same file by another path too).
    pub fn open(module: &Path) -> Result<Arc<Self>, KeyHolderError> {
        if !module.is_absolute() {
            return Err(KeyHolderError::Unavailable("the PKCS#11 module is not named by an absolute path".into()));
        }
        shared(modules(), module, MAX_MODULES, |file| {
            let context = Pkcs11::new(file).map_err(|e| KeyHolderError::Unavailable(e.to_string()))?;
            match context.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
                Ok(()) | Err(Error::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => Ok(Self { context }),
                Err(e) => Err(KeyHolderError::Unavailable(e.to_string())),
            }
        })
    }

    /// [`list`] on this module's tokens.
    pub fn certificates(&self) -> Result<Vec<TokenCertificate>, KeyHolderError> {
        list(self, now())
    }

    /// [`sign_digest`] with this module's tokens.
    pub fn sign_digest(
        &self,
        fingerprint: &[u8; 32],
        scheme: SignatureScheme,
        algorithm: DigestAlgorithm,
        digest: &[u8],
        pin: Option<(&TokenRef, &Pin)>,
    ) -> Result<Vec<u8>, KeyHolderError> {
        sign_digest(self, fingerprint, scheme, algorithm, digest, pin)
    }

    /// [`decrypt_key`] with this module's tokens.
    pub fn decrypt_key(
        &self,
        fingerprint: &[u8; 32],
        padding: RsaPadding,
        wrapped: &[u8],
        pin: Option<(&TokenRef, &Pin)>,
    ) -> Result<UnwrappedKey, KeyHolderError> {
        decrypt_key(self, fingerprint, padding, wrapped, pin)
    }
}

impl Module for Pkcs11Holder {
    type Session = Pkcs11Session;

    fn slots(&self) -> Result<Vec<u64>, KeyHolderError> {
        let slots = self.context.get_slots_with_token().map_err(|e| KeyHolderError::Unavailable(e.to_string()))?;
        Ok(slots.iter().take(MAX_SLOTS).map(Slot::id).collect())
    }

    fn open(&self, slot: u64) -> Result<Pkcs11Session, KeyHolderError> {
        let slot = Slot::try_from(slot).map_err(|e| KeyHolderError::System(e.to_string()))?;
        let session = self.context.open_ro_session(slot).map_err(|e| KeyHolderError::System(e.to_string()))?;
        Ok(Pkcs11Session { context: self.context.clone(), slot, session })
    }
}

/// A read-only `cryptoki` session on one slot's token.
pub struct Pkcs11Session {
    context: Pkcs11,
    slot: Slot,
    session: Session,
}

impl TokenSession for Pkcs11Session {
    type Key = ObjectHandle;

    fn facts(&self) -> Result<TokenFacts, KeyHolderError> {
        let info = self.context.get_token_info(self.slot).map_err(|e| KeyHolderError::System(e.to_string()))?;
        Ok(TokenFacts {
            token: TokenRef { slot: self.slot.id(), serial: token_text(info.serial_number()), label: token_text(info.label()) },
            protected_path: info.protected_authentication_path(),
            login_required: info.login_required(),
        })
    }

    fn certificates(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        let template = [Attribute::Class(ObjectClass::CERTIFICATE), Attribute::CertificateType(CertificateType::X_509)];
        let Ok(handles) = self.session.find_objects(&template) else { return Vec::new() };
        let mut out = Vec::new();
        for handle in handles.into_iter().take(MAX_SCANNED) {
            let Ok(attributes) = self.session.get_attributes(handle, &[AttributeType::Value, AttributeType::Id]) else { continue };
            let mut der = None;
            let mut id = None;
            for attribute in attributes {
                match attribute {
                    Attribute::Value(v) if v.len() <= MAX_CERTIFICATE_LEN => der = Some(v),
                    Attribute::Id(v) => id = Some(v),
                    _ => {}
                }
            }
            if let (Some(der), Some(id)) = (der, id) {
                out.push((der, id));
            }
        }
        out
    }

    fn keys(&self, class: ObjectClass, id: &[u8]) -> Vec<ObjectHandle> {
        let mut keys = self.session.find_objects(&[Attribute::Class(class), Attribute::Id(id.to_vec())]).unwrap_or_default();
        keys.truncate(MAX_KEYS_PER_ID);
        keys
    }

    fn key_facts(&self, key: ObjectHandle) -> KeyFacts {
        let mut facts = KeyFacts::default();
        let wanted = [AttributeType::KeyType, AttributeType::Modulus, AttributeType::EcPoint];
        for attribute in self.session.get_attributes(key, &wanted).unwrap_or_default() {
            match attribute {
                Attribute::KeyType(t) if t == KeyType::RSA => facts.kind = Some(KeyKindOnToken::Rsa),
                Attribute::KeyType(t) if t == KeyType::EC => facts.kind = Some(KeyKindOnToken::Ec),
                Attribute::Modulus(m) => facts.modulus = Some(m),
                Attribute::EcPoint(p) => facts.ec_point = Some(p),
                _ => {}
            }
        }
        facts
    }

    fn always_authenticate(&self, key: ObjectHandle) -> bool {
        self.session
            .get_attributes(key, &[AttributeType::AlwaysAuthenticate])
            .is_ok_and(|attributes| attributes.iter().any(|a| matches!(a, Attribute::AlwaysAuthenticate(true))))
    }

    fn login(&self, pin: Option<&Pin>) -> Result<(), Error> {
        let pin = pin.map(|p| AuthPin::from(p.expose().to_owned()));
        self.session.login(UserType::User, pin.as_ref())
    }

    fn sign(&self, mechanism: &Mechanism<'_>, key: ObjectHandle, input: &[u8]) -> Result<Vec<u8>, Error> {
        self.session.sign(mechanism, key, input)
    }

    fn decrypt(&self, mechanism: &Mechanism<'_>, key: ObjectHandle, input: &[u8]) -> Result<Vec<u8>, Error> {
        self.session.decrypt(mechanism, key, input)
    }
}
