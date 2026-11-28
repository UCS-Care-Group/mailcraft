// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A delivery receipt on the fake SMTP server: requested when EHLO offers DSN, and the message
//! still goes, with the owner told, when it does not.

use mailcraft_model::sync::{FolderState, SyncRequest};
use mailcraft_model::{Account, AccountKind, Address, FolderKind, Message, Security, Server};

use crate::fake::{Fake, SmtpWorld};
use crate::sync::sync;

const PASSWORD: &str = "hunter2";

fn world(offer_dsn: bool) -> SmtpWorld {
    SmtpWorld { user: "ada".into(), password: PASSWORD.into(), auth: true, offer_dsn, ..Default::default() }
}

fn going(delivery_receipt: bool) -> Message {
    Message {
        id: 9,
        to: vec![Address::new("Bob", "bob@example.org")],
        cc: vec![Address::new("Carol", "carol@example.org")],
        subject: "Hello".into(),
        date: 1_057_049_557,
        delivery_receipt,
        ..Default::default()
    }
}

fn sent(offer_dsn: bool, message: Message) -> (mailcraft_model::sync::SyncResult, Vec<String>) {
    let smtp = Fake::smtp(world(offer_dsn));
    let account = Account {
        id: 7,
        name: "Ada".into(),
        email: "ada@example.org".into(),
        display_name: "Ada Lovelace".into(),
        kind: AccountKind::Imap,
        outgoing: Some(Server { host: "127.0.0.1".into(), port: smtp.port, security: Security::None }),
        username: "ada".into(),
        ..Default::default()
    };
    let folders = FolderKind::STANDARD.iter().zip(100..).map(|(kind, id)| FolderState { id, kind: *kind, ..Default::default() }).collect();
    let request = SyncRequest { account, password: PASSWORD.into(), folders, outbox: vec![message], ..Default::default() };
    let result = sync(&request);
    let log = smtp.world().log.clone();
    (result, log)
}

#[test]
fn a_server_that_offers_dsn_is_asked_on_mail_from_and_each_rcpt_to() {
    let (result, log) = sent(true, going(true));
    assert_eq!(result.sent, [9]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let mail = log.iter().find(|line| line.starts_with("MAIL FROM:")).unwrap();
    assert!(mail.contains("RET=HDRS") && mail.contains("<ada@example.org>"), "{mail}");
    let rcpt: Vec<_> = log.iter().filter(|line| line.starts_with("RCPT TO:")).collect();
    assert_eq!(rcpt.len(), 2, "{log:?}");
    assert!(rcpt.iter().all(|line| line.contains("NOTIFY=SUCCESS,FAILURE")), "{rcpt:?}");
    assert!(rcpt.iter().any(|line| line.contains("<bob@example.org>")));
    assert!(rcpt.iter().any(|line| line.contains("<carol@example.org>")));
}

#[test]
fn a_server_that_does_not_offer_dsn_still_gets_the_message_and_the_owner_is_told() {
    let (result, log) = sent(false, going(true));
    assert_eq!(result.sent, [9], "the send is not a failure");
    assert_eq!(result.errors, ["Sent \"Hello\" without a delivery receipt: the outgoing server doesn't offer them"]);
    assert!(log.iter().any(|line| line.starts_with("MAIL FROM:") && !line.contains("RET=")), "{log:?}");
    assert!(log.iter().any(|line| line.starts_with("RCPT TO:") && !line.contains("NOTIFY=")), "{log:?}");
    assert!(!log.iter().any(|line| line.contains("RET=") || line.contains("NOTIFY=")), "{log:?}");
}

#[test]
fn a_message_that_asks_for_none_sends_no_dsn_parameters_even_when_offered() {
    let (result, log) = sent(true, going(false));
    assert_eq!(result.sent, [9]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert!(!log.iter().any(|line| line.contains("RET=") || line.contains("NOTIFY=")), "{log:?}");
}
