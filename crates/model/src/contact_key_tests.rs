// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A contact's keys in the mailbox file: absent in old files, base64 in JSON, an unknown kind kept.

use crate::{Contact, ContactKey, KeyKind};

#[test]
fn a_contact_without_keys_reads_and_writes_as_before() {
    let old: Contact = serde_json::from_str(r#"{"id":3,"displayName":"Ada","emails":[{"label":"work","value":"ada@example.org"}]}"#).unwrap();
    assert!(old.keys.is_empty());
    assert!(!serde_json::to_string(&old).unwrap().contains("keys"));
}

#[test]
fn keys_round_trip_as_base64() {
    let c = Contact {
        display_name: "Ada".into(),
        keys: vec![
            ContactKey { kind: KeyKind::X509, data: vec![0x30, 0x82, 0x00, 0xff], ..Default::default() },
            ContactKey { kind: KeyKind::OpenPgp, data: b"-----BEGIN PGP PUBLIC KEY BLOCK-----".to_vec(), ..Default::default() },
            ContactKey {
                kind: KeyKind::Uri,
                uri: "https://keys.example.org/ada.asc".into(),
                media_type: ContactKey::OPENPGP_MEDIA_TYPE.into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let json = serde_json::to_string(&c).unwrap();
    assert!(json.contains(r#""data":"MIIA/w==""#), "{json}");
    assert!(json.contains(r#""kind":"openPgp""#) && json.contains(r#""kind":"x509""#) && json.contains(r#""kind":"uri""#), "{json}");
    assert_eq!(serde_json::from_str::<Contact>(&json).unwrap(), c);
}

#[test]
fn a_kind_from_a_later_version_is_unknown() {
    let c: Contact = serde_json::from_str(r#"{"keys":[{"kind":"somethingNew","data":"AAEC"},{}]}"#).unwrap();
    assert_eq!(c.keys[0], ContactKey { kind: KeyKind::Unknown, data: vec![0, 1, 2], ..Default::default() });
    assert_eq!(c.keys[1], ContactKey::default());
    assert!(serde_json::from_str::<Contact>(r#"{"keys":[{"data":"not base64!"}]}"#).is_err());
}
