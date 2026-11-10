// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Request a Read Receipt: the message being written asks for one, to its sender, by every way it
//! goes; it is kept with its draft; and a request in mail that comes in is never taken up.

use mailcraft_mime::WriteOptions;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn asked(s: &Session) -> bool {
    s.compose.as_ref().unwrap().read_receipt
}

fn written(s: &mut Session) {
    run(s, "mail.new", json!({}));
    run(s, "compose.set", json!({"to": "ada@example.org", "subject": "Tell me", "body": "Did this arrive?"}));
}

#[test]
fn it_turns_on_and_off_and_takes_only_true_or_false() {
    let mut s = session();
    assert!(s.run("compose.readReceipt", &json!({})).is_err(), "nothing is being written");
    written(&mut s);
    assert!(!asked(&s));
    assert_eq!(run(&mut s, "compose.readReceipt", json!({}))["readReceipt"], true);
    assert!(asked(&s) && s.compose.as_ref().unwrap().dirty);
    assert_eq!(run(&mut s, "compose.readReceipt", json!({}))["readReceipt"], false);
    run(&mut s, "compose.readReceipt", json!({"value": true}));
    run(&mut s, "compose.readReceipt", json!({"value": true}));
    assert!(asked(&s));
    for wrong in [json!({"value": "yes"}), json!({"value": 1}), json!({"value": true, "to": "x@example.org"})] {
        assert!(s.run("compose.readReceipt", &wrong).is_err(), "{wrong}");
    }
    assert!(asked(&s));
}

#[test]
fn the_message_sent_asks_its_sender_for_the_receipt() {
    let mut s = session();
    written(&mut s);
    run(&mut s, "compose.readReceipt", json!({"value": true}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let sent = s.loaded_message(id).unwrap();
    assert!(sent.read_receipt);
    let mime = String::from_utf8(mailcraft_mime::write(&sent, &WriteOptions::default())).unwrap();
    let line = mime.lines().find(|l| l.starts_with("Disposition-Notification-To: ")).unwrap_or_default();
    assert!(!sent.from.email.is_empty() && line.contains(&sent.from.email), "{mime}");
    assert_eq!(mime.matches("Disposition-Notification-To").count(), 1);
}

#[test]
fn a_message_that_asks_for_none_carries_none() {
    let mut s = session();
    written(&mut s);
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let sent = s.loaded_message(id).unwrap();
    assert!(!sent.read_receipt);
    let mime = String::from_utf8(mailcraft_mime::write(&sent, &WriteOptions::default())).unwrap();
    assert!(!mime.contains("Disposition-Notification-To"), "{mime}");
}

#[test]
fn the_draft_keeps_it_and_opens_again_with_it() {
    let mut s = session();
    written(&mut s);
    run(&mut s, "compose.readReceipt", json!({"value": true}));
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    assert!(s.loaded_message(draft).unwrap().read_receipt);
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert!(asked(&s));
    // Turned off and saved again, it stays off.
    run(&mut s, "compose.readReceipt", json!({"value": false}));
    run(&mut s, "compose.saveDraft", json!({}));
    assert!(!s.loaded_message(draft).unwrap().read_receipt);
}

#[test]
fn a_draft_from_a_server_asks_for_one_by_its_header() {
    let mut s = session();
    written(&mut s);
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    run(&mut s, "compose.close", json!({}));
    // As an IMAP server's Drafts gives it back: the request is one of its headers.
    s.mailbox.message_mut(draft).unwrap().headers.push(("Disposition-Notification-To".into(), "owner@example.org".into()));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert!(asked(&s));
}

#[test]
fn a_request_in_mail_that_comes_in_is_never_taken_up() {
    let raw =
        b"From: Bea <bea@example.org>\r\nTo: owner@example.org\r\nSubject: Read me\r\nDisposition-Notification-To: bea@example.org\r\n\r\nHello\r\n";
    let m = mailcraft_mime::parse(raw);
    assert!(!m.read_receipt, "nothing is set by what a sender asks");
    // A reply to it doesn't ask for one either.
    let mut s = session();
    let id = s.mailbox.messages.values().find(|m| !m.draft).unwrap().id;
    run(&mut s, "mail.reply", json!({"id": id}));
    assert!(!asked(&s));
}

#[test]
fn an_agent_does_not_change_it_in_the_owners_message() {
    let mut s = session();
    written(&mut s);
    run(&mut s, "compose.type", json!({"text": " Yes?"}));
    assert!(s.compose.as_ref().unwrap().owner_changed);
    s.agent = Some(Vec::new());
    let r = s.run("compose.readReceipt", &json!({"value": true}));
    s.agent = None;
    let e = r.expect_err("the owner's message").to_string();
    assert!(e.contains("the message the owner is writing"), "{e}");
    assert!(!asked(&s));
}

#[test]
fn a_draft_of_a_microsoft_account_takes_it_up() {
    let mut m = mailcraft_model::Message {
        id: 7,
        draft: true,
        server_draft: Some(mailcraft_model::sync::ServerDraft { marker: "m".into(), ..Default::default() }),
        ..Default::default()
    };
    // A new draft that asks for none says nothing of it.
    assert_eq!(super::graph_drafts::draft_up(&m).unwrap().read_receipt, None);
    m.read_receipt = true;
    assert_eq!(super::graph_drafts::draft_up(&m).unwrap().read_receipt, Some(true));
    assert_eq!(super::graph_drafts::draft_up_whole(&m).unwrap().read_receipt, Some(true));
}
