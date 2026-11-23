// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Separate GOA mail credentials and hosts that cannot resolve GOA.

use crate::fake::{Fake, ImapWorld, SmtpWorld};
use mailcraft_model::sync::SyncRequest;
use mailcraft_model::{Account, AccountKind, Address, GoaAccount, Message, Security, Server};

#[test]
fn goa_outgoing_credentials_are_separate_and_redacted_in_every_piece() {
    for (accepted, auth) in [(true, true), (false, true), (false, false)] {
        let incoming = Fake::imap(ImapWorld { user: "incoming".into(), password: "synthetic-incoming".into(), ..Default::default() });
        let outgoing = Fake::smtp(SmtpWorld {
            user: "outgoing".into(),
            password: if accepted { "synthetic-outgoing" } else { "different" }.into(),
            auth,
            ..Default::default()
        });
        let server = |port| Some(Server { host: "127.0.0.1".into(), port, security: Security::None });
        let request = SyncRequest {
            account: Account {
                id: 7,
                kind: AccountKind::Imap,
                email: "owner@example.org".into(),
                username: "incoming".into(),
                incoming: server(incoming.port),
                outgoing: server(outgoing.port),
                ..Default::default()
            },
            password: "synthetic-incoming".into(),
            smtp_password: Some("synthetic-outgoing".into()),
            smtp_username: Some("outgoing".into()),
            outbox: vec![Message {
                id: 9,
                to: vec![Address::new("Receiver", "receiver@example.org")],
                subject: "Synthetic mail".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut pieces = Vec::new();
        crate::sync_with(&request, &std::sync::atomic::AtomicBool::new(false), &mut |piece| pieces.push(piece));
        assert_eq!(outgoing.world().delivered.len(), usize::from(accepted));
        assert!(incoming.world().log.iter().any(|line| line.contains(" LOGIN ")));
        for piece in pieces {
            let serialized = serde_json::to_string(&piece).unwrap();
            assert!(!serialized.contains("synthetic-outgoing"), "{serialized}");
            assert!(!serialized.contains("synthetic-incoming"));
        }
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(!serialized.contains("synthetic-outgoing") && !serialized.contains("synthetic-incoming"));
        assert!(!format!("{request:?}").contains("synthetic-outgoing"));
    }
}

#[test]
fn goa_unresolved_accounts_never_connect_or_accept_an_injected_password() {
    let incoming = Fake::imap(ImapWorld::default());
    let outgoing = Fake::smtp(SmtpWorld::default());
    let server = |port| Some(Server { host: "127.0.0.1".into(), port, security: Security::None });
    let account = Account {
        email: "owner@example.org".into(),
        incoming: server(incoming.port),
        outgoing: server(outgoing.port),
        goa: Some(GoaAccount { id: "account_1".into(), provider: "google".into() }),
        ..Default::default()
    };
    let result = crate::sync(&SyncRequest { account: account.clone(), password: "injected".into(), ..Default::default() });
    assert!(!result.errors.is_empty());
    assert!(crate::test_account(&account, "injected").is_err());
    let message = Message { to: vec![Address::new("Receiver", "receiver@example.org")], ..Default::default() };
    assert!(crate::send(&account, "injected", &message).is_err());
    assert_eq!(incoming.world().connections, 0);
    assert_eq!(outgoing.world().connections, 0);
}

#[test]
fn overlapping_credentials_are_redacted_in_the_original_text() {
    for secrets in [["αβ", "αβγ", "γδ"], ["γδ", "αβγ", "αβ"]] {
        assert_eq!(crate::scrub_secrets("before αβγδ after αβγ", &secrets), "before *** after ***");
    }
    assert_eq!(crate::scrub_secrets("ababa", &["aba", "", "b"]), "***");
    assert_eq!(crate::scrub_secrets("abc abc", &["", "b", "abc"]), "*** ***");
}
