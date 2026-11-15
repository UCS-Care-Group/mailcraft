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
//!
//! The keychain search gives expired identities as well; they are listed as on the other
//! systems, marked, and sign nothing; their keys still unwrap old mail.

use security_framework::identity::SecIdentity;
use security_framework::import_export::Pkcs12ImportOptions;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
use security_framework::key::Algorithm;

use super::certificate::sha256;
use super::{
    DigestAlgorithm, HeldCertificate, KeyHolderError, MAX_CERTIFICATES, MAX_SCANNED, Pin, RsaPadding, SignatureScheme, UnwrappedKey,
    check_for_decrypting, check_for_signing, check_p12, check_wrapped, now,
};

/// `errSecItemNotFound`: a search that found nothing.
const ITEM_NOT_FOUND: i32 = -25300;
/// `errSecUserCanceled`: the owner closed the system's prompt.
const USER_CANCELED: i32 = -128;
/// `errSecAuthFailed`: a wrong keychain password or passphrase.
const AUTH_FAILED: i32 = -25293;
/// `errSecPkcs12VerifyFailure`: a wrong `.p12` passphrase.
const PKCS12_VERIFY_FAILURE: i32 = -25264;

/// The identities in the owner's keychains whose certificates may be used for mail, expired
/// ones included (marked).
pub fn certificates() -> Result<Vec<HeldCertificate>, KeyHolderError> {
    Ok(super::usable(identities()?.into_iter().map(|(_, der)| der), now()))
}

/// Signs `digest` with the key of the certificate whose SHA-256 is `fingerprint`: the raw RSA
/// signature, or DER `Ecdsa-Sig-Value` for ECDSA (what the keychain's X9.62 algorithms give).
/// The certificate must be for mail and in its dates.
pub fn sign_digest(fingerprint: &[u8; 32], scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    let digest = algorithm.check(digest)?;
    let (identity, held) = find(fingerprint)?;
    check_for_signing(&held, scheme)?;
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
/// SHA-256 is `fingerprint` (an expired certificate's too).
pub fn decrypt_key(fingerprint: &[u8; 32], padding: RsaPadding, wrapped: &[u8]) -> Result<UnwrappedKey, KeyHolderError> {
    let wrapped = check_wrapped(wrapped)?;
    let (identity, held) = find(fingerprint)?;
    check_for_decrypting(&held)?;
    let algorithm = match padding {
        RsaPadding::Pkcs1v15 => Algorithm::RSAEncryptionPKCS1,
        RsaPadding::OaepSha1 => Algorithm::RSAEncryptionOAEPSHA1,
        RsaPadding::OaepSha256 => Algorithm::RSAEncryptionOAEPSHA256,
    };
    let key = identity.private_key().map_err(status)?;
    key.decrypt_data(algorithm, wrapped).map(UnwrappedKey::new).map_err(|e| cf_error(e.code(), e.description().to_string()))
}

/// Imports a `.p12` into the owner's default keychain (the login keychain), where the keychain
/// asks before another program uses the key. Gives the fingerprints of the certificates that
/// came in with a key. The authorities' certificates in the file go into the keychain too, with
/// no trust set on them: macOS asks the owner itself before it trusts a root.
///
/// The passphrase has to reach macOS as a `CFString`, a copy that safe code can't overwrite: it
/// is made inside the import's options, which live for this one statement, and nothing else of
/// it is made.
pub fn import_p12(p12: &[u8], passphrase: &Pin) -> Result<Vec<[u8; 32]>, KeyHolderError> {
    let p12 = check_p12(p12)?;
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

/// The identities the keychains hold, with their certificates' DER: at most [`MAX_SCANNED`].
fn identities() -> Result<Vec<(SecIdentity, Vec<u8>)>, KeyHolderError> {
    let limit = i64::try_from(MAX_SCANNED).unwrap_or(i64::MAX);
    let results = match ItemSearchOptions::new().class(ItemClass::identity()).load_refs(true).limit(Limit::Max(limit)).search() {
        Ok(results) => results,
        Err(e) if e.code() == ITEM_NOT_FOUND => return Ok(Vec::new()),
        Err(e) => return Err(KeyHolderError::Unavailable(describe(e))),
    };
    let mut out = Vec::new();
    for result in results.into_iter().take(MAX_SCANNED) {
        let SearchResult::Ref(Reference::Identity(identity)) = result else { continue };
        let Ok(certificate) = identity.certificate() else { continue };
        out.push((identity, certificate.to_der()));
    }
    Ok(out)
}

fn find(fingerprint: &[u8; 32]) -> Result<(SecIdentity, HeldCertificate), KeyHolderError> {
    let (identity, der) = identities()?.into_iter().find(|(_, der)| sha256(der) == *fingerprint).ok_or(KeyHolderError::NotFound)?;
    Ok((identity, HeldCertificate::read(der, now())?))
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
