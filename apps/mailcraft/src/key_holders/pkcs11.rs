// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! PKCS#11 tokens on Linux and FreeBSD (PKCS#11 v2.40 / v3.0), through `cryptoki`'s safe API:
//! smartcards and USB keys through OpenSC, and whatever p11-kit's proxy gathers (the module
//! every desktop registers its tokens with).
//!
//! Loading a module runs its code in this process, so the module is named by the owner alone
//! (the settings, or `MAILCRAFT_PKCS11_MODULE` in the environment the owner started the
//! program with), by an absolute path, never by a command's params; with nothing named, the
//! first of p11-kit's proxy's usual places that exists is used ([`default_module`]).
//!
//! A certificate's private key is the token's private-key object with the same `CKA_ID`
//! (PKCS#11 v2.40 §4.6.3, as every token sets them). Logging in: a token with a protected
//! authentication path (a PIN pad) is logged in to with no PIN and asks on the reader; any other
//! token that needs a login takes the [`Pin`] the caller hands in, and without one the call
//! ends with [`KeyHolderError::PinNeeded`] naming the token, so that the window can ask.
//! A key marked `CKA_ALWAYS_AUTHENTICATE` (a PIV card's signing key) needs a second, context-
//! specific login between `C_SignInit` and `C_Sign`, which `cryptoki`'s single-part `sign` and
//! `decrypt` don't leave room for: such a key is refused as unsupported for now.

use std::path::{Path, PathBuf};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::error::{Error, RvError};
use cryptoki::mechanism::rsa::{PkcsMgfType, PkcsOaepParams, PkcsOaepSource, PkcsPssParams};
use cryptoki::mechanism::{Mechanism, MechanismType};
use cryptoki::object::{Attribute, AttributeType, CertificateType, ObjectClass, ObjectHandle};
use cryptoki::session::{Session, UserType};
use cryptoki::slot::Slot;
use cryptoki::types::AuthPin;

use super::certificate::{HeldCertificate, sha256};
use super::encoding::{digest_info, ecdsa_raw_to_der};
use super::{
    DigestAlgorithm, KeyHolderError, KeyKind, MAX_CERTIFICATE_LEN, MAX_CERTIFICATES, MAX_WRAPPED_KEY_LEN, Pin, RsaPadding, SignatureScheme,
    UnwrappedKey,
};

/// The environment variable that names a module, for an owner who starts the program with it.
pub const MODULE_VARIABLE: &str = "MAILCRAFT_PKCS11_MODULE";

/// Where p11-kit's proxy module usually is: Arch, Fedora, Debian and Ubuntu, FreeBSD.
const PROXY_PLACES: &[&str] = &[
    "/usr/lib/p11-kit-proxy.so",
    "/usr/lib64/p11-kit-proxy.so",
    "/usr/lib/x86_64-linux-gnu/p11-kit-proxy.so",
    "/usr/lib/aarch64-linux-gnu/p11-kit-proxy.so",
    "/usr/local/lib/p11-kit-proxy.so",
];

/// The most slots looked through.
const MAX_SLOTS: usize = 64;

/// The module named in the environment, when it is an absolute path.
pub fn module_from_env() -> Option<PathBuf> {
    let named = PathBuf::from(std::env::var_os(MODULE_VARIABLE)?);
    named.is_absolute().then_some(named)
}

/// The module to load when the owner named none: the environment's, else p11-kit's proxy.
pub fn default_module() -> Option<PathBuf> {
    module_from_env().or_else(|| PROXY_PLACES.iter().map(PathBuf::from).find(|p| p.is_file()))
}

/// A certificate on a token, with what it takes to use its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenCertificate {
    /// The certificate.
    pub certificate: HeldCertificate,
    /// The token's label, as the window names it when asking for its PIN.
    pub token: String,
    /// Whether the token asks for its PIN on the reader (a PIN pad).
    pub protected_path: bool,
    /// Whether the token needs a login before its key can be used.
    pub login_required: bool,
}

/// A loaded PKCS#11 module.
pub struct Pkcs11Holder {
    context: Pkcs11,
}

impl Pkcs11Holder {
    /// Loads and initialises the module at `module`, an absolute path the owner chose.
    pub fn open(module: &Path) -> Result<Self, KeyHolderError> {
        if !module.is_absolute() {
            return Err(KeyHolderError::Unavailable("the PKCS#11 module is not named by an absolute path".into()));
        }
        let context = Pkcs11::new(module).map_err(|e| KeyHolderError::Unavailable(e.to_string()))?;
        match context.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
            Ok(()) | Err(Error::Pkcs11(RvError::CryptokiAlreadyInitialized, _)) => {}
            Err(e) => return Err(KeyHolderError::Unavailable(e.to_string())),
        }
        Ok(Self { context })
    }

    /// The certificates on the tokens present that have a private key and may be used for mail.
    pub fn certificates(&self) -> Result<Vec<TokenCertificate>, KeyHolderError> {
        let mut out = Vec::new();
        for slot in self.slots()? {
            let Ok((token, protected_path, login_required)) = self.token(slot) else { continue };
            let Ok(session) = self.context.open_ro_session(slot) else { continue };
            for (der, id) in certificates_on(&session) {
                // Without a login a token may hide its private keys; a certificate with an id is
                // listed then, and the key is looked for when it is used.
                if !login_required && find_key(&session, &id).is_none() {
                    continue;
                }
                for certificate in super::usable([der]) {
                    if out.len() < MAX_CERTIFICATES && !out.iter().any(|c: &TokenCertificate| c.certificate.fingerprint == certificate.fingerprint) {
                        out.push(TokenCertificate { certificate, token: token.clone(), protected_path, login_required });
                    }
                }
            }
        }
        Ok(out)
    }

    /// Signs `digest` with the key of the certificate whose SHA-256 is `fingerprint`.
    /// The result is the raw RSA signature, or DER `Ecdsa-Sig-Value` for ECDSA.
    pub fn sign_digest(
        &self,
        fingerprint: &[u8; 32],
        scheme: SignatureScheme,
        algorithm: DigestAlgorithm,
        digest: &[u8],
        pin: Option<&Pin>,
    ) -> Result<Vec<u8>, KeyHolderError> {
        let digest = algorithm.check(digest)?;
        let found = self.key_for(fingerprint, pin)?;
        if !found.certificate.can_sign || !scheme.fits(found.certificate.key) {
            return Err(KeyHolderError::Unsupported("this certificate's key doesn't sign that way"));
        }
        let (mechanism, input) = match scheme {
            SignatureScheme::RsaPkcs1v15 => (Mechanism::RsaPkcs, digest_info(algorithm, digest)?),
            SignatureScheme::RsaPss => {
                let (hash_alg, mgf) = hash_and_mgf(algorithm);
                let s_len = u64::try_from(algorithm.digest_len()).map_err(|_| KeyHolderError::BadInput("digest length"))?;
                (Mechanism::RsaPkcsPss(PkcsPssParams { hash_alg, mgf, s_len: s_len.into() }), digest.to_vec())
            }
            SignatureScheme::Ecdsa => (Mechanism::Ecdsa, digest.to_vec()),
        };
        let signature = found.session.sign(&mechanism, found.key, &input).map_err(token_error)?;
        match scheme {
            SignatureScheme::Ecdsa => ecdsa_raw_to_der(&signature),
            _ => Ok(signature),
        }
    }

    /// Unwraps an RSA-encrypted content-encryption key with the key of the certificate whose
    /// SHA-256 is `fingerprint`.
    pub fn decrypt_key(
        &self,
        fingerprint: &[u8; 32],
        padding: RsaPadding,
        wrapped: &[u8],
        pin: Option<&Pin>,
    ) -> Result<UnwrappedKey, KeyHolderError> {
        if wrapped.is_empty() || wrapped.len() > MAX_WRAPPED_KEY_LEN {
            return Err(KeyHolderError::BadInput("the wrapped key's length"));
        }
        let found = self.key_for(fingerprint, pin)?;
        if !found.certificate.can_decrypt {
            return Err(KeyHolderError::Unsupported("this certificate's key doesn't unwrap keys"));
        }
        let mechanism = match padding {
            RsaPadding::Pkcs1v15 => Mechanism::RsaPkcs,
            RsaPadding::OaepSha1 => Mechanism::RsaPkcsOaep(PkcsOaepParams::new(MechanismType::SHA1, PkcsMgfType::MGF1_SHA1, PkcsOaepSource::empty())),
            RsaPadding::OaepSha256 => {
                Mechanism::RsaPkcsOaep(PkcsOaepParams::new(MechanismType::SHA256, PkcsMgfType::MGF1_SHA256, PkcsOaepSource::empty()))
            }
        };
        found.session.decrypt(&mechanism, found.key, wrapped).map(UnwrappedKey::new).map_err(token_error)
    }

    fn slots(&self) -> Result<Vec<Slot>, KeyHolderError> {
        let mut slots = self.context.get_slots_with_token().map_err(|e| KeyHolderError::Unavailable(e.to_string()))?;
        slots.truncate(MAX_SLOTS);
        Ok(slots)
    }

    /// A token's label, whether it has a protected path, whether it needs a login.
    fn token(&self, slot: Slot) -> Result<(String, bool, bool), KeyHolderError> {
        let info = self.context.get_token_info(slot).map_err(token_error)?;
        let label: String = info.label().trim().chars().filter(|c| !c.is_control()).take(64).collect();
        Ok((label, info.protected_authentication_path(), info.login_required()))
    }

    /// The token, session and private key for the certificate `fingerprint`, logged in.
    fn key_for(&self, fingerprint: &[u8; 32], pin: Option<&Pin>) -> Result<FoundKey, KeyHolderError> {
        for slot in self.slots()? {
            let Ok((token, protected_path, login_required)) = self.token(slot) else { continue };
            let Ok(session) = self.context.open_ro_session(slot) else { continue };
            let Some((der, id)) = certificates_on(&session).into_iter().find(|(der, _)| sha256(der) == *fingerprint) else { continue };
            let certificate = HeldCertificate::from_der(der)?;
            if login_required {
                let result = if protected_path {
                    session.login(UserType::User, None)
                } else {
                    let Some(pin) = pin else { return Err(KeyHolderError::PinNeeded { token }) };
                    session.login(UserType::User, Some(&AuthPin::from(pin.expose().to_owned())))
                };
                match result {
                    Ok(()) | Err(Error::Pkcs11(RvError::UserAlreadyLoggedIn, _)) => {}
                    Err(e) => return Err(token_error(e)),
                }
            }
            let key = find_key(&session, &id).ok_or(KeyHolderError::NotFound)?;
            if always_authenticate(&session, key) {
                return Err(KeyHolderError::Unsupported("a key that asks for its PIN at every use"));
            }
            if matches!(certificate.key, KeyKind::Other) {
                return Err(KeyHolderError::Unsupported("this kind of key"));
            }
            return Ok(FoundKey { session, key, certificate });
        }
        Err(KeyHolderError::NotFound)
    }
}

struct FoundKey {
    session: Session,
    key: ObjectHandle,
    certificate: HeldCertificate,
}

/// The X.509 certificates on a token with their `CKA_ID`, at most [`MAX_CERTIFICATES`].
fn certificates_on(session: &Session) -> Vec<(Vec<u8>, Vec<u8>)> {
    let template = [Attribute::Class(ObjectClass::CERTIFICATE), Attribute::CertificateType(CertificateType::X_509)];
    let Ok(handles) = session.find_objects(&template) else { return Vec::new() };
    let mut out = Vec::new();
    for handle in handles.into_iter().take(MAX_CERTIFICATES) {
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

/// The private key with this `CKA_ID`, when the token shows exactly one.
fn find_key(session: &Session, id: &[u8]) -> Option<ObjectHandle> {
    if id.is_empty() {
        return None;
    }
    let template = [Attribute::Class(ObjectClass::PRIVATE_KEY), Attribute::Id(id.to_vec())];
    let found = session.find_objects(&template).ok()?;
    match found.as_slice() {
        [key] => Some(*key),
        _ => None,
    }
}

fn always_authenticate(session: &Session, key: ObjectHandle) -> bool {
    session
        .get_attributes(key, &[AttributeType::AlwaysAuthenticate])
        .map(|attributes| attributes.iter().any(|a| matches!(a, Attribute::AlwaysAuthenticate(true))))
        .unwrap_or(false)
}

fn hash_and_mgf(algorithm: DigestAlgorithm) -> (MechanismType, PkcsMgfType) {
    match algorithm {
        DigestAlgorithm::Sha256 => (MechanismType::SHA256, PkcsMgfType::MGF1_SHA256),
        DigestAlgorithm::Sha384 => (MechanismType::SHA384, PkcsMgfType::MGF1_SHA384),
        DigestAlgorithm::Sha512 => (MechanismType::SHA512, PkcsMgfType::MGF1_SHA512),
    }
}

/// A token's error, named; its text never carries the PIN (`cryptoki` is never given it in a
/// form it prints).
pub(super) fn token_error(error: Error) -> KeyHolderError {
    match error {
        Error::Pkcs11(RvError::PinIncorrect | RvError::PinInvalid | RvError::PinLenRange, _) => KeyHolderError::PinIncorrect,
        Error::Pkcs11(RvError::PinLocked | RvError::PinExpired, _) => KeyHolderError::PinLocked,
        Error::Pkcs11(RvError::FunctionCanceled, _) => KeyHolderError::Cancelled,
        Error::Pkcs11(RvError::UserNotLoggedIn, _) => KeyHolderError::PinNeeded { token: String::new() },
        Error::Pkcs11(
            RvError::MechanismInvalid | RvError::MechanismParamInvalid | RvError::KeyTypeInconsistent | RvError::KeyFunctionNotPermitted,
            _,
        ) => KeyHolderError::Unsupported("the token doesn't do that with this key"),
        other => KeyHolderError::System(other.to_string()),
    }
}
