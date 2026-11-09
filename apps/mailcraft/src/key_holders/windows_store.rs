// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's certificate store on Windows (Current User › Personal, "MY"), through the
//! Windows Runtime's certificate classes (`Windows.Security.Cryptography.Certificates` and
//! `.Core`), which the `windows` crate projects as safe calls.
//!
//! The private key stays with its key storage provider (the software provider, the TPM's, a
//! smartcard's): `PersistedKeyProvider.OpenKeyPairFromCertificateAsync` opens it, and
//! `CryptographicEngine.SignHashedData` / `Decrypt` use it. A PIN or a consent is asked for by
//! Windows itself, in its own window; MailCraft never sees it. Each call waits for the system,
//! so it is made on a background thread, never the window's.
//!
//! The store's enumeration leaves out expired certificates (as Microsoft documents it), which
//! is what signing wants; decrypting old mail with an expired certificate's key is part 6's
//! question.

use base64::Engine as _;
use windows::Security::Cryptography::Certificates::{
    Certificate, CertificateEnrollmentManager, CertificateQuery, CertificateStores, ExportOption, InstallOptions, KeyProtectionLevel,
};
use windows::Security::Cryptography::Core::{CryptographicEngine, CryptographicKey, CryptographicPadding, PersistedKeyProvider};
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Storage::Streams::IBuffer;
use windows::core::{Array, HSTRING};

use super::certificate::sha256;
use super::encoding::ecdsa_raw_to_der;
use super::{
    DigestAlgorithm, HeldCertificate, KeyHolderError, KeyKind, MAX_CERTIFICATE_LEN, MAX_CERTIFICATES, MAX_P12_LEN, MAX_WRAPPED_KEY_LEN, Pin,
    RsaPadding, SignatureScheme, UnwrappedKey,
};

/// How strongly an imported key is guarded by Windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportProtection {
    /// Used without asking, while the owner is signed in to Windows.
    NoConsent,
    /// Windows asks the owner each time the key is used.
    Consent,
    /// Windows asks for a password of the owner's choosing, set at import.
    ConsentWithPassword,
}

/// The certificates in Current User › Personal that have a private key and may be used for mail.
pub fn certificates() -> Result<Vec<HeldCertificate>, KeyHolderError> {
    let found = personal()?;
    Ok(super::usable(found.into_iter().map(|(_, der)| der)))
}

/// Signs `digest` with the key of the certificate whose SHA-256 is `fingerprint`: the raw RSA
/// signature, or DER `Ecdsa-Sig-Value` for ECDSA.
pub fn sign_digest(fingerprint: &[u8; 32], scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    let digest = algorithm.check(digest)?;
    let (certificate, held) = find(fingerprint)?;
    if !held.can_sign || !scheme.fits(held.key) {
        return Err(KeyHolderError::Unsupported("this certificate's key doesn't sign that way"));
    }
    let padding = match scheme {
        SignatureScheme::RsaPkcs1v15 => CryptographicPadding::RsaPkcs1V15,
        SignatureScheme::RsaPss => CryptographicPadding::RsaPss,
        SignatureScheme::Ecdsa => CryptographicPadding::None,
    };
    let key = open_key(&certificate, hash_name(algorithm), padding)?;
    let data = CryptographicBuffer::CreateFromByteArray(digest).map_err(system)?;
    let signature = bytes(&CryptographicEngine::SignHashedData(&key, &data).map_err(system)?)?;
    match scheme {
        // CNG gives an ECDSA signature as `r || s`.
        SignatureScheme::Ecdsa => ecdsa_raw_to_der(&signature),
        _ => Ok(signature),
    }
}

/// Unwraps an RSA-encrypted content-encryption key with the key of the certificate whose
/// SHA-256 is `fingerprint`. The Windows Runtime's OAEP is SHA-1's alone.
pub fn decrypt_key(fingerprint: &[u8; 32], padding: RsaPadding, wrapped: &[u8]) -> Result<UnwrappedKey, KeyHolderError> {
    if wrapped.is_empty() || wrapped.len() > MAX_WRAPPED_KEY_LEN {
        return Err(KeyHolderError::BadInput("the wrapped key's length"));
    }
    let (certificate, held) = find(fingerprint)?;
    if !held.can_decrypt {
        return Err(KeyHolderError::Unsupported("this certificate's key doesn't unwrap keys"));
    }
    let padding = match padding {
        RsaPadding::Pkcs1v15 => CryptographicPadding::RsaPkcs1V15,
        RsaPadding::OaepSha1 => CryptographicPadding::RsaOaep,
        RsaPadding::OaepSha256 => return Err(KeyHolderError::Unsupported("RSA-OAEP with SHA-256 on Windows")),
    };
    // "For encryption, the hash algorithm is ignored."
    let key = open_key(&certificate, "SHA256", padding)?;
    let data = CryptographicBuffer::CreateFromByteArray(wrapped).map_err(system)?;
    let plain = CryptographicEngine::Decrypt(&key, &data, None::<&IBuffer>).map_err(system)?;
    bytes(&plain).map(UnwrappedKey::new)
}

/// Imports a `.p12` (PFX) into Current User › Personal, its key not exportable, guarded as
/// `protection` says. The passphrase is handed to Windows and not kept.
pub fn import_p12(p12: &[u8], passphrase: &Pin, protection: ImportProtection) -> Result<(), KeyHolderError> {
    if p12.is_empty() || p12.len() > MAX_P12_LEN {
        return Err(KeyHolderError::BadInput("the .p12 file's size"));
    }
    let manager = CertificateEnrollmentManager::UserCertificateEnrollmentManager().map_err(system)?;
    let level = match protection {
        ImportProtection::NoConsent => KeyProtectionLevel::NoConsent,
        ImportProtection::Consent => KeyProtectionLevel::ConsentOnly,
        ImportProtection::ConsentWithPassword => KeyProtectionLevel::ConsentWithPassword,
    };
    let data = HSTRING::from(base64::engine::general_purpose::STANDARD.encode(p12));
    let password = HSTRING::from(passphrase.expose());
    manager
        .ImportPfxDataAsync(&data, &password, ExportOption::NotExportable, level, InstallOptions::None, &HSTRING::new())
        .map_err(system)?
        .join()
        .map_err(system)
}

/// Every certificate in Current User › Personal with a private key, with its DER.
fn personal() -> Result<Vec<(Certificate, Vec<u8>)>, KeyHolderError> {
    let query = CertificateQuery::new().map_err(|e| KeyHolderError::Unavailable(e.message()))?;
    query.SetStoreName(&HSTRING::from("MY")).map_err(system)?;
    let found = CertificateStores::FindAllWithQueryAsync(&query)
        .map_err(|e| KeyHolderError::Unavailable(e.message()))?
        .join()
        .map_err(|e| KeyHolderError::Unavailable(e.message()))?;
    let mut out = Vec::new();
    for certificate in found.into_iter().take(MAX_CERTIFICATES) {
        if !certificate.HasPrivateKey().unwrap_or(false) {
            continue;
        }
        let Ok(blob) = certificate.GetCertificateBlob() else { continue };
        let Ok(der) = bytes(&blob) else { continue };
        if der.len() <= MAX_CERTIFICATE_LEN {
            out.push((certificate, der));
        }
    }
    Ok(out)
}

fn find(fingerprint: &[u8; 32]) -> Result<(Certificate, HeldCertificate), KeyHolderError> {
    let (certificate, der) = personal()?.into_iter().find(|(_, der)| sha256(der) == *fingerprint).ok_or(KeyHolderError::NotFound)?;
    let held = HeldCertificate::from_der(der)?;
    if matches!(held.key, KeyKind::Other) {
        return Err(KeyHolderError::Unsupported("this kind of key"));
    }
    Ok((certificate, held))
}

fn open_key(certificate: &Certificate, hash: &str, padding: CryptographicPadding) -> Result<CryptographicKey, KeyHolderError> {
    PersistedKeyProvider::OpenKeyPairFromCertificateAsync(certificate, &HSTRING::from(hash), padding).map_err(system)?.join().map_err(system)
}

fn hash_name(algorithm: DigestAlgorithm) -> &'static str {
    match algorithm {
        DigestAlgorithm::Sha256 => "SHA256",
        DigestAlgorithm::Sha384 => "SHA384",
        DigestAlgorithm::Sha512 => "SHA512",
    }
}

/// A buffer's bytes, at most a certificate's size.
fn bytes(buffer: &IBuffer) -> Result<Vec<u8>, KeyHolderError> {
    let len = usize::try_from(buffer.Length().map_err(system)?).unwrap_or(usize::MAX);
    if len > MAX_CERTIFICATE_LEN {
        return Err(KeyHolderError::System("the system handed back too much".into()));
    }
    let mut array = Array::<u8>::new();
    CryptographicBuffer::CopyToByteArray(buffer, &mut array).map_err(system)?;
    Ok(array.to_vec())
}

/// The system's error. `ERROR_CANCELLED` (0x800704C7) and `SCARD_W_CANCELLED_BY_USER`
/// (0x8010006E) are the owner closing its prompt.
fn system(error: windows::core::Error) -> KeyHolderError {
    // HRESULTs are i32; these are their bit patterns.
    const ERROR_CANCELLED: u32 = 0x8007_04C7;
    const SCARD_W_CANCELLED_BY_USER: u32 = 0x8010_006E;
    let code = u32::from_ne_bytes(error.code().0.to_ne_bytes());
    if code == ERROR_CANCELLED || code == SCARD_W_CANCELLED_BY_USER {
        return KeyHolderError::Cancelled;
    }
    KeyHolderError::System(error.message())
}
