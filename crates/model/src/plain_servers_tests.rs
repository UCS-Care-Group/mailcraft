// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #836: no encryption is for a server on this computer alone (a mail bridge on 127.0.0.1), and
//! an outgoing server may be a relay that takes mail without a sign-in.

use crate::*;

#[test]
fn this_computer_is_localhost_or_a_loopback_address_and_nothing_else() {
    for yes in ["localhost", "LocalHost", "localhost.", " localhost ", "127.0.0.1", "127.8.9.10", "::1", "[::1]", "0:0:0:0:0:0:0:1"] {
        assert!(is_loopback_host(yes), "{yes}");
    }
    for no in [
        "",
        "mail.example.org",
        "localhost.example.org",
        "mylocalhost",
        "192.0.2.7",
        "10.0.0.1",
        "::",
        "[::2]",
        "[::1",
        "127.0.0.1.example.org",
        "0.0.0.0",
    ] {
        assert!(!is_loopback_host(no), "{no}");
    }
}

#[test]
fn only_an_unencrypted_server_on_the_network_is_plain_over_it() {
    let server = |host: &str, security| Server { host: host.into(), port: 0, security };
    assert!(server("mail.example.org", Security::None).plain_over_the_network());
    assert!(server("192.0.2.25", Security::None).plain_over_the_network());
    for fine in [
        server("127.0.0.1", Security::None),
        server("[::1]", Security::None),
        server("mail.example.org", Security::StartTls),
        server("mail.example.org", Security::Tls),
    ] {
        assert!(!fine.plain_over_the_network(), "{fine:?}");
    }
    assert!(server("localhost", Security::None).on_this_computer());
}

#[test]
fn a_relay_without_a_sign_in_is_kept_and_a_file_from_before_reads_as_signing_in() {
    let relay = Account {
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "127.0.0.1".into(), port: 1143, security: Security::None }),
        outgoing: Some(Server { host: "relay.example.org".into(), port: 25, security: Security::StartTls }),
        outgoing_without_sign_in: true,
        ..Default::default()
    };
    let written = serde_json::to_value(&relay).unwrap();
    assert_eq!(written["outgoingWithoutSignIn"], true);
    assert_eq!(written["incoming"]["security"], "none");
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
