// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account at Resend shows its API key to Resend's SMTP server, over TLS, and to no other.
use mailcraft_model::{Account, Security, Server};

fn resend(outgoing: Server) -> Account {
    Account {
        email: "avery@mail.example".into(),
        incoming: Some(Server { host: "api.resend.com".into(), port: 443, security: Security::Tls }),
        outgoing: Some(outgoing),
        ..Default::default()
    }
}

#[test]
fn a_resend_key_is_not_shown_to_another_smtp_server_or_in_the_clear() {
    // Nothing listens on these: the refusal comes before any connection is made.
    for (host, security) in [
        ("127.0.0.1", Security::Tls),
        ("smtp.resend.com.attacker.example", Security::Tls),
        ("smtp.example.org", Security::StartTls),
        ("smtp.resend.com", Security::None),
    ] {
        let account = resend(Server { host: host.into(), port: 465, security });
        let refused = crate::send::open_smtp(&account, "re_invented").err();
        assert!(matches!(refused, Some(crate::NetError::Config(_))), "{host}: {refused:?}");
    }
}
