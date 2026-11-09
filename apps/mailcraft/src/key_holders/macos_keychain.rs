// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's keychains on macOS, through `security-framework`'s safe API: identities (a
//! certificate with its private key) in the keychains the system searches, smartcards that
//! CryptoTokenKit shows among them, and `SecKey`'s signing and decrypting.
//!
//! The private key stays in the keychain or on the card: a keychain password, a card's PIN or
//! a "MailCraft wants to use your key" consent is asked for by macOS itself, in its own window;
//! MailCraft never sees it. Each call may wait on that prompt, so it is made on a background
//! thread, never the window's.

use security_framework::identity::SecIdentity;
use security_framework::import_export::Pkcs12ImportOptions;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
use security_framework::key::Algorithm;

use super::certificate::sha256;
use super::{
    DigestAlgorithm, HeldCertificate, KeyHolderError, KeyKind, MAX_CERTIFICATE_LEN, MAX_CERTIFICATES, MAX_P12_LEN, MAX_WRAPPED_KEY_LEN, Pin,
    RsaPadding, SignatureScheme, UnwrappedKey,
};

/// `errSecItemNotFound`: a search that found nothing.
const ITEM_NOT_FOUND: i32 = -25300;
/// `errSecUserCanceled`: the owner closed the system's prompt.
const USER_CANCELED: i32 = -128;
/// `errSecAuthFailed`: a wrong keychain password or passphrase.
const AUTH_FAILED: i32 = -25293;
/// `errSecPkcs12VerifyFailure`: a wrong `.p12` passphrase.
const PKCS12_VERIFY_FAILURE: i32 = -25264;

/// The identities in the owner's keychains whose certificates may be used for mail.
pub fn certificates() -> Result<Vec<HeldCertificate>, KeyHolderError> {
    Ok(super::usable(identities()?.into_iter().map(|(_, der)| der)))
}

/// Signs `digest` with the key of the certificate whose SHA-256 is `fingerprint`: the raw RSA
/// signature, or DER `Ecdsa-Sig-Value` for ECDSA (what the keychain's X9.62 algorithms give).
pub fn sign_digest(fingerprint: &[u8; 32], scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    let digest = algorithm.check(digest)?;
    let (identity, held) = find(fingerprint)?;
    if !held.can_sign || !scheme.fits(held.key) {
        return Err(KeyHolderError::Unsupported("this certificate's key doesn't sign that way"));
    }
    let algorithm = match (scheme, algorithm) {
        (SignatureScheme::RsaPkcs1v15, DigestAlgorithm::Sha256) => Algorithm::RSASignatureDigestPKCS1v15SHA256,
        (SignatureScheme::RsaPkcs1v15, DigestAlgorithm::Sha384) => Algorithm::RSASignatureDigestPKCS1v15SHA384,
        (SignatureScheme::RsaPkcs1v15, DigestAlgorithm::Sha512) => Algorithm::RSASignatureDigestPKCS1v15SHA512,
        (SignatureScheme::RsaPss, DigestAlgorithm::Sha256) => Algorithm::RSASignatureDigestPSSSHA256,
        (SignatureScheme::RsaPss, DigestAlgorithm::Sha384) => Algorithm::RSASignatureDigestPSSSHA384,
        (SignatureScheme::RsaPss, DigestAlgorithm::Sha512) => Algorithm::RSASignatureDigestPSSSHA512,
        (SignatureScheme::Ecdsa, DigestAlgorithm::Sha256) => Algorithm::ECDSASignatureDigestX962SHA256,
        (SignatureScheme::Ecdsa, DigestAlgorithm::Sha384) => Algorithm::ECDSASignatureDigestX962SHA384,
        (SignatureScheme::Ecdsa, DigestAlgorithm::Sha512) => Algorithm::ECDSASignatureDigestX962SHA512,
    };
    let key = identity.private_key().map_err(status)?;
    key.create_signature(algorithm, digest).map_err(|e| cf_error(e.code(), e.description().to_string()))
}

/// Unwraps an RSA-encrypted content-encryption key with the key of the certificate whose
/// SHA-256 is `fingerprint`.
pub fn decrypt_key(fingerprint: &[u8; 32], padding: RsaPadding, wrapped: &[u8]) -> Result<UnwrappedKey, KeyHolderError> {
    if wrapped.is_empty() || wrapped.len() > MAX_WRAPPED_KEY_LEN {
        return Err(KeyHolderError::BadInput("the wrapped key's length"));
    }
    let (identity, held) = find(fingerprint)?;
    if !held.can_decrypt {
        return Err(KeyHolderError::Unsupported("this certificate's key doesn't unwrap keys"));
    }
    let algorithm = match padding {
        RsaPadding::Pkcs1v15 => Algorithm::RSAEncryptionPKCS1,
        RsaPadding::OaepSha1 => Algorithm::RSAEncryptionOAEPSHA1,
        RsaPadding::OaepSha256 => Algorithm::RSAEncryptionOAEPSHA256,
    };
    let key = identity.private_key().map_err(status)?;
    key.decrypt_data(algorithm, wrapped).map(UnwrappedKey::new).map_err(|e| cf_error(e.code(), e.description().to_string()))
}

/// Imports a `.p12` into the owner's default keychain (the login keychain). The passphrase is
/// handed to macOS and not kept. Gives the fingerprints of the certificates that came in with a
/// key.
pub fn import_p12(p12: &[u8], passphrase: &Pin) -> Result<Vec<[u8; 32]>, KeyHolderError> {
    if p12.is_empty() || p12.len() > MAX_P12_LEN {
        return Err(KeyHolderError::BadInput("the .p12 file's size"));
    }
    let imported = Pkcs12ImportOptions::new().passphrase(passphrase.expose()).import(p12).map_err(|e| match e.code() {
        AUTH_FAILED | PKCS12_VERIFY_FAILURE => KeyHolderError::PinIncorrect,
        _ => status(e),
    })?;
    let mut fingerprints = Vec::new();
    for identity in imported.into_iter().filter_map(|i| i.identity).take(MAX_CERTIFICATES) {
        if let Ok(certificate) = identity.certificate() {
            fingerprints.push(sha256(&certificate.to_der()));
        }
    }
    Ok(fingerprints)
}

/// Every identity the keychains hold, with its certificate's DER.
fn identities() -> Result<Vec<(SecIdentity, Vec<u8>)>, KeyHolderError> {
    let limit = i64::try_from(MAX_CERTIFICATES).unwrap_or(i64::MAX);
    let results = match ItemSearchOptions::new().class(ItemClass::identity()).load_refs(true).limit(Limit::Max(limit)).search() {
        Ok(results) => results,
        Err(e) if e.code() == ITEM_NOT_FOUND => return Ok(Vec::new()),
        Err(e) => return Err(KeyHolderError::Unavailable(describe(e))),
    };
    let mut out = Vec::new();
    for result in results {
        let SearchResult::Ref(Reference::Identity(identity)) = result else { continue };
        let Ok(certificate) = identity.certificate() else { continue };
        let der = certificate.to_der();
        if der.len() <= MAX_CERTIFICATE_LEN {
            out.push((identity, der));
        }
    }
    Ok(out)
}

fn find(fingerprint: &[u8; 32]) -> Result<(SecIdentity, HeldCertificate), KeyHolderError> {
    let (identity, der) = identities()?.into_iter().find(|(_, der)| sha256(der) == *fingerprint).ok_or(KeyHolderError::NotFound)?;
    let held = HeldCertificate::from_der(der)?;
    if matches!(held.key, KeyKind::Other) {
        return Err(KeyHolderError::Unsupported("this kind of key"));
    }
    Ok((identity, held))
}

fn describe(error: security_framework::base::Error) -> String {
    error.message().unwrap_or_else(|| format!("error {}", error.code()))
}

fn status(error: security_framework::base::Error) -> KeyHolderError {
    if error.code() == USER_CANCELED { KeyHolderError::Cancelled } else { KeyHolderError::System(describe(error)) }
}

/// A `CFError` from `SecKey`, by its code and description.
fn cf_error(code: isize, description: String) -> KeyHolderError {
    if code == USER_CANCELED as isize { KeyHolderError::Cancelled } else { KeyHolderError::System(description) }
}
