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
//! Expired certificates are listed as on the other systems (the query is asked to include
//! them), marked, and sign nothing; their keys still unwrap old mail.

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
    DigestAlgorithm, HeldCertificate, KeyHolderError, MAX_CERTIFICATE_LEN, MAX_SCANNED, Pin, RsaPadding, SignatureScheme, UnwrappedKey,
    check_for_decrypting, check_for_signing, check_p12, check_wrapped, now,
};

/// How Windows guards a key imported from a `.p12`. Either way Windows asks before the key is
/// used, so an imported key stays protected as the file was (R13): a key usable without a word
/// is not offered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportProtection {
    /// Windows asks the owner each time the key is used.
    #[default]
    Consent,
    /// Windows asks for a password of the owner's choosing, set at import, each time.
    ConsentWithPassword,
}

/// The certificates in Current User › Personal that have a private key and may be used for
/// mail, expired ones included (marked).
pub fn certificates() -> Result<Vec<HeldCertificate>, KeyHolderError> {
    let found = personal()?;
    Ok(super::usable(found.into_iter().map(|(_, der)| der), now()))
}

/// Signs `digest` with the key of the certificate whose SHA-256 is `fingerprint`: the raw RSA
/// signature, or DER `Ecdsa-Sig-Value` for ECDSA. The certificate must be for mail and in its
/// dates.
pub fn sign_digest(fingerprint: &[u8; 32], scheme: SignatureScheme, algorithm: DigestAlgorithm, digest: &[u8]) -> Result<Vec<u8>, KeyHolderError> {
    let digest = algorithm.check(digest)?;
    let (certificate, held) = find(fingerprint)?;
    check_for_signing(&held, scheme)?;
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
        SignatureScheme::Ecdsa => ecdsa_raw_to_der(&signature, held.key),
        _ => Ok(signature),
    }
}

/// Unwraps an RSA-encrypted content-encryption key with the key of the certificate whose
/// SHA-256 is `fingerprint` (an expired certificate's too). The Windows Runtime's OAEP is
/// SHA-1's alone.
pub fn decrypt_key(fingerprint: &[u8; 32], padding: RsaPadding, wrapped: &[u8]) -> Result<UnwrappedKey, KeyHolderError> {
    let wrapped = check_wrapped(wrapped)?;
    let (certificate, held) = find(fingerprint)?;
    check_for_decrypting(&held)?;
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
/// `protection` says ([`ImportProtection::default`]: asked each time).
///
/// The certificates of the authorities that come in the file go to Windows as well; Windows
/// asks the owner itself, in its own window, before it trusts a root among them.
///
/// The passphrase has to reach Windows as a Windows string (`HSTRING`), a copy that safe code
/// can't overwrite: it is made inside the call and let go of when the call returns, and
/// nothing else of it is made.
pub fn import_p12(p12: &[u8], passphrase: &Pin, protection: ImportProtection) -> Result<(), KeyHolderError> {
    let p12 = check_p12(p12)?;
    let manager = CertificateEnrollmentManager::UserCertificateEnrollmentManager().map_err(system)?;
    let level = match protection {
        ImportProtection::Consent => KeyProtectionLevel::ConsentOnly,
        ImportProtection::ConsentWithPassword => KeyProtectionLevel::ConsentWithPassword,
    };
    let data = HSTRING::from(base64::engine::general_purpose::STANDARD.encode(p12));
    let started = manager.ImportPfxDataAsync(
        &data,
        &HSTRING::from(passphrase.expose()),
        ExportOption::NotExportable,
        level,
        InstallOptions::None,
        &HSTRING::new(),
    );
    started.map_err(system)?.join().map_err(system)
}

/// The certificates in Current User › Personal with a private key, expired ones included, with
/// their DER: at most [`MAX_SCANNED`] looked through.
fn personal() -> Result<Vec<(Certificate, Vec<u8>)>, KeyHolderError> {
    let query = CertificateQuery::new().map_err(|e| KeyHolderError::Unavailable(e.message()))?;
    query.SetStoreName(&HSTRING::from("MY")).map_err(system)?;
    query.SetIncludeExpiredCertificates(true).map_err(system)?;
    let found = CertificateStores::FindAllWithQueryAsync(&query)
        .map_err(|e| KeyHolderError::Unavailable(e.message()))?
        .join()
        .map_err(|e| KeyHolderError::Unavailable(e.message()))?;
    let mut out = Vec::new();
    for certificate in found.into_iter().take(MAX_SCANNED) {
        if !certificate.HasPrivateKey().unwrap_or(false) {
            continue;
        }
        let Ok(blob) = certificate.GetCertificateBlob() else { continue };
        let Ok(der) = bytes(&blob) else { continue };
        out.push((certificate, der));
    }
    Ok(out)
}

fn find(fingerprint: &[u8; 32]) -> Result<(Certificate, HeldCertificate), KeyHolderError> {
    let (certificate, der) = personal()?.into_iter().find(|(_, der)| sha256(der) == *fingerprint).ok_or(KeyHolderError::NotFound)?;
    Ok((certificate, HeldCertificate::read(der, now())?))
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
