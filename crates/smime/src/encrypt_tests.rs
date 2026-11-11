// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypting to S/MIME recipients (#30 part 7): with keys and example.org certificates made here,
//! each message read back by a decrypt written in this file (the crate has none: a private key is
//! used only by tests), and never a panic on any certificate. Every name is invented.

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};
use cms::content_info::ContentInfo;
use cms::enveloped_data::{EnvelopedData, RecipientIdentifier, RecipientInfo};
use der::{Decode, Encode};
use proptest::prelude::*;

use crate::sign_tests::{Authority, root, rsa_key, rsa_public};
use crate::{EncryptError, EncryptRequest, MAX_RECIPIENTS, can_encrypt_to, encrypt};

/// 2026-11-09.
const NOW: i64 = 1_794_182_400;
const CONTENT: &[u8] = b"Content-Type: text/plain; charset=utf-8\r\n\r\nThe minutes are attached.\r\n";

/// The system's randomness, as the host hands it in.
fn os(buf: &mut [u8]) -> bool {
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), buf).is_ok()
}

/// A certificate for `address` and `key` to encrypt mail to, issued by `by`; `usage` its key usage.
fn cert_with(address: &str, key: &impl rcgen::PublicKeyData, by: &Authority, usage: rcgen::KeyUsagePurpose, not_after: (i32, u8, u8)) -> Vec<u8> {
    let mut p = crate::sign_tests::params("Ada");
    p.not_after = rcgen::date_time_ymd(not_after.0, not_after.1, not_after.2);
    // A serial of its own (rcgen's comes from the key, which these tests share).
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1000);
    p.serial_number = Some(rcgen::SerialNumber::from(SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    p.subject_alt_names = vec![rcgen::SanType::Rfc822Name(address.try_into().unwrap())];
    p.key_usages = vec![usage];
    p.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::EmailProtection];
    p.signed_by(key, &rcgen::Issuer::new(by.params.clone(), &by.key)).unwrap().der().to_vec()
}

fn encryption_cert(address: &str, key: &rsa::PrivateKey, by: &Authority) -> Vec<u8> {
    cert_with(address, &rsa_public(key), by, rcgen::KeyUsagePurpose::KeyEncipherment, (2060, 1, 1))
}

fn request<'a>(recipients: &'a [Vec<u8>]) -> EncryptRequest<'a> {
    EncryptRequest { content: CONTENT, recipients, now: NOW }
}

/// What `key` (the private key of `cert`) reads out of `cms`: the test's own decrypt.
fn decrypt(cms: &[u8], cert: &[u8], key: &rsa::PrivateKey) -> Option<Vec<u8>> {
    let info = ContentInfo::from_der(cms).ok()?;
    assert_eq!(info.content_type.to_string(), "1.2.840.113549.1.7.3");
    let data = info.content.decode_as::<EnvelopedData>().ok()?;
    let mine = x509_cert::Certificate::from_der(cert).ok()?;
    let ours = data.recip_infos.0.iter().find_map(|r| match r {
        RecipientInfo::Ktri(k) => match &k.rid {
            RecipientIdentifier::IssuerAndSerialNumber(i)
                if i.issuer == mine.tbs_certificate().issuer().clone() && i.serial_number == mine.tbs_certificate().serial_number().clone() =>
            {
                Some(k)
            }
            _ => None,
        },
        _ => None,
    })?;
    let cek = key.decrypt(rsa::Pkcs1v15Encrypt, ours.enc_key.as_bytes()).ok()?;
    let alg = &data.encrypted_content.content_enc_alg;
    assert_eq!(alg.oid.to_string(), "2.16.840.1.101.3.4.1.42", "AES-256-CBC");
    let iv = alg.parameters.as_ref()?.decode_as::<der::asn1::OctetString>().ok()?;
    let decryptor = cbc::Decryptor::<aes::Aes256>::new_from_slices(&cek, iv.as_bytes()).ok()?;
    let mut buffer = data.encrypted_content.encrypted_content.as_ref()?.as_bytes().to_vec();
    let len = decryptor.decrypt_padded::<Pkcs7>(&mut buffer).ok()?.len();
    buffer.truncate(len);
    Some(buffer)
}

#[test]
fn every_recipient_reads_it_and_nobody_else_does() {
    let ca = root("Example Root");
    // (One 2048-bit key is made per test run; each certificate is named by its own serial number,
    // so a certificate that wasn't encrypted to finds nothing to open even with the same key.)
    let key = rsa_key(2048);
    let ada_cert = encryption_cert("ada@example.org", key, &ca);
    let kit_cert = encryption_cert("kit@example.org", key, &ca);
    let eve_cert = encryption_cert("eve@example.org", key, &ca);
    let recipients = vec![ada_cert.clone(), kit_cert.clone()];
    let cms = encrypt(&request(&recipients), &mut os).unwrap();
    assert_eq!(decrypt(&cms, &ada_cert, key).as_deref(), Some(CONTENT));
    assert_eq!(decrypt(&cms, &kit_cert, key).as_deref(), Some(CONTENT));
    assert_eq!(decrypt(&cms, &eve_cert, key), None);
}

#[test]
fn the_structure_is_what_mail_programs_read() {
    let ca = root("Example Root");
    let key = rsa_key(2048);
    let cert = encryption_cert("ada@example.org", key, &ca);
    let cms = encrypt(&request(&[cert]), &mut os).unwrap();
    let data = ContentInfo::from_der(&cms).unwrap().content.decode_as::<EnvelopedData>().unwrap();
    assert_eq!(data.version, cms::content_info::CmsVersion::V0);
    assert!(data.originator_info.is_none() && data.unprotected_attrs.is_none());
    assert_eq!(data.encrypted_content.content_type.to_string(), "1.2.840.113549.1.7.1");
    let alg = &data.encrypted_content.content_enc_alg;
    assert_eq!(alg.parameters.as_ref().unwrap().decode_as::<der::asn1::OctetString>().unwrap().as_bytes().len(), 16, "a 16-byte IV");
    let RecipientInfo::Ktri(info) = data.recip_infos.0.iter().next().unwrap() else { panic!("key transport") };
    assert_eq!(info.version, cms::content_info::CmsVersion::V0);
    assert_eq!(info.key_enc_alg.oid.to_string(), "1.2.840.113549.1.1.1", "rsaEncryption");
    assert_eq!(info.enc_key.as_bytes().len(), 256, "as long as the modulus");
    // DER, whole.
    assert_eq!(ContentInfo::from_der(&cms).unwrap().to_der().unwrap(), cms);
}

#[test]
fn nothing_is_the_same_twice() {
    let ca = root("Example Root");
    let cert = encryption_cert("ada@example.org", rsa_key(2048), &ca);
    let recipients = [cert];
    let (one, two) = (encrypt(&request(&recipients), &mut os).unwrap(), encrypt(&request(&recipients), &mut os).unwrap());
    assert_ne!(one, two, "a new key, IV and padding each time");
}

#[test]
fn the_same_recipient_twice_is_one() {
    let ca = root("Example Root");
    let cert = encryption_cert("ada@example.org", rsa_key(2048), &ca);
    let cms = encrypt(&request(&[cert.clone(), cert]), &mut os).unwrap();
    let data = ContentInfo::from_der(&cms).unwrap().content.decode_as::<EnvelopedData>().unwrap();
    assert_eq!(data.recip_infos.0.len(), 1);
}

#[test]
fn a_certificate_that_cant_be_encrypted_to_stops_it() {
    let ca = root("Example Root");
    let key = rsa_key(2048);
    let good = encryption_cert("ada@example.org", key, &ca);
    let signing_only = cert_with("ada@example.org", &rsa_public(key), &ca, rcgen::KeyUsagePurpose::DigitalSignature, (2060, 1, 1));
    let expired = cert_with("ada@example.org", &rsa_public(key), &ca, rcgen::KeyUsagePurpose::KeyEncipherment, (2021, 1, 1));
    let short = cert_with("ada@example.org", &rsa_public(rsa_key(1024)), &ca, rcgen::KeyUsagePurpose::KeyEncipherment, (2060, 1, 1));
    let ec = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let elliptic = cert_with("ada@example.org", &ec, &ca, rcgen::KeyUsagePurpose::KeyEncipherment, (2060, 1, 1));
    for bad in [signing_only, expired, short, elliptic, ca.der.clone(), vec![1, 2, 3], Vec::new()] {
        assert!(can_encrypt_to(&bad, NOW).is_err());
        // One bad recipient among good ones: nothing is made for the rest.
        assert!(matches!(encrypt(&request(&[good.clone(), bad]), &mut os), Err(EncryptError::Certificate(_))));
    }
    assert_eq!(can_encrypt_to(&good, NOW), Ok(()));
    // Not yet valid, and no longer.
    assert!(can_encrypt_to(&good, 1_000).is_err());
}

#[test]
fn nobody_and_too_many_and_nothing_are_refused() {
    let ca = root("Example Root");
    let cert = encryption_cert("ada@example.org", rsa_key(2048), &ca);
    assert_eq!(encrypt(&request(&[]), &mut os), Err(EncryptError::NoRecipients));
    let many = vec![cert.clone(); MAX_RECIPIENTS + 1];
    assert_eq!(encrypt(&request(&many), &mut os), Err(EncryptError::TooLarge));
    let empty = EncryptRequest { content: b"", recipients: &[cert], now: NOW };
    assert_eq!(encrypt(&empty, &mut os), Err(EncryptError::Encoding));
}

#[test]
fn a_source_of_random_numbers_that_fails_makes_nothing() {
    let ca = root("Example Root");
    let cert = encryption_cert("ada@example.org", rsa_key(2048), &ca);
    // Fails at once, and after the key and the IV (at the padding).
    for allowed in [0, 2] {
        let mut calls = 0;
        let mut flaky = |buf: &mut [u8]| {
            calls += 1;
            calls <= allowed && os(buf)
        };
        assert_eq!(encrypt(&request(std::slice::from_ref(&cert)), &mut flaky), Err(EncryptError::Random), "after {allowed}");
    }
}

#[test]
fn a_large_message_is_encrypted_whole() {
    let ca = root("Example Root");
    let key = rsa_key(2048);
    let cert = encryption_cert("ada@example.org", key, &ca);
    let content = vec![b'x'; 3 * 1024 * 1024 + 7];
    let recipients = [cert.clone()];
    let cms = encrypt(&EncryptRequest { content: &content, recipients: &recipients, now: NOW }, &mut os).unwrap();
    assert_eq!(decrypt(&cms, &cert, key), Some(content));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever bytes are given as a certificate, nothing panics and nothing is made.
    #[test]
    fn hostile_certificates_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let recipients = [bytes.clone()];
        prop_assert!(encrypt(&request(&recipients), &mut os).is_err());
        prop_assert!(can_encrypt_to(&bytes, NOW).is_err());
    }

    /// A real certificate with bytes changed or cut off is refused or still a certificate, never a panic.
    #[test]
    fn damaged_certificates_never_panic(at in 0usize..2000, byte in any::<u8>(), cut in 0usize..2000) {
        let ca = root("Example Root");
        let mut cert = encryption_cert("ada@example.org", rsa_key(2048), &ca);
        if let Some(b) = cert.get_mut(at) { *b = byte; }
        cert.truncate(cert.len().saturating_sub(cut % 8));
        let _ = encrypt(&request(&[cert.clone()]), &mut os);
        let _ = can_encrypt_to(&cert, NOW);
    }
}

/// Writes the certificates other crates' tests encrypt to (`testdata/*.enc.der`: RSA, key
/// encipherment, valid until 2060, for example.org-style addresses). Run on purpose:
/// `cargo test -p mailcraft-smime -- --ignored make_the_testdata`.
#[test]
#[ignore = "writes files: run on purpose"]
fn make_the_testdata() {
    let ca = root("Example Root");
    let key = rsa_key(2048);
    for (name, address) in [("maya", "maya@lumen.example"), ("owner", "ada@mail.example"), ("kit", "kit@lumen.example")] {
        let der = encryption_cert(address, key, &ca);
        std::fs::write(format!("{}/testdata/{name}.enc.der", env!("CARGO_MANIFEST_DIR")), der).unwrap();
    }
}
