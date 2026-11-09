// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `KEY`: certificates and OpenPGP keys read in every version's form, written back as 3.0, capped,
//! and never a panic.

use mailcraft_charset::base64::STANDARD;
use mailcraft_model::{Contact, ContactKey, KeyKind};
use proptest::prelude::*;

use crate::{parse, write};

/// Made-up bytes that look like a DER certificate (a SEQUENCE) and like an OpenPGP public key.
const DER: &[u8] = &[0x30, 0x82, 0x00, 0x05, 0x02, 0x01, 0x07, 0xa0, 0x00];
const PGP: &[u8] = &[0x99, 0x00, 0x0d, 0x04, 0x5e, 0x00, 0x00, 0x01, 0x16];

fn keys(version: &str, properties: &str) -> Vec<ContactKey> {
    let got = parse(&format!("BEGIN:VCARD\r\nVERSION:{version}\r\nFN:Ada\r\n{properties}\r\nEND:VCARD\r\n"));
    assert_eq!(got.len(), 1, "{properties}");
    got.into_iter().next().map(|c| c.keys).unwrap_or_default()
}

fn x509(data: &[u8]) -> ContactKey {
    ContactKey { kind: KeyKind::X509, data: data.to_vec(), ..Default::default() }
}

fn openpgp(data: &[u8]) -> ContactKey {
    ContactKey { kind: KeyKind::OpenPgp, data: data.to_vec(), ..Default::default() }
}

fn at(uri: &str, media_type: &str) -> ContactKey {
    ContactKey { kind: KeyKind::Uri, uri: uri.into(), media_type: media_type.into(), ..Default::default() }
}

#[test]
fn reads_2_1_keys() {
    let (der, pgp) = (STANDARD.encode(DER), STANDARD.encode(PGP));
    let (head, tail) = der.split_at(4);
    let got = keys("2.1", &format!("KEY;X509;ENCODING=BASE64:{head}\r\n  {tail}\r\n\r\nKEY;PGP;BASE64:{pgp}"));
    assert_eq!(got, [x509(DER), openpgp(PGP)]);
}

#[test]
fn reads_3_0_keys() {
    let (der, pgp) = (STANDARD.encode(DER), STANDARD.encode(PGP));
    let (head, tail) = pgp.split_at(5);
    let got = keys(
        "3.0",
        &format!(
            "KEY;ENCODING=b;TYPE=X509:{der}\r\nkey;type=pgp;encoding=B:{head}\r\n {tail}\r\nKEY;TYPE=PGP:-----BEGIN PGP PUBLIC KEY BLOCK-----\\n\\nmQIN\\n-----END PGP PUBLIC KEY BLOCK-----\r\nKEY;VALUE=uri;TYPE=X509:ldap://ldap.example.org/cn=Ada"
        ),
    );
    assert_eq!(
        got,
        [
            x509(DER),
            openpgp(PGP),
            openpgp(b"-----BEGIN PGP PUBLIC KEY BLOCK-----\n\nmQIN\n-----END PGP PUBLIC KEY BLOCK-----"),
            at("ldap://ldap.example.org/cn=Ada", ContactKey::X509_MEDIA_TYPE),
        ]
    );
}

#[test]
fn reads_4_0_keys() {
    let (der, pgp) = (STANDARD.encode(DER), STANDARD.encode(PGP));
    let got = keys(
        "4.0",
        &format!(
            "KEY:data:application/pgp-keys;base64,{pgp}\r\nKEY:data:application/pkix-cert;base64\\,{der}\r\nKEY:data:application/pgp-keys,%99%00%0D%04%5E%00%00%01%16\r\n\
KEY;MEDIATYPE=application/pgp-keys:https://keys.example.org/ada.asc\r\nKEY;TYPE=work:openpgp4fpr:ABCDEF0123456789ABCDEF0123456789ABCDEF01\r\nKEY:http://example.org/ada.cer"
        ),
    );
    assert_eq!(
        got,
        [
            openpgp(PGP),
            x509(DER),
            // (The same key twice is kept once.)
            at("https://keys.example.org/ada.asc", ContactKey::OPENPGP_MEDIA_TYPE),
            at("openpgp4fpr:ABCDEF0123456789ABCDEF0123456789ABCDEF01", ""),
            at("http://example.org/ada.cer", ""),
        ]
    );
}

#[test]
fn a_key_without_a_type_is_known_by_its_bytes() {
    let pem = format!("-----BEGIN CERTIFICATE-----\\n{}\\n-----END CERTIFICATE-----\\n", STANDARD.encode(DER));
    let got = keys(
        "3.0",
        &format!(
            "KEY;ENCODING=b:{}\r\nKEY;ENCODING=b:{}\r\nKEY:{pem}\r\nKEY:data:,{}\r\nKEY;ENCODING=b:{}",
            STANDARD.encode(DER),
            STANDARD.encode(PGP),
            "-----BEGIN PGP PUBLIC KEY BLOCK-----",
            STANDARD.encode(b"neither")
        ),
    );
    assert_eq!(got, [x509(DER), openpgp(PGP), openpgp(b"-----BEGIN PGP PUBLIC KEY BLOCK-----")]);
}

#[test]
fn what_is_not_a_key_is_dropped() {
    for junk in [
        "KEY;TYPE=PGP;ENCODING=b:!!!",
        "KEY;TYPE=PGP;ENCODING=b:",
        "KEY;TYPE=X509:",
        "KEY:javascript:alert(1)",
        "KEY:file:///etc/passwd",
        "KEY:cid:part1@example.org",
        "KEY;VALUE=uri:https://exa mple.org/key",
        "KEY:https:",
        "KEY:no scheme at all",
        "KEY:data:application/pgp-keys;base64",
        "KEY;TYPE=PEM:-----BEGIN CERTIFICATE-----\\n!!!\\n-----END CERTIFICATE-----",
        "KEY;TYPE=X-SOMETHING;ENCODING=b:AAEC",
    ] {
        assert!(keys("3.0", junk).is_empty(), "{junk}");
    }
    let long = format!("KEY:https://example.org/{}", "a".repeat(ContactKey::MAX_URI));
    assert!(keys("4.0", &long).is_empty());
    let mut big = DER.to_vec();
    big.resize(ContactKey::MAX_BYTES + 1, 0);
    assert!(keys("3.0", &format!("KEY;TYPE=X509;ENCODING=b:{}", STANDARD.encode(&big))).is_empty());
    big.truncate(ContactKey::MAX_BYTES);
    assert_eq!(keys("3.0", &format!("KEY;TYPE=X509;ENCODING=b:{}", STANDARD.encode(&big))), [x509(&big)]);
}

#[test]
fn keys_are_capped_per_card_and_per_text() {
    let many: String = (0..Contact::MAX_KEYS + 10)
        .map(|i| format!("KEY;TYPE=PGP;ENCODING=b:{}\r\n", STANDARD.encode([0x99, 0, u8::try_from(i).unwrap()])))
        .collect();
    assert_eq!(keys("3.0", &many).len(), Contact::MAX_KEYS);
    // What one text keeps of keys in all: the cards after the allowance is spent keep none.
    let card = |n: u8| format!("BEGIN:VCARD\r\nFN:C{n}\r\nKEY;TYPE=X509;ENCODING=b:{}\r\nEND:VCARD\r\n", STANDARD.encode([0x30, n, 0, 0]));
    let text: String = (0..5).map(card).collect();
    let got = crate::read::parse_keeping(&text, 10);
    assert_eq!(got.iter().map(|c| c.keys.len()).collect::<Vec<_>>(), [1, 1, 0, 0, 0]);
    assert_eq!(parse(&text).iter().map(|c| c.keys.len()).sum::<usize>(), 5);
}

#[test]
fn writes_keys_as_3_0() {
    let c = Contact {
        display_name: "Ada".into(),
        keys: vec![
            x509(DER),
            openpgp(PGP),
            at("https://keys.example.org/ada.asc", ContactKey::OPENPGP_MEDIA_TYPE),
            at("ldap://ldap.example.org/cn=Ada,o=Example", ContactKey::X509_MEDIA_TYPE),
            at("openpgp4fpr:ABCDEF01", ""),
            // Never written: a kind of a later version, nothing in it, an address that isn't one.
            ContactKey { kind: KeyKind::Unknown, data: DER.to_vec(), ..Default::default() },
            openpgp(&[]),
            at("javascript:alert(1)", ""),
            at("https://example.org/a\r\nTEL:666", ""),
        ],
        ..Default::default()
    };
    let out = write(&[&c]);
    let lines: Vec<&str> = out.split("\r\n").filter(|l| l.starts_with("KEY")).collect();
    assert_eq!(
        lines,
        [
            format!("KEY;TYPE=X509;ENCODING=b:{}", STANDARD.encode(DER)).as_str(),
            &format!("KEY;TYPE=PGP;ENCODING=b:{}", STANDARD.encode(PGP)),
            "KEY;VALUE=uri;TYPE=PGP:https://keys.example.org/ada.asc",
            "KEY;VALUE=uri;TYPE=X509:ldap://ldap.example.org/cn=Ada,o=Example",
            "KEY;VALUE=uri:openpgp4fpr:ABCDEF01",
        ]
    );
    assert!(!out.contains("TEL"));
    let back = parse(&out);
    assert_eq!(back[0].keys, c.keys[..5]);
}

#[test]
fn large_keys_fold_and_read_back() {
    let data: Vec<u8> = std::iter::once(0x30).chain((0..ContactKey::MAX_BYTES - 1).map(|i| (i % 251) as u8)).collect();
    let c = Contact { display_name: "Ada".into(), keys: vec![x509(&data), openpgp(&data[1..])], ..Default::default() };
    let out = write(&[&c]);
    assert!(out.split("\r\n").all(|line| line.len() <= 75));
    assert_eq!(parse(&out)[0].keys, c.keys);
    assert_eq!(crate::parse_bytes(out.as_bytes())[0].keys, c.keys);
}

fn key() -> impl Strategy<Value = ContactKey> {
    let bytes = proptest::collection::vec(any::<u8>(), 1..300);
    let uri = (proptest::sample::select(vec!["http", "https", "ldap", "ldaps", "openpgp4fpr"]), "[!-\\[\\]-~]{1,80}");
    let media = proptest::sample::select(vec!["", ContactKey::X509_MEDIA_TYPE, ContactKey::OPENPGP_MEDIA_TYPE]);
    prop_oneof![
        bytes.clone().prop_map(|data| ContactKey { kind: KeyKind::X509, data, ..Default::default() }),
        bytes.prop_map(|data| ContactKey { kind: KeyKind::OpenPgp, data, ..Default::default() }),
        (uri, media).prop_map(|((scheme, rest), media)| at(&format!("{scheme}:{rest}"), media)),
    ]
}

proptest! {
    #[test]
    fn any_keys_round_trip(list in proptest::collection::vec(key(), 0..6)) {
        let mut unique: Vec<ContactKey> = Vec::new();
        for k in list {
            if !unique.contains(&k) {
                unique.push(k);
            }
        }
        let c = Contact { display_name: "Ada".into(), keys: unique, ..Default::default() };
        let back = parse(&write(&[&c]));
        prop_assert_eq!(back.len(), 1);
        prop_assert_eq!(&back[0].keys, &c.keys);
    }

    #[test]
    fn arbitrary_key_values_never_panic(
        params in proptest::sample::select(vec!["", ";ENCODING=b", ";TYPE=PGP", ";TYPE=X509;ENCODING=BASE64", ";X509;QUOTED-PRINTABLE", ";MEDIATYPE=application/pgp-keys", ";VALUE=text", ";VALUE=uri"]),
        head in proptest::sample::select(vec!["", "data:", "data:application/pkix-cert;base64,", "data:,%", "-----BEGIN CERTIFICATE-----", "-----BEGIN PGP PUBLIC KEY BLOCK-----", "https:", "openpgp4fpr:"]),
        value in any::<String>(),
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let text = format!("BEGIN:VCARD\r\nFN:x\r\nKEY{params}:{head}{value}\r\nKEY{params}:{head}{}\r\nEND:VCARD\r\n", STANDARD.encode(&bytes));
        let got = parse(&text);
        for c in &got {
            prop_assert!(c.keys.iter().all(|k| k.data.len() <= ContactKey::MAX_BYTES && k.uri.len() <= ContactKey::MAX_URI));
            // What was read writes and reads back as itself.
            let again = parse(&write(&[c]));
            prop_assert_eq!(again.first().map(|a| &a.keys), Some(&c.keys));
        }
        let mut raw = format!("BEGIN:VCARD\r\nFN:x\r\nKEY{params}:{head}").into_bytes();
        raw.extend(&bytes);
        raw.extend(b"\r\nEND:VCARD\r\n");
        let _ = crate::parse_bytes(&raw);
    }
}
