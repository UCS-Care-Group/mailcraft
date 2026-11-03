// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading what a person is shown of a certificate: against certificates made at run time, and
//! against bytes that are no certificate.
#![allow(clippy::unwrap_used)]

use proptest::prelude::*;
use rcgen::{CertificateParams, DnType, KeyPair};

use super::*;

/// A self-signed certificate's DER with this subject and these dates.
pub(crate) fn make(names: &[&str], common: &str, organization: Option<&str>, from: (i32, u8, u8), to: (i32, u8, u8)) -> Vec<u8> {
    let mut params = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    params.distinguished_name.push(DnType::CommonName, common);
    if let Some(organization) = organization {
        params.distinguished_name.push(DnType::OrganizationName, organization);
    }
    params.not_before = rcgen::date_time_ymd(from.0, from.1, from.2);
    params.not_after = rcgen::date_time_ymd(to.0, to.1, to.2);
    let key = KeyPair::generate().unwrap();
    params.self_signed(&key).unwrap().der().to_vec()
}

#[test]
fn a_certificate_shows_its_fingerprint_names_and_dates() {
    let der = make(&["mail.example.org"], "mail.example.org", Some("Example Org"), (2026, 1, 2), (2027, 3, 4));
    let info = describe(&der).unwrap();
    assert_eq!(info.fingerprint.len(), 64);
    assert!(info.fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    assert_eq!(info.fingerprint, fingerprint(&der));
    // Self-signed: issued to and by the same.
    assert_eq!((info.subject.as_str(), info.issuer.as_str()), ("mail.example.org (Example Org)", "mail.example.org (Example Org)"));
    let seconds = |y, m, d| DateTime::new(Date::new(y, m, d).unwrap(), 0, 0, 0).unwrap().civil_seconds();
    assert_eq!((info.not_before, info.not_after), (seconds(2026, 1, 2), seconds(2027, 3, 4)));
    // Nothing of it in the debug output.
    assert!(!format!("{info:?}").contains("example"), "{info:?}");
}

#[test]
fn a_name_is_made_fit_to_show() {
    let der = make(&["x.example.org"], "B\u{fc}cher \u{202e}evil\u{7}\n", None, (2026, 1, 1), (2027, 1, 1));
    assert_eq!(describe(&der).unwrap().subject, "B\u{fc}cher evil");
    let der = make(&["x.example.org"], &"n".repeat(500), None, (2026, 1, 1), (2027, 1, 1));
    assert_eq!(describe(&der).unwrap().subject.chars().count(), MAX_NAME);
}

#[test]
fn fingerprints_parse_and_round_trip() {
    let der = make(&["x.example.org"], "x", None, (2026, 1, 1), (2027, 1, 1));
    let text = fingerprint(&der);
    assert_eq!(parse_fingerprint(&text), Some(sha256(&der)));
    assert_eq!(parse_fingerprint(&text.to_uppercase()), Some(sha256(&der)));
    for bad in ["", "abc", &text[1..], &format!("{text}0"), &format!("g{}", &text[1..]), &format!(" {}", &text[1..]), &"\u{e9}".repeat(32)] {
        assert_eq!(parse_fingerprint(bad), None, "{bad:?}");
    }
    // SHA-256 of "abc" (FIPS 180-4).
    assert_eq!(fingerprint(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn bytes_that_are_no_certificate_show_nothing() {
    let der = make(&["x.example.org"], "x", Some("o"), (2026, 1, 1), (2027, 1, 1));
    // Every cut and every byte broken: never a panic.
    for cut in 0..der.len() {
        let _ = describe(&der[..cut]);
    }
    for at in 0..der.len().min(400) {
        for byte in [0x00, 0x7f, 0x80, 0x81, 0x84, 0xff] {
            let mut broken = der.clone();
            broken[at] = byte;
            let _ = describe(&broken);
        }
    }
    for junk in [&b""[..], b"\x30", b"\x30\x80", b"\x30\x84\xff\xff\xff\xff", b"\x1f\x01\x00", b"not a certificate"] {
        assert_eq!(describe(junk), None);
    }
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let _ = describe(&bytes);
        let _ = parse_fingerprint(&String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn der_shaped_noise_never_panics(parts in proptest::collection::vec((prop_oneof![Just(0x30u8), Just(0x31), Just(0x06), Just(0x02), Just(0xa0), Just(0x17), Just(0x18), any::<u8>()], proptest::collection::vec(any::<u8>(), 0..12)), 0..30)) {
        let mut bytes = Vec::new();
        for (tag, contents) in parts {
            bytes.push(tag);
            bytes.push(u8::try_from(contents.len()).unwrap());
            bytes.extend(contents);
        }
        let _ = describe(&bytes);
        // Wrapped as the nested sequences of a certificate.
        for _ in 0..3 {
            let mut wrapped = vec![0x30, u8::try_from(bytes.len().min(120)).unwrap()];
            wrapped.extend(bytes.iter().take(120));
            bytes = wrapped;
            let _ = describe(&bytes);
        }
    }
}

fn signed_with(algorithm: &'static rcgen::SignatureAlgorithm) -> Vec<u8> {
    let params = CertificateParams::new(vec!["mail.example.org".to_string()]).unwrap();
    let key = KeyPair::generate_for(algorithm).unwrap();
    params.self_signed(&key).unwrap().der().to_vec()
}

#[test]
fn the_end_point_hash_follows_the_certificates_signature() {
    // RFC 5929 4.1: the hash of the signature algorithm, over the whole certificate.
    let p256 = signed_with(&rcgen::PKCS_ECDSA_P256_SHA256);
    assert_eq!(end_point_hash(&p256), Some(digest::digest(&digest::SHA256, &p256).as_ref().to_vec()));
    let p384 = signed_with(&rcgen::PKCS_ECDSA_P384_SHA384);
    let hash = end_point_hash(&p384).unwrap();
    assert_eq!(hash.len(), 48);
    assert_eq!(hash, digest::digest(&digest::SHA384, &p384).as_ref().to_vec());
    // A signature with no hash of its own to name gets none, and so does anything that isn't a certificate.
    assert_eq!(end_point_hash(&signed_with(&rcgen::PKCS_ED25519)), None);
    assert_eq!(end_point_hash(&[]), None);
    assert_eq!(end_point_hash(&[0x30, 0x03, 0x30, 0x01, 0x00]), None);
}

proptest! {
    #[test]
    fn the_end_point_hash_of_noise_is_no_panic(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = end_point_hash(&bytes);
    }
}
