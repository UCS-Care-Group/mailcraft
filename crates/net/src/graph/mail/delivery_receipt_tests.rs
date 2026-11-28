// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message sent straight away, never saved as a draft, that asks for a delivery receipt.
//! `sendMail` takes MIME, and the request is not a header, so the send makes a draft from that
//! MIME, sets the flag, and sends the draft. Nothing here leaves this machine.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{GraphMailRequest, GraphMailResult, SendOutcome, SendUp};
use mailcraft_model::{Address, Body, Message};
use serde_json::json;

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };

fn run(server: &StandIn, sends: Vec<SendUp>) -> GraphMailResult {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), sends, ..Default::default() };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    pieces.into_iter().fold(GraphMailResult::default(), |mut all, piece| {
        all.absorb(piece);
        all
    })
}

fn going(delivery_receipt: bool) -> Message {
    Message {
        to: vec![Address::new("Kit", "kit@example.org")],
        subject: "On its way".into(),
        body: Body { text: "Hello.".into(), html: None },
        in_reply_to: "orig@example.org".into(),
        message_id: "mine@example.org".into(),
        date: 1_790_000_000,
        delivery_receipt,
        ..Default::default()
    }
}

#[test]
fn a_message_never_saved_as_a_draft_is_sent_with_the_delivery_receipt_set() {
    let server = StandIn::start().unwrap();
    let all = run(&server, vec![SendUp { message: 9, draft: None, mime: Some(Box::new(going(true))) }]);
    assert!(all.errors.is_empty(), "{:?}", all.errors);
    assert!(matches!(all.sent.as_slice(), [done] if done.message == 9 && done.outcome == SendOutcome::Sent), "{:?}", all.sent);
    let sent_items = server.state().well_known("sentitems");
    let state = server.state();
    assert_eq!(state.sent.len(), 1, "made once and sent once");
    assert_eq!(state.sent[0].how, "send");
    assert!(state.sent[0].in_reply_to.contains("orig@example.org"), "{}", state.sent[0].in_reply_to);
    let copy = state.mailbox.messages.values().find(|m| m.folder == sent_items).expect("filed in Sent Items");
    assert!(copy.delivery_receipt, "the flag went with the draft that was sent");
    assert!(!copy.draft);
}

/// A draft left from an earlier attempt, found by the same marker the send writes.
fn leave_draft(server: &StandIn, subject: &str, marker: &str) {
    let body = json!({
        "subject": subject,
        "body": {"contentType": "text", "content": "the earlier copy"},
        "toRecipients": [{"emailAddress": {"name": "Kit", "address": "kit@example.org"}}],
        "singleValueExtendedProperties": [{"id": super::up::MARKER, "value": marker}],
    })
    .to_string();
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let head = format!(
        "POST /v1.0/me/messages HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    let mut all = String::new();
    stream.read_to_string(&mut all).unwrap();
    assert!(all.starts_with("HTTP/1.1 201"), "{all}");
}

#[test]
fn a_corrected_message_replaces_the_draft_an_earlier_attempt_left() {
    let server = StandIn::start().unwrap();
    leave_draft(&server, "Earlier copy", "mailcraft-send-9");
    let mut message = going(true);
    message.subject = "Corrected".into();
    let all = run(&server, vec![SendUp { message: 9, draft: None, mime: Some(Box::new(message)) }]);
    assert!(all.errors.is_empty(), "{:?}", all.errors);
    assert!(matches!(all.sent.as_slice(), [done] if done.message == 9 && done.outcome == SendOutcome::Sent), "{:?}", all.sent);
    let state = server.state();
    assert_eq!(state.sent.len(), 1, "the earlier draft is removed, not sent");
    assert_eq!(state.sent[0].subject, "Corrected");
    assert_eq!(state.sent[0].how, "send");
    assert!(state.mailbox.messages.values().all(|m| m.subject != "Earlier copy"));
    let sent_items = state.well_known("sentitems");
    let copy = state.mailbox.messages.values().find(|m| m.folder == sent_items).expect("filed in Sent Items");
    assert_eq!(copy.subject, "Corrected");
    assert!(copy.delivery_receipt);
}

#[test]
fn a_message_that_asks_for_none_still_goes_with_send_mail() {
    let server = StandIn::start().unwrap();
    let all = run(&server, vec![SendUp { message: 4, draft: None, mime: Some(Box::new(going(false))) }]);
    assert!(matches!(all.sent.as_slice(), [done] if done.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    assert_eq!(server.state().sent[0].how, "sendMail");
}
