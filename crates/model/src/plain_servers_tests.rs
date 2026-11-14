// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #836: an outgoing server may be a relay that takes mail without a sign-in.

use crate::*;

#[test]
fn a_relay_without_a_sign_in_is_kept_and_a_file_from_before_reads_as_signing_in() {
    let relay = Account {
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "imap.example.org".into(), port: 993, security: Security::Tls }),
        outgoing: Some(Server { host: "relay.example.org".into(), port: 25, security: Security::StartTls }),
        outgoing_without_sign_in: true,
        ..Default::default()
    };
    let written = serde_json::to_value(&relay).unwrap();
    assert_eq!(written["outgoingWithoutSignIn"], true);
    assert_eq!(serde_json::from_value::<Account>(written).unwrap(), relay);
    // Off is not written, and a file without it, or with something odd in it, signs in.
    let before = Account { outgoing_without_sign_in: false, ..relay };
    let written = serde_json::to_value(&before).unwrap();
    assert!(written.get("outgoingWithoutSignIn").is_none());
    let mut odd = written.clone();
    odd["outgoingWithoutSignIn"] = serde_json::json!("yes");
    for v in [written, odd] {
        assert!(!serde_json::from_value::<Account>(v).unwrap().outgoing_without_sign_in);
    }
}
