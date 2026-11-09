// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The key holders' logic, with synthetic example.org certificates made here (their signatures
//! are filler: nothing here checks one). The systems' stores themselves are reached only on the
//! systems; PKCS#11 has an opt-in test against a module the tester names.

use std::str::FromStr;

use x509_cert::der::asn1::{BitString, Ia5String, ObjectIdentifier, OctetString};
use x509_cert::der::{Any, Decode, Encode, Reader, SliceReader, Tag};
use x509_cert::ext::Extension;
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{ExtendedKeyUsage, KeyUsage, KeyUsages, SubjectAltName};
use x509_cert::name::Name;
use x509_cert::spki::{AlgorithmIdentifierOwned, SubjectPublicKeyInfoOwned};

use super::certificate::{hex, sha256};
use super::encoding::{digest_info, ecdsa_raw_to_der};
use super::{DigestAlgorithm, HeldCertificate, KeyHolderError, KeyKind, MAX_CERTIFICATE_LEN, Pin, SignatureScheme, usable};

const RSA: &str = "1.2.840.113549.1.1.1";
const EC: &str = "1.2.840.10045.2.1";
const P256: &str = "1.2.840.10045.3.1.7";
const P384: &str = "1.3.132.0.34";
const EMAIL_PROTECTION: &str = "1.3.6.1.5.5.7.3.4";
const SERVER_AUTH: &str = "1.3.6.1.5.5.7.3.1";
const NOT_BEFORE: u64 = 1_767_225_600; // 2026-01-01
const NOT_AFTER: u64 = 1_798_761_600; // 2027-01-01

fn oid(text: &str) -> ObjectIdentifier {
    ObjectIdentifier::new(text).unwrap()
}

/// An `RSAPublicKey` with a modulus of `bits` bits (top bit set) and e = 65537.
fn rsa_public_key(bits: usize) -> Vec<u8> {
    let mut n = vec![0x00, 0x80];
    n.resize(bits / 8 + 1, 0x01);
    let mut body = Vec::new();
    body.extend(der_tlv(0x02, &n));
    body.extend(der_tlv(0x02, &[0x01, 0x00, 0x01]));
    der_tlv(0x30, &body)
}

fn der_tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = value.len();
    if len < 0x80 {
        out.push(len as u8);
    } else if len < 0x100 {
        out.extend([0x81, len as u8]);
    } else {
        out.extend([0x82, (len >> 8) as u8, len as u8]);
    }
    out.extend_from_slice(value);
    out
}

struct Spec {
    subject: &'static str,
    key: SubjectPublicKeyInfoOwned,
    extensions: Vec<Extension>,
}

fn rsa_key() -> SubjectPublicKeyInfoOwned {
    SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned { oid: oid(RSA), parameters: Some(Any::new(Tag::Null, Vec::new()).unwrap()) },
        subject_public_key: BitString::from_bytes(&rsa_public_key(2048)).unwrap(),
    }
}

fn ec_key(curve: &str) -> SubjectPublicKeyInfoOwned {
    let mut point = vec![0x04];
    point.extend([0x11; 64]);
    SubjectPublicKeyInfoOwned {
        algorithm: AlgorithmIdentifierOwned { oid: oid(EC), parameters: Some(Any::encode_from(&oid(curve)).unwrap()) },
        subject_public_key: BitString::from_bytes(&point).unwrap(),
    }
}

fn extension<T: Encode>(id: &str, critical: bool, value: &T) -> Extension {
    Extension { extn_id: oid(id), critical, extn_value: OctetString::new(value.to_der().unwrap()).unwrap() }
}

fn san(addresses: &[&str]) -> Extension {
    let names = addresses.iter().map(|a| GeneralName::Rfc822Name(Ia5String::new(a).unwrap())).collect();
    extension("2.5.29.17", false, &SubjectAltName(names))
}

fn eku(usages: &[&str]) -> Extension {
    extension("2.5.29.37", false, &ExtendedKeyUsage(usages.iter().map(|u| oid(u)).collect()))
}

fn key_usage(usages: x509_cert::der::flagset::FlagSet<KeyUsages>) -> Extension {
    extension("2.5.29.15", true, &KeyUsage(usages))
}

/// A certificate as RFC 5280 §4.1 lays it out, put together by hand (x509-cert keeps its fields
/// to itself); its signature is filler.
fn certificate(spec: Spec) -> Vec<u8> {
    let algorithm = AlgorithmIdentifierOwned { oid: oid("1.2.840.113549.1.1.11"), parameters: None }.to_der().unwrap();
    let mut validity = Vec::new();
    validity.extend(der_tlv(0x17, b"260101000000Z"));
    validity.extend(der_tlv(0x17, b"270101000000Z"));
    let mut tbs = Vec::new();
    tbs.extend([0xa0, 0x03, 0x02, 0x01, 0x02]); // version v3
    tbs.extend([0x02, 0x02, 0x01, 0x02]); // serial number
    tbs.extend(&algorithm);
    tbs.extend(Name::from_str("CN=Example Test CA,O=Example").unwrap().to_der().unwrap());
    tbs.extend(der_tlv(0x30, &validity));
    tbs.extend(Name::from_str(spec.subject).unwrap().to_der().unwrap());
    tbs.extend(spec.key.to_der().unwrap());
    if !spec.extensions.is_empty() {
        tbs.extend(der_tlv(0xa3, &spec.extensions.to_der().unwrap()));
    }
    let mut cert = der_tlv(0x30, &tbs);
    cert.extend(&algorithm);
    cert.extend(BitString::from_bytes(&[0x5a; 16]).unwrap().to_der().unwrap());
    der_tlv(0x30, &cert)
}

fn alice_rsa() -> Vec<u8> {
    certificate(Spec {
        subject: "CN=Alice Example,O=Example",
        key: rsa_key(),
        extensions: vec![san(&["Alice@Example.org"]), eku(&[EMAIL_PROTECTION]), key_usage(KeyUsages::DigitalSignature | KeyUsages::KeyEncipherment)],
    })
}

#[test]
fn an_rsa_certificate_for_mail_is_read_whole() {
    let der = alice_rsa();
    let held = HeldCertificate::from_der(der.clone()).unwrap();
    assert_eq!(held.fingerprint, sha256(&der));
    assert_eq!(held.fingerprint_hex().len(), 64);
    assert!(held.subject.contains("CN=Alice Example"), "{}", held.subject);
    assert!(held.issuer.contains("CN=Example Test CA"), "{}", held.issuer);
    assert_eq!(held.emails, vec!["alice@example.org".to_string()]);
    assert_eq!((held.not_before, held.not_after), (NOT_BEFORE, NOT_AFTER));
    assert_eq!(held.key, KeyKind::Rsa { bits: 2048 });
    assert!(held.for_mail && held.can_sign && held.can_decrypt);
    assert_eq!(held.der, der);
}

#[test]
fn an_ec_certificate_with_no_extensions_takes_the_subjects_address() {
    let der = certificate(Spec { subject: "CN=Bob Example,emailAddress=bob@example.org", key: ec_key(P256), extensions: Vec::new() });
    let held = HeldCertificate::from_der(der).unwrap();
    assert_eq!(held.emails, vec!["bob@example.org".to_string()]);
    assert_eq!(held.key, KeyKind::EcP256);
    // No usages named: any purpose, but an EC key unwraps nothing here (ECDH is not RSA's job).
    assert!(held.for_mail && held.can_sign && !held.can_decrypt);

    let der = certificate(Spec { subject: "CN=Carol Example", key: ec_key(P384), extensions: vec![san(&["carol@example.org"])] });
    assert_eq!(HeldCertificate::from_der(der).unwrap().key, KeyKind::EcP384);
}

#[test]
fn key_usage_decides_signing_and_unwrapping() {
    let sign_only = certificate(Spec { subject: "CN=Dan", key: rsa_key(), extensions: vec![key_usage(KeyUsages::NonRepudiation.into())] });
    let held = HeldCertificate::from_der(sign_only).unwrap();
    assert!(held.can_sign && !held.can_decrypt);
    let unwrap_only = certificate(Spec { subject: "CN=Dan", key: rsa_key(), extensions: vec![key_usage(KeyUsages::KeyEncipherment.into())] });
    let held = HeldCertificate::from_der(unwrap_only).unwrap();
    assert!(!held.can_sign && held.can_decrypt);
}

#[test]
fn only_certificates_for_mail_are_listed_each_once() {
    let web = certificate(Spec { subject: "CN=www.example.org", key: rsa_key(), extensions: vec![eku(&[SERVER_AUTH])] });
    assert!(!HeldCertificate::from_der(web.clone()).unwrap().for_mail);
    let neither = certificate(Spec { subject: "CN=Eve", key: rsa_key(), extensions: vec![key_usage(KeyUsages::KeyCertSign.into())] });
    let listed = usable([alice_rsa(), web, neither, alice_rsa(), b"not a certificate".to_vec()]);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed.first().map(|h| h.emails.clone()), Some(vec!["alice@example.org".to_string()]));
}

#[test]
fn addresses_that_cant_be_addresses_are_left_out() {
    let der = certificate(Spec {
        subject: "CN=Frank",
        key: rsa_key(),
        extensions: vec![san(&["no-at-sign", "frank@example.org", "FRANK@example.org", "two words@example.org"])],
    });
    assert_eq!(HeldCertificate::from_der(der).unwrap().emails, vec!["frank@example.org".to_string()]);
}

#[test]
fn hostile_certificates_never_panic() {
    let der = alice_rsa();
    // Every truncation, and every byte flipped in turn.
    for end in 0..der.len() {
        let _ = HeldCertificate::from_der(der.get(..end).unwrap().to_vec());
    }
    for i in 0..der.len() {
        for flip in [0x01u8, 0x80, 0xff] {
            let mut bad = der.clone();
            if let Some(b) = bad.get_mut(i) {
                *b ^= flip;
            }
            let _ = HeldCertificate::from_der(bad);
        }
    }
    let _ = usable([Vec::new(), vec![0x30], vec![0x30, 0x84, 0xff, 0xff, 0xff, 0xff]]);
}

#[test]
fn an_oversized_certificate_is_refused_unread() {
    assert_eq!(HeldCertificate::from_der(vec![0x30; MAX_CERTIFICATE_LEN + 1]), Err(KeyHolderError::BadInput("the certificate is too large")));
}

#[test]
fn digest_info_is_rfc_8017s() {
    let digest = [0xab; 32];
    let info = digest_info(DigestAlgorithm::Sha256, &digest).unwrap();
    assert_eq!(hex(info.get(..19).unwrap()), "3031300d060960864801650304020105000420");
    assert_eq!(info.get(19..), Some(&digest[..]));
    assert_eq!(hex(digest_info(DigestAlgorithm::Sha384, &[0; 48]).unwrap().get(..19).unwrap()), "3041300d060960864801650304020205000430");
    assert_eq!(hex(digest_info(DigestAlgorithm::Sha512, &[0; 64]).unwrap().get(..19).unwrap()), "3051300d060960864801650304020305000440");
    assert!(digest_info(DigestAlgorithm::Sha256, &[0; 48]).is_err());
    assert_eq!(digest_info(DigestAlgorithm::Sha512, &[0; 64]).unwrap().len(), 83);
}

#[test]
fn digests_must_be_their_hashs_length() {
    assert!(DigestAlgorithm::Sha256.check(&[0; 32]).is_ok());
    assert!(DigestAlgorithm::Sha256.check(&[0; 20]).is_err());
    assert!(DigestAlgorithm::Sha384.check(&[0; 48]).is_ok());
    assert!(DigestAlgorithm::Sha512.check(&[]).is_err());
}

/// The two INTEGERs of a DER `Ecdsa-Sig-Value`, as minimal unsigned bytes.
fn ecdsa_parts(der: &[u8]) -> (Vec<u8>, Vec<u8>) {
    use x509_cert::der::asn1::UintRef;
    let mut reader = SliceReader::new(der).unwrap();
    let parts = reader
        .sequence(|seq| Ok::<_, x509_cert::der::Error>((UintRef::decode(seq)?.as_bytes().to_vec(), UintRef::decode(seq)?.as_bytes().to_vec())))
        .unwrap();
    reader.finish().unwrap();
    parts
}

#[test]
fn ecdsa_r_s_becomes_der() {
    // r = 1, s with its top bit set: a leading zero for s, none for r.
    let mut raw = vec![0u8; 31];
    raw.push(0x01);
    raw.extend([0xff; 32]);
    let der = ecdsa_raw_to_der(&raw).unwrap();
    assert_eq!(hex(der.get(..7).unwrap()), "30260201010221");
    assert_eq!(ecdsa_parts(&der), (vec![0x01], vec![0xff; 32]));

    // Zero is one zero byte.
    assert_eq!(hex(&ecdsa_raw_to_der(&[0u8; 64]).unwrap()), "3006020100020100");

    // P-384 and P-521 sizes, the latter needing a long length.
    for half in [48usize, 66] {
        let raw = vec![0x80; half * 2];
        let der = ecdsa_raw_to_der(&raw).unwrap();
        assert_eq!(ecdsa_parts(&der), (vec![0x80; half], vec![0x80; half]));
    }
    assert_eq!(ecdsa_raw_to_der(&[0x80; 132]).unwrap().get(..2), Some(&[0x30, 0x81][..]));

    assert!(ecdsa_raw_to_der(&[]).is_err());
    assert!(ecdsa_raw_to_der(&[1, 2, 3]).is_err());
    assert!(ecdsa_raw_to_der(&[1; 134]).is_err());
}

#[test]
fn schemes_fit_their_keys() {
    let rsa = KeyKind::Rsa { bits: 2048 };
    assert!(SignatureScheme::RsaPkcs1v15.fits(rsa) && SignatureScheme::RsaPss.fits(rsa) && !SignatureScheme::Ecdsa.fits(rsa));
    assert!(SignatureScheme::Ecdsa.fits(KeyKind::EcP256) && SignatureScheme::Ecdsa.fits(KeyKind::EcP384));
    assert!(!SignatureScheme::RsaPss.fits(KeyKind::EcP256) && !SignatureScheme::Ecdsa.fits(KeyKind::Other));
}

#[test]
fn a_pin_is_never_shown() {
    let pin = Pin::new("246813".to_string());
    assert_eq!(format!("{pin:?}"), "Pin(…)");
    assert_eq!(format!("{:?}", Some(&pin)), "Some(Pin(…))");
    assert_eq!(pin.expose(), "246813");
    let error = KeyHolderError::PinNeeded { token: "Example Card".into() };
    assert!(!error.to_string().contains("246813"));
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod pkcs11 {
    use std::path::Path;

    use cryptoki::context::Function;
    use cryptoki::error::{Error, RvError};

    use super::super::KeyHolderError;
    use super::super::pkcs11::{Pkcs11Holder, token_error};

    #[test]
    fn a_module_must_be_named_by_an_absolute_path_that_loads() {
        assert!(matches!(Pkcs11Holder::open(Path::new("p11-kit-proxy.so")), Err(KeyHolderError::Unavailable(_))));
        assert!(matches!(Pkcs11Holder::open(Path::new("/nonexistent/example/module.so")), Err(KeyHolderError::Unavailable(_))));
    }

    #[test]
    fn token_errors_are_named() {
        assert_eq!(token_error(Error::Pkcs11(RvError::PinIncorrect, Function::Login)), KeyHolderError::PinIncorrect);
        assert_eq!(token_error(Error::Pkcs11(RvError::PinLocked, Function::Login)), KeyHolderError::PinLocked);
        assert_eq!(token_error(Error::Pkcs11(RvError::FunctionCanceled, Function::Sign)), KeyHolderError::Cancelled);
        assert!(matches!(token_error(Error::Pkcs11(RvError::MechanismInvalid, Function::Sign)), KeyHolderError::Unsupported(_)));
        assert!(matches!(token_error(Error::Pkcs11(RvError::DeviceError, Function::Sign)), KeyHolderError::System(_)));
    }

    /// Opt-in: `MAILCRAFT_PKCS11_TEST_MODULE=/usr/lib/softhsm/libsofthsm2.so cargo test -p mailcraft
    /// -- --ignored key_holders` lists what the named module's tokens hold. Use a test token
    /// (SoftHSM2, or p11-kit's trust module, which has no keys), never a card in daily use.
    #[test]
    #[ignore = "needs a PKCS#11 module named in MAILCRAFT_PKCS11_TEST_MODULE"]
    fn a_named_module_lists_its_certificates() {
        let Some(module) = std::env::var_os("MAILCRAFT_PKCS11_TEST_MODULE") else { return };
        let holder = Pkcs11Holder::open(Path::new(&module)).unwrap();
        let listed = holder.certificates().unwrap();
        for held in &listed {
            assert!(held.certificate.for_mail);
        }
        // An unknown fingerprint is not found, and asks for no PIN.
        let missing = holder.sign_digest(&[0; 32], super::super::SignatureScheme::RsaPkcs1v15, super::super::DigestAlgorithm::Sha256, &[0; 32], None);
        assert_eq!(missing, Err(KeyHolderError::NotFound));
    }
}
