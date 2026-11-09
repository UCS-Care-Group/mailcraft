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
//! (PKCS#11 v2.40 §4.6.3) whose public half is the certificate's: the RSA modulus, or the EC
//! point (read from the key, or from the public-key object with that id). A key that doesn't
//! match is never used.
//!
//! Logging in: a token with a protected authentication path (a PIN pad) is logged in to with no
//! PIN and asks on the reader. Any other token that needs a login ends the call with
//! [`KeyHolderError::PinNeeded`], naming the token by slot, serial number and label; the window
//! asks the owner and calls again with that [`TokenRef`] and the [`Pin`], which then goes to that
//! slot only, and only while it still holds the token with that serial number
//! ([`KeyHolderError::TokenChanged`] otherwise). A key marked `CKA_ALWAYS_AUTHENTICATE` (a PIV
//! card's signing key) needs a second, context-specific login between `C_SignInit` and
//! `C_Sign`, which `cryptoki`'s single-part `sign` and `decrypt` don't leave room for: such a key
//! is refused as unsupported for now (`docs/gaps.md`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

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
/// The most private keys with one `CKA_ID` looked at.
const MAX_KEYS_PER_ID: usize = 16;
/// The most modules loaded in one run.
const MAX_MODULES: usize = 8;
/// The longest token label or serial number kept, in characters.
const MAX_TOKEN_TEXT: usize = 64;

/// The module named in the environment, when it is an absolute path.
pub fn module_from_env() -> Option<PathBuf> {
    let named = PathBuf::from(std::env::var_os(MODULE_VARIABLE)?);
    named.is_absolute().then_some(named)
}

/// The module the owner named in the environment, else the first of p11-kit's proxy's places
/// that loads. A module the owner named is the only one tried.
pub fn open_default() -> Result<Arc<Pkcs11Holder>, KeyHolderError> {
    if let Some(named) = module_from_env() {
        return Pkcs11Holder::open(&named);
    }
    first_that_opens(PROXY_PLACES.iter().map(PathBuf::from).filter(|p| p.is_file()), Pkcs11Holder::open)
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

/// What is kept of each module loaded: never emptied, so that no module is unloaded.
type Registry<T> = Mutex<Vec<(PathBuf, Arc<T>)>>;

fn modules() -> &'static Registry<Pkcs11Holder> {
    static MODULES: OnceLock<Registry<Pkcs11Holder>> = OnceLock::new();
    MODULES.get_or_init(|| Mutex::new(Vec::new()))
}

/// The one `T` kept in `registry` for `key`, made by `make` the first time; at most `max` kept.
/// A failure is not kept, so a later call tries again.
pub fn shared<T, F>(registry: &Registry<T>, key: &Path, max: usize, make: F) -> Result<Arc<T>, KeyHolderError>
where
    F: FnOnce() -> Result<T, KeyHolderError>,
{
    let mut kept = registry.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, found)) = kept.iter().find(|(path, _)| path == key) {
        return Ok(Arc::clone(found));
    }
    if kept.len() >= max {
        return Err(KeyHolderError::Unavailable("too many PKCS#11 modules".into()));
    }
    let made = Arc::new(make()?);
    kept.push((key.to_path_buf(), Arc::clone(&made)));
    Ok(made)
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

/// A loaded PKCS#11 module, kept for the life of the program.
pub struct Pkcs11Holder {
    context: Pkcs11,
}

impl Pkcs11Holder {
    /// The module at `module`, an absolute path the owner chose: loaded and initialised the
    /// first time, the same one after.
    pub fn open(module: &Path) -> Result<Arc<Self>, KeyHolderError> {
        if !module.is_absolute() {
            return Err(KeyHolderError::Unavailable("the PKCS#11 module is not named by an absolute path".into()));
        }
        shared(modules(), module, MAX_MODULES, || {
            let context = Pkcs11::new(module).map_err(|e| KeyHolderError::Unavailable(e.to_string()))?;
            match context.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
                Ok(()) | Err(Error::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => Ok(Self { context }),
                Err(e) => Err(KeyHolderError::Unavailable(e.to_string())),
            }
        })
    }

    /// The certificates on the tokens present that have a private key and may be used for mail,
    /// expired ones included (marked).
    pub fn certificates(&self) -> Result<Vec<TokenCertificate>, KeyHolderError> {
        let now = now();
        let mut out: Vec<TokenCertificate> = Vec::new();
        for slot in self.slots()? {
            let Ok(token) = self.token(slot) else { continue };
            let Ok(session) = self.context.open_ro_session(slot) else { continue };
            for (der, id) in certificates_on(&session) {
                for certificate in super::usable([der], now) {
                    // Without a login a token may hide its private keys: a certificate with an
                    // id is listed then, and its key is looked for when it is used.
                    if !token.login_required && find_key(&session, &id, &certificate).is_none() {
                        continue;
                    }
                    if out.len() < MAX_CERTIFICATES && !out.iter().any(|c| c.certificate.fingerprint == certificate.fingerprint) {
                        out.push(TokenCertificate {
                            certificate,
                            token: token.token.clone(),
                            protected_path: token.protected_path,
                            login_required: token.login_required,
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    /// Signs `digest` with the key of the certificate whose SHA-256 is `fingerprint`; `pin` is
    /// the PIN the owner gave for the token a [`KeyHolderError::PinNeeded`] named. The result is
    /// the raw RSA signature, or DER `Ecdsa-Sig-Value` for ECDSA.
    pub fn sign_digest(
        &self,
        fingerprint: &[u8; 32],
        scheme: SignatureScheme,
        algorithm: DigestAlgorithm,
        digest: &[u8],
        pin: Option<(&TokenRef, &Pin)>,
    ) -> Result<Vec<u8>, KeyHolderError> {
        let digest = algorithm.check(digest)?;
        let found = self.key_for(fingerprint, pin)?;
        check_for_signing(&found.certificate, scheme)?;
        let (mechanism, input) = match scheme {
            SignatureScheme::RsaPkcs1v15 => (Mechanism::RsaPkcs, digest_info(algorithm, digest)?),
            SignatureScheme::RsaPss => {
                let (hash_alg, mgf) = hash_and_mgf(algorithm);
                let s_len = u64::try_from(algorithm.digest_len()).map_err(|_| KeyHolderError::BadInput("digest length"))?;
                (Mechanism::RsaPkcsPss(PkcsPssParams { hash_alg, mgf, s_len: s_len.into() }), digest.to_vec())
            }
            SignatureScheme::Ecdsa => (Mechanism::Ecdsa, digest.to_vec()),
        };
        let signature = found.session.sign(&mechanism, found.key, &input).map_err(|e| token_error(e, &found.token, found.pin_given))?;
        match scheme {
            SignatureScheme::Ecdsa => ecdsa_raw_to_der(&signature, found.certificate.key),
            _ => Ok(signature),
        }
    }

    /// Unwraps an RSA-encrypted content-encryption key with the key of the certificate whose
    /// SHA-256 is `fingerprint` (an expired certificate's too); `pin` as for signing.
    pub fn decrypt_key(
        &self,
        fingerprint: &[u8; 32],
        padding: RsaPadding,
        wrapped: &[u8],
        pin: Option<(&TokenRef, &Pin)>,
    ) -> Result<UnwrappedKey, KeyHolderError> {
        let wrapped = check_wrapped(wrapped)?;
        let found = self.key_for(fingerprint, pin)?;
        check_for_decrypting(&found.certificate)?;
        let mechanism = match padding {
            RsaPadding::Pkcs1v15 => Mechanism::RsaPkcs,
            RsaPadding::OaepSha1 => Mechanism::RsaPkcsOaep(PkcsOaepParams::new(MechanismType::SHA1, PkcsMgfType::MGF1_SHA1, PkcsOaepSource::empty())),
            RsaPadding::OaepSha256 => {
                Mechanism::RsaPkcsOaep(PkcsOaepParams::new(MechanismType::SHA256, PkcsMgfType::MGF1_SHA256, PkcsOaepSource::empty()))
            }
        };
        found.session.decrypt(&mechanism, found.key, wrapped).map(UnwrappedKey::new).map_err(|e| token_error(e, &found.token, found.pin_given))
    }

    fn slots(&self) -> Result<Vec<Slot>, KeyHolderError> {
        let mut slots = self.context.get_slots_with_token().map_err(|e| KeyHolderError::Unavailable(e.to_string()))?;
        slots.truncate(MAX_SLOTS);
        Ok(slots)
    }

    /// The token in `slot`: how it is named, whether it has a PIN pad, whether it needs a login.
    fn token(&self, slot: Slot) -> Result<TokenFacts, KeyHolderError> {
        let info = self.context.get_token_info(slot).map_err(|e| KeyHolderError::System(e.to_string()))?;
        Ok(TokenFacts {
            token: TokenRef { slot: slot.id(), serial: token_text(info.serial_number()), label: token_text(info.label()) },
            protected_path: info.protected_authentication_path(),
            login_required: info.login_required(),
        })
    }

    /// The token, session and private key for the certificate `fingerprint`, logged in. With a
    /// PIN, only the slot it was asked for is looked at.
    fn key_for(&self, fingerprint: &[u8; 32], pin: Option<(&TokenRef, &Pin)>) -> Result<FoundKey, KeyHolderError> {
        let named = pin.map(|(token, _)| token);
        for slot in self.slots()? {
            if named.is_some_and(|n| n.slot != slot.id()) {
                continue;
            }
            let Ok(facts) = self.token(slot) else { continue };
            // A PIN goes only to the token it was asked for: the same slot, the same serial number.
            check_same_token(named, &facts.token)?;
            let Ok(session) = self.context.open_ro_session(slot) else { continue };
            let Some((der, id)) = certificates_on(&session).into_iter().find(|(der, _)| sha256(der) == *fingerprint) else {
                if named.is_some() {
                    return Err(KeyHolderError::TokenChanged);
                }
                continue;
            };
            let certificate = HeldCertificate::read(der, now())?;
            if facts.login_required || pin.is_some() {
                let result = if facts.protected_path {
                    session.login(UserType::User, None)
                } else {
                    let Some((_, pin)) = pin else { return Err(KeyHolderError::PinNeeded(facts.token)) };
                    session.login(UserType::User, Some(&AuthPin::from(pin.expose().to_owned())))
                };
                match result {
                    Ok(()) | Err(Error::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
                    Err(e) => return Err(token_error(e, &facts.token, pin.is_some())),
                }
            }
            let key = find_key(&session, &id, &certificate).ok_or(KeyHolderError::NotFound)?;
            if always_authenticate(&session, key) {
                return Err(KeyHolderError::Unsupported("a key that asks for its PIN at every use"));
            }
            return Ok(FoundKey { session, key, certificate, token: facts.token, pin_given: pin.is_some() });
        }
        Err(if named.is_some() { KeyHolderError::TokenChanged } else { KeyHolderError::NotFound })
    }
}

struct TokenFacts {
    token: TokenRef,
    protected_path: bool,
    login_required: bool,
}

struct FoundKey {
    session: Session,
    key: ObjectHandle,
    certificate: HeldCertificate,
    token: TokenRef,
    pin_given: bool,
}

/// A PIN given for `named` goes only to the token that is still that one.
pub fn check_same_token(named: Option<&TokenRef>, found: &TokenRef) -> Result<(), KeyHolderError> {
    match named {
        Some(n) if n.slot != found.slot || n.serial != found.serial => Err(KeyHolderError::TokenChanged),
        _ => Ok(()),
    }
}

fn token_text(text: &str) -> String {
    clean_name(text.trim()).chars().take(MAX_TOKEN_TEXT).collect()
}

/// The X.509 certificates on a token with their `CKA_ID`: at most [`MAX_SCANNED`] looked at.
fn certificates_on(session: &Session) -> Vec<(Vec<u8>, Vec<u8>)> {
    let template = [Attribute::Class(ObjectClass::CERTIFICATE), Attribute::CertificateType(CertificateType::X_509)];
    let Ok(handles) = session.find_objects(&template) else { return Vec::new() };
    let mut out = Vec::new();
    for handle in handles.into_iter().take(MAX_SCANNED) {
        let Ok(attributes) = session.get_attributes(handle, &[AttributeType::Value, AttributeType::Id]) else { continue };
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

/// The private key with this `CKA_ID` whose public half is the certificate's.
fn find_key(session: &Session, id: &[u8], certificate: &HeldCertificate) -> Option<ObjectHandle> {
    if id.is_empty() {
        return None;
    }
    let template = [Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(id.to_vec())];
    let keys = session.find_objects(&template).ok()?;
    keys.into_iter().take(MAX_KEYS_PER_ID).find(|key| key_matches(certificate, &key_facts(session, *key, id)))
}

/// What the token says of `key`'s public half; an EC point not on the private key is taken from
/// the public-key object with the same id, when there is exactly one.
fn key_facts(session: &Session, key: ObjectHandle, id: &[u8]) -> KeyFacts {
    let mut facts = KeyFacts::default();
    let wanted = [AttributeType::KeyType, AttributeType::Modulus, AttributeType::EcPoint];
    for attribute in session.get_attributes(key, &wanted).unwrap_or_default() {
        match attribute {
            Attribute::KeyType(t) if t == KeyType::RSA => facts.kind = Some(KeyKindOnToken::Rsa),
            Attribute::KeyType(t) if t == KeyType::EC => facts.kind = Some(KeyKindOnToken::Ec),
            Attribute::Modulus(m) => facts.modulus = Some(m),
            Attribute::EcPoint(p) => facts.ec_point = Some(p),
            _ => {}
        }
    }
    if facts.kind == Some(KeyKindOnToken::Ec) && facts.ec_point.is_none() {
        let template = [Attribute::Class(ObjectClass::PUBLIC_KEY), Attribute::Id(id.to_vec())];
        if let Ok(public) = session.find_objects(&template)
            && let [public] = public.as_slice()
        {
            for attribute in session.get_attributes(*public, &[AttributeType::EcPoint]).unwrap_or_default() {
                if let Attribute::EcPoint(p) = attribute {
                    facts.ec_point = Some(p);
                }
            }
        }
    }
    facts
}

/// Whether the key needs a login at every use. A token that can't say is taken at its word
/// that it doesn't; a use that then needs one fails, after a PIN, as unsupported
/// ([`token_error`]), never with another request for the PIN.
fn always_authenticate(session: &Session, key: ObjectHandle) -> bool {
    session
        .get_attributes(key, &[AttributeType::AlwaysAuthenticate])
        .is_ok_and(|attributes| attributes.iter().any(|a| matches!(a, Attribute::AlwaysAuthenticate(true))))
}

fn hash_and_mgf(algorithm: DigestAlgorithm) -> (MechanismType, PkcsMgfType) {
    match algorithm {
        DigestAlgorithm::Sha256 => (MechanismType::SHA256, PkcsMgfType::MGF1_SHA256),
        DigestAlgorithm::Sha384 => (MechanismType::SHA384, PkcsMgfType::MGF1_SHA384),
        DigestAlgorithm::Sha512 => (MechanismType::SHA512, PkcsMgfType::MGF1_SHA512),
    }
}

/// A token's error for `token`, named; `pin_given` says whether the owner's PIN went with the
/// call. Its text never carries the PIN (`cryptoki` is never given it in a form it prints).
pub fn token_error(error: Error, token: &TokenRef, pin_given: bool) -> KeyHolderError {
    match error {
        Error::Pkcs11(RvError::PinIncorrect | RvError::PinInvalid | RvError::PinLenRange, _) => KeyHolderError::PinIncorrect,
        Error::Pkcs11(RvError::PinLocked, _) => KeyHolderError::PinLocked,
        Error::Pkcs11(RvError::PinExpired, _) => KeyHolderError::PinExpired,
        Error::Pkcs11(RvError::FunctionCanceled, _) => KeyHolderError::Cancelled,
        Error::Pkcs11(RvError::TokenNotPresent | RvError::DeviceRemoved | RvError::SessionHandleInvalid | RvError::SessionClosed, _) => {
            KeyHolderError::TokenChanged
        }
        Error::Pkcs11(RvError::UserNotLoggedIn, _) if pin_given => KeyHolderError::Unsupported("a key that asks for its PIN at every use"),
        Error::Pkcs11(RvError::UserNotLoggedIn, _) => KeyHolderError::PinNeeded(token.clone()),
        Error::Pkcs11(
            RvError::MechanismInvalid | RvError::MechanismParamInvalid | RvError::KeyTypeInconsistent | RvError::KeyFunctionNotPermitted,
            _,
        ) => KeyHolderError::Unsupported("the token doesn't do that with this key"),
        other => KeyHolderError::System(other.to_string()),
    }
}
