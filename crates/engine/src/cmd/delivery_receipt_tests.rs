// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Request a Delivery Receipt: the message being written asks for one, kept with its draft, and
//! a request in mail that comes in is never taken up. The receipt itself is not a header.

use mailcraft_mime_model::WriteOptions;
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
    s.compose.as_ref().unwrap().delivery_receipt
}

fn written(s: &mut Session) {
    run(s, "mail.new", json!({}));
    run(s, "compose.set", json!({"to": "ada@example.org", "subject": "Tell me", "body": "Did this arrive?"}));
}

#[test]
fn it_turns_on_and_off_and_takes_only_true_or_false() {
    let mut s = session();
    assert!(s.run("compose.deliveryReceipt", &json!({})).is_err(), "nothing is being written");
    written(&mut s);
    assert!(!asked(&s));
    assert_eq!(run(&mut s, "compose.deliveryReceipt", json!({}))["deliveryReceipt"], true);
    assert!(asked(&s) && s.compose.as_ref().unwrap().dirty);
    assert_eq!(run(&mut s, "compose.deliveryReceipt", json!({}))["deliveryReceipt"], false);
    run(&mut s, "compose.deliveryReceipt", json!({"value": true}));
    run(&mut s, "compose.deliveryReceipt", json!({"value": true}));
    assert!(asked(&s));
    for wrong in [json!({"value": "yes"}), json!({"value": 1}), json!({"value": true, "to": "x@example.org"})] {
        assert!(s.run("compose.deliveryReceipt", &wrong).is_err(), "{wrong}");
    }
    assert!(asked(&s));
}

#[test]
fn the_message_sent_keeps_the_request_and_adds_no_header_for_it() {
    let mut s = session();
    written(&mut s);
    run(&mut s, "compose.deliveryReceipt", json!({"value": true}));
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    let sent = s.loaded_message(id).unwrap();
    assert!(sent.delivery_receipt);
    assert!(!sent.read_receipt);
    let mime = String::from_utf8(mailcraft_mime_model::write(&sent, &WriteOptions::default())).unwrap();
    assert!(!mime.to_ascii_lowercase().contains("disposition-notification-to"), "{mime}");
    assert!(!mime.to_ascii_lowercase().contains("return-receipt"), "{mime}");
}

#[test]
fn a_message_that_asks_for_none_keeps_none() {
    let mut s = session();
    written(&mut s);
    let id = run(&mut s, "compose.send", json!({}))["id"].as_u64().unwrap();
    assert!(!s.loaded_message(id).unwrap().delivery_receipt);
}

#[test]
fn the_draft_keeps_it_and_opens_again_with_it() {
    let mut s = session();
    written(&mut s);
    run(&mut s, "compose.deliveryReceipt", json!({"value": true}));
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    assert!(s.loaded_message(draft).unwrap().delivery_receipt);
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert!(asked(&s));
    run(&mut s, "compose.deliveryReceipt", json!({"value": false}));
    run(&mut s, "compose.saveDraft", json!({}));
    assert!(!s.loaded_message(draft).unwrap().delivery_receipt);
}

#[test]
fn a_header_on_a_draft_does_not_ask_for_one() {
    let mut s = session();
    written(&mut s);
    run(&mut s, "compose.saveDraft", json!({}));
    let draft = s.compose.as_ref().unwrap().draft.unwrap();
    run(&mut s, "compose.close", json!({}));
    s.mailbox.message_mut(draft).unwrap().headers.push(("Return-Receipt-To".into(), "owner@example.org".into()));
    run(&mut s, "mail.editDraft", json!({"id": draft}));
    assert!(!asked(&s));
}

#[test]
fn a_request_in_mail_that_comes_in_is_never_taken_up() {
    let raw = b"From: Bea <bea@example.org>\r\nTo: owner@example.org\r\nSubject: Arrived?\r\nReturn-Receipt-To: bea@example.org\r\n\r\nHello\r\n";
    let m = mailcraft_mime_model::parse(raw);
    assert!(!m.delivery_receipt, "nothing is set by what a sender asks");
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
    let r = s.run("compose.deliveryReceipt", &json!({"value": true}));
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
    assert_eq!(super::graph_drafts::draft_up(&m).unwrap().delivery_receipt, None);
    m.delivery_receipt = true;
    assert_eq!(super::graph_drafts::draft_up(&m).unwrap().delivery_receipt, Some(true));
    assert_eq!(super::graph_drafts::draft_up_whole(&m).unwrap().delivery_receipt, Some(true));
}
