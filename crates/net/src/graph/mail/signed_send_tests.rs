// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Digitally signed message sent through Microsoft Graph (#30 R19), against the strict
//! stand-in: it goes with `sendMail` as its own bytes, the signed part byte for byte, a draft the
//! server has of it included (which is then taken away); and one that was to be signed and
//! isn't never goes. Nothing here leaves this machine; every name and message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::signing::SigningKey;
use mailcraft_model::sync::{GraphMailRequest, GraphMailResult, SendOutcome, SendUp};
use mailcraft_model::{Address, Body, Message, Original};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn run(server: &StandIn, sends: Vec<SendUp>) -> GraphMailResult {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), sends, ..Default::default() };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    pieces.into_iter().fold(GraphMailResult::default(), |mut all, piece| {
        all.absorb(piece);
        all
    })
}

fn key() -> SigningKey {
    SigningKey::new(SignatureFormat::Smime, &"AB".repeat(32), "Owner").unwrap_or_else(|| panic!("a key"))
}

/// A message to Kit, to be signed; and, when `signed`, signed (its body the signed layout, with a
/// stand-in signature: the server reads no signature), with the bytes that were signed.
fn message(signed: bool) -> (Message, Vec<u8>) {
    let m = Message {
        id: 5,
        from: Address::new("Owner", "owner@example.org"),
        to: vec![Address::new("Kit", "kit@example.net")],
        subject: "Rota".into(),
        date: NOW,
        message_id: "rota.1@example.org".into(),
        body: Body { text: "Hello Kit,\n\nThe rota is attached. \nFrom Monday on.\n".into(), html: Some("<p>Hello Kit,</p>".into()) },
        sign_with: Some(key()),
        ..Message::default()
    };
    let entity = mailcraft_mime_model::signable(&m);
    if !signed {
        return (m, entity);
    }
    let body = mailcraft_mime::multipart_signed(&entity, mailcraft_mime::SignatureKind::Smime, "sha-256", &[0x30, 0x03, 0x02, 0x01, 0x01])
        .unwrap_or_else(|e| panic!("{e}"));
    (Message { original: Original::new(body), ..m }, entity)
}

#[test]
fn a_signed_message_goes_with_send_mail_byte_for_byte() {
    let server = StandIn::start().unwrap_or_else(|e| panic!("{e}"));
    let (m, entity) = message(true);
    let all = run(&server, vec![SendUp { message: 5, draft: None, mime: Some(Box::new(m)), discard: String::new() }]);
    assert!(matches!(all.sent.as_slice(), [d] if d.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    let state = server.state();
    let sent = state.sent.first().cloned().unwrap_or_default();
    assert_eq!(sent.how, "sendMail");
    let parts = mailcraft_mime::signed_parts(&sent.mime).unwrap_or_else(|| panic!("sent signed: {}", String::from_utf8_lossy(&sent.mime)));
    assert_eq!(parts.kind, mailcraft_mime::SignedKind::SmimeDetached);
    assert_eq!(parts.content, entity, "what was signed went byte for byte");
    assert!(parts.whole);
}

#[test]
fn a_signed_message_saved_as_a_draft_goes_as_its_own_bytes_and_the_draft_is_taken_away() {
    let server = StandIn::start().unwrap_or_else(|e| panic!("{e}"));
    let drafts = server.state().well_known("drafts");
    let draft = server.state().mailbox.add(&drafts, "Rota", NOW);
    server.state().mailbox.update(&draft, |d| d.draft = true);
    let (m, entity) = message(true);
    let all = run(&server, vec![SendUp { message: 5, draft: None, mime: Some(Box::new(m)), discard: draft.clone() }]);
    let done = all.sent.first().cloned().unwrap_or_default();
    assert_eq!(done.outcome, SendOutcome::Sent, "{:?}", all.errors);
    assert!(done.draft_left.is_empty(), "taken away in the run");
    let state = server.state();
    assert!(!state.mailbox.messages.contains_key(&draft), "the server's draft is gone");
    assert_eq!(state.sent.iter().map(|s| s.how.as_str()).collect::<Vec<_>>(), ["sendMail"], "never the draft's send, which rebuilds it");
    let parts = state.sent.first().and_then(|s| mailcraft_mime::signed_parts(&s.mime)).unwrap_or_else(|| panic!("signed"));
    assert_eq!(parts.content, entity);
}

#[test]
fn a_message_to_be_signed_that_isnt_never_goes() {
    let server = StandIn::start().unwrap_or_else(|e| panic!("{e}"));
    let (m, _) = message(false);
    let all = run(&server, vec![SendUp { message: 5, draft: None, mime: Some(Box::new(m)), discard: String::new() }]);
    let done = all.sent.first().cloned().unwrap_or_default();
    assert!(matches!(&done.outcome, SendOutcome::Held { lost: false, why } if why.contains("Digitally signed")), "{done:?}");
    assert!(server.state().sent.is_empty(), "nothing went");
}
