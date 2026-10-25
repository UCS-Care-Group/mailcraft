// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Password negotiation through the shared fake, including a real STARTTLS handshake.
use crate::fake::{Fake, ImapWorld};
use mailcraft_model::{Account, Security, Server};
fn account(port: u16, security: Security) -> Account {
    Account { email: "user@example.org".into(), incoming: Some(Server { host: "127.0.0.1".into(), port, security }), ..Default::default() }
}
#[test]
fn sync_downloads_through_plain_with_and_without_an_initial_response() {
    for sasl_ir in [false, true] {
        let peer = Fake::imap(ImapWorld { user: "user@example.org".into(), password: "invented".into(), plain: true, sasl_ir, ..Default::default() });
        let result = crate::sync(&mailcraft_model::sync::SyncRequest {
            account: account(peer.port, Security::None),
            password: "invented".into(),
            ..Default::default()
        });
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        let log = &peer.world().log;
        assert!(log.iter().any(|s| s.contains(" AUTHENTICATE PLAIN")));
        assert!(!log.iter().any(|s| s.contains(" CAPABILITY") || s.contains(" LOGIN ")));
        let payload = mailcraft_charset::base64::STANDARD.encode(b"\0user@example.org\0invented");
        assert!(log.iter().any(|s| if sasl_ir { s.ends_with(&format!("PLAIN {payload}")) } else { s == &payload }));
    }
}
#[test]
fn starttls_discards_the_greeting_capabilities_and_authenticates_inside_tls() {
    for sasl_ir in [false, true] {
        crate::tls_test_support::with_tls(|config| {
            let peer = Fake::imap(ImapWorld {
                user: "user@example.org".into(),
                password: "invented".into(),
                plain: true,
                sasl_ir,
                offer_starttls: true,
                starttls_upgrade: Some(std::sync::Arc::new(move |stream| -> Box<dyn crate::fake::MailIo> {
                    let connection = rustls::ServerConnection::new(config.clone()).unwrap();
                    Box::new(rustls::StreamOwned::new(connection, stream))
                })),
                ..Default::default()
            });
            let result = crate::sync(&mailcraft_model::sync::SyncRequest {
                account: account(peer.port, Security::StartTls),
                password: "invented".into(),
                ..Default::default()
            });
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            let log = &peer.world().log;
            let tls = log.iter().position(|s| s.contains(" STARTTLS")).unwrap();
            let auth = log.iter().position(|s| s.contains(" AUTHENTICATE PLAIN")).unwrap();
            assert!(log.iter().skip(tls + 1).take(auth - tls - 1).any(|s| s.contains(" CAPABILITY")));
            assert_eq!(log.iter().filter(|s| s.contains(" CAPABILITY")).count(), 2);
            assert!(!log.iter().any(|s| s.contains(" LOGIN ")));
        });
    }
}
#[test]
fn a_missing_password_is_explained_before_connecting() {
    let peer = Fake::imap(ImapWorld::default());
    let error = crate::sync::open_imap(&account(peer.port, Security::None), "").unwrap_err();
    assert!(error.to_string().contains("enter a password"));
    assert_eq!(peer.world().connections, 0);
}
