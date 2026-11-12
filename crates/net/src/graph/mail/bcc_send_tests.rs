// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Encrypted message to Bcc recipients sent through Microsoft Graph (#30 part 7), against the
//! strict stand-in: each Bcc recipient gets a copy of their own and the server files exactly one
//! copy in Sent Items, a send that stops half way is never reported as sent and remembers who had
//! theirs, and a draft kept Encrypted is taken away for good only when it is still a draft in
//! Drafts that carries our marker. Nothing here leaves this machine; every name is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::encrypting::{BlindCopy, EncryptTo, Encryption};
use mailcraft_model::sync::{DraftUp, GraphMailRequest, GraphMailResult, SendOutcome, SendUp};
use mailcraft_model::{Address, Message, Original};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 2, first_wait: Duration::from_millis(5), longest_wait: Duration::from_millis(20) };
const NOW: i64 = 1_790_000_000;
const MARKER: &str = "mailcraft-draft-5-1";

fn run(server: &StandIn, request: GraphMailRequest) -> GraphMailResult {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), ..request };
    let pieces = whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    pieces.into_iter().fold(GraphMailResult::default(), |mut all, piece| {
        all.absorb(piece);
        all
    })
}

fn body(tag: u8) -> Original {
    Original::new(mailcraft_mime::smime_enveloped(&[0x30, 0x03, 0x02, 0x01, tag]).unwrap()).unwrap()
}

fn people(all: &[&str]) -> Vec<Address> {
    all.iter().map(|a| Address::new("", *a)).collect()
}

/// An Encrypted message as the host hands it to a run (stand-in bodies: the server reads no encryption).
fn encrypted(to: &[&str], bcc: &[&str], sent: &[&str]) -> Box<Message> {
    Box::new(Message {
        id: 5,
        subject: "Rota".into(),
        from: Address::new("Owner", "owner@example.org"),
        to: people(to),
        bcc: people(bcc),
        date: NOW,
        message_id: "rota.1@example.org".into(),
        encrypt: Some(true),
        encrypt_with: Some(Encryption {
            format: SignatureFormat::Smime,
            recipients: vec![EncryptTo { address: "zoe@example.org".into(), fingerprint: "AB".into(), key: Arc::new(vec![1]), own: false }],
            own_address: "owner@example.org".into(),
        }),
        original: Some(body(0xAA)),
        blind_copies: bcc
            .iter()
            .filter(|a| !sent.contains(a))
            .enumerate()
            .map(|(at, a)| BlindCopy { address: (*a).to_string(), original: body(0xB0 + u8::try_from(at).unwrap_or(0)) })
            .collect(),
        blind_sent: sent.iter().map(|a| (*a).to_string()).collect(),
        ..Message::default()
    })
}

fn send(message: Box<Message>) -> GraphMailRequest {
    GraphMailRequest { sends: vec![SendUp { message: 5, draft: None, mime: Some(message), discard: String::new() }], ..Default::default() }
}

fn in_sent_items(server: &StandIn) -> usize {
    let state = server.state();
    let sent = state.well_known("sentitems");
    state.mailbox.messages.values().filter(|m| m.folder == sent).count()
}

#[test]
fn two_bcc_recipients_leave_one_copy_in_sent_items() {
    let server = StandIn::start().unwrap();
    let all = run(&server, send(encrypted(&["zoe@example.org"], &["erin@example.org", "finn@example.org"], &[])));
    assert!(matches!(all.sent.as_slice(), [d] if d.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    assert_eq!(all.sent[0].blind_sent, ["erin@example.org", "finn@example.org"]);
    assert_eq!(server.state().sent.len(), 3, "a message for each of the two and the one for the rest");
    assert_eq!(in_sent_items(&server), 1, "one copy in Sent Items, not one a recipient");
}

#[test]
fn a_main_message_that_is_throttled_after_the_bcc_copies_is_not_sent_and_remembers_them() {
    let server = StandIn::start().unwrap();
    for _ in 0..8 {
        server.state().fail_next.push(("POST /v1.0/me/sendMail".into(), 429, "TooManyRequests".into()));
    }
    let all = run(&server, send(encrypted(&["zoe@example.org"], &["erin@example.org"], &[])));
    assert!(!all.sent.iter().any(|d| d.outcome == SendOutcome::Sent), "{:?}", all.sent);
    let went: Vec<&String> = all.sent.iter().flat_map(|d| &d.blind_sent).collect();
    assert_eq!(went, ["erin@example.org"], "{:?} {:?}", all.sent, all.errors);
}

#[test]
fn a_send_again_gives_those_who_had_theirs_nothing_more() {
    let server = StandIn::start().unwrap();
    let all = run(&server, send(encrypted(&["zoe@example.org"], &["erin@example.org", "finn@example.org"], &["erin@example.org"])));
    assert!(matches!(all.sent.as_slice(), [d] if d.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    assert_eq!(all.sent[0].blind_sent, ["finn@example.org"]);
    assert_eq!(server.state().sent.len(), 2);
    assert_eq!(in_sent_items(&server), 1);
}

#[test]
fn an_encrypted_message_to_bcc_recipients_alone_is_sent_and_filed_once() {
    let server = StandIn::start().unwrap();
    let all = run(&server, send(encrypted(&[], &["erin@example.org", "finn@example.org"], &[])));
    assert!(matches!(all.sent.as_slice(), [d] if d.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    assert_eq!(server.state().sent.len(), 2, "theirs, and nothing for nobody");
    assert_eq!(in_sent_items(&server), 1);
}

fn sealed() -> Vec<u8> {
    let m = Message {
        from: Address::new("Owner", "owner@example.org"),
        to: vec![Address::new("Kit", "kit@example.net")],
        subject: "Plans".into(),
        date: NOW,
        message_id: "plans.1-s1@example.org".into(),
        original: Some(body(1)),
        ..Message::default()
    };
    mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default())
}

fn clear_draft(server: &StandIn) -> String {
    let first = run(
        server,
        GraphMailRequest {
            drafts: vec![DraftUp { message: 5, marker: MARKER.into(), saves: 1, subject: Some("Plans".into()), ..Default::default() }],
            ..Default::default()
        },
    );
    first.drafts.first().cloned().unwrap().id
}

fn purge_only() -> GraphMailRequest {
    let purge = DraftUp { message: 5, marker: MARKER.into(), saves: 2, purge: true, purge_only: true, ..Default::default() };
    GraphMailRequest { drafts: vec![purge], ..Default::default() }
}

#[test]
fn a_permanent_delete_that_is_throttled_is_not_softened_to_a_delete() {
    let server = StandIn::start().unwrap();
    let clear = clear_draft(&server);
    for _ in 0..8 {
        server.state().fail_next.push(("POST".into(), 429, "TooManyRequests".into()));
    }
    let all = run(&server, purge_only());
    let state = server.state();
    assert!(state.mailbox.messages.contains_key(&clear), "kept for the next run, not deleted into the recoverable items: {:?}", all.errors);
    assert!(state.mailbox.purged.is_empty());
}

#[test]
fn a_purge_only_save_takes_the_copy_away_for_good() {
    let server = StandIn::start().unwrap();
    let clear = clear_draft(&server);
    let all = run(&server, purge_only());
    assert!(all.drafts.iter().any(|d| d.purged), "{:?} {:?}", all.drafts, all.errors);
    let state = server.state();
    assert!(!state.mailbox.messages.contains_key(&clear));
    assert_eq!(state.mailbox.purged.as_slice(), std::slice::from_ref(&clear));
}

#[test]
fn what_is_no_longer_a_draft_or_is_not_in_drafts_is_never_taken_away() {
    let server = StandIn::start().unwrap();
    let clear = clear_draft(&server);
    // Our marker, as the draft above has it.
    let mark = server.state().mailbox.messages.get(&clear).map(|m| m.extended.clone()).unwrap();
    assert!(!mark.is_empty(), "the draft carries its marker");
    // A message with our marker that has been sent (another device sent the draft): in Sent Items.
    let sent_items = server.state().well_known("sentitems");
    let sent = server.state().mailbox.add(&sent_items, "Plans", NOW);
    server.state().mailbox.update(&sent, |m| m.extended = mark.clone());
    // A message with our marker in Drafts that isn't a draft any more.
    let drafts = server.state().well_known("drafts");
    let not_draft = server.state().mailbox.add(&drafts, "Plans", NOW);
    server.state().mailbox.update(&not_draft, |m| {
        m.draft = false;
        m.extended = mark.clone();
    });
    run(&server, purge_only());
    let state = server.state();
    assert!(state.mailbox.messages.contains_key(&sent), "a sent message is left alone");
    assert!(state.mailbox.messages.contains_key(&not_draft), "what is not a draft is left alone");
    assert!(!state.mailbox.messages.contains_key(&clear), "the draft is taken away");
}

#[test]
fn a_sealed_create_the_server_refuses_still_takes_the_clear_copy_away() {
    let server = StandIn::start().unwrap();
    let clear = clear_draft(&server);
    server.state().fail_next.push(("POST /v1.0/me/messages".into(), 413, "ErrorInvalidRequest".into()));
    let up =
        DraftUp { message: 5, id: clear.clone(), marker: MARKER.into(), saves: 2, purge: true, sealed: Arc::new(sealed()), ..Default::default() };
    let all = run(&server, GraphMailRequest { drafts: vec![up], ..Default::default() });
    let done = all.drafts.first().cloned().unwrap_or_else(|| panic!("an answer: {:?}", all.errors));
    assert!(done.refused.is_some(), "{done:?}");
    assert!(!server.state().mailbox.messages.contains_key(&clear), "no clear copy is left behind a refused one");
}

fn drafts_in_drafts(server: &StandIn) -> usize {
    let state = server.state();
    let drafts = state.well_known("drafts");
    state.mailbox.messages.values().filter(|m| m.folder == drafts && m.draft).count()
}

#[test]
fn a_copy_whose_send_is_throttled_leaves_no_draft_behind_after_a_retry_and_is_sent_once() {
    let server = StandIn::start().unwrap();
    for _ in 0..8 {
        server.state().fail_next.push(("POST /v1.0/me/messages/*/send".into(), 429, "TooManyRequests".into()));
    }
    let all = run(&server, send(encrypted(&["zoe@example.org"], &["erin@example.org"], &[])));
    assert!(!all.sent.iter().any(|d| d.outcome == SendOutcome::Sent), "{:?}", all.sent);
    // (The mailbox is told to wait, so the delete of that draft waits too and may not come in this run:
    // the next try finds it by its Message-ID and takes it away, whatever this one managed.)
    assert!(drafts_in_drafts(&server) <= 1, "{:?}", all.errors);
    server.state().fail_next.clear();
    let all = run(&server, send(encrypted(&["zoe@example.org"], &["erin@example.org"], &[])));
    assert!(matches!(all.sent.as_slice(), [d] if d.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    assert_eq!(drafts_in_drafts(&server), 0);
    assert_eq!(in_sent_items(&server), 1);
}

#[test]
fn a_draft_left_by_a_lost_answer_is_found_by_its_message_id_and_taken_away_by_the_next_try() {
    let server = StandIn::start().unwrap();
    server.state().lose_next.push("POST /v1.0/me/messages".into());
    let all = run(&server, send(encrypted(&["zoe@example.org"], &["erin@example.org"], &[])));
    assert!(!all.sent.iter().any(|d| d.outcome == SendOutcome::Sent), "{:?}", all.sent);
    assert_eq!(drafts_in_drafts(&server), 1, "made, and nobody knows it");
    let all = run(&server, send(encrypted(&["zoe@example.org"], &["erin@example.org"], &[])));
    assert!(matches!(all.sent.as_slice(), [d] if d.outcome == SendOutcome::Sent), "{:?} {:?}", all.sent, all.errors);
    assert_eq!(drafts_in_drafts(&server), 0);
    assert_eq!(server.state().sent.len(), 2, "theirs and the message itself, each once");
}

#[test]
fn an_encrypted_draft_whose_make_was_lost_is_swept_by_its_message_id_at_the_next_save() {
    let server = StandIn::start().unwrap();
    server.state().lose_next.push("POST /v1.0/me/messages".into());
    let up = || DraftUp { message: 5, marker: MARKER.into(), saves: 1, sealed: Arc::new(sealed()), ..Default::default() };
    run(&server, GraphMailRequest { drafts: vec![up()], ..Default::default() });
    assert_eq!(drafts_in_drafts(&server), 1, "an orphan: no marker went on");
    let all = run(&server, GraphMailRequest { drafts: vec![up()], ..Default::default() });
    assert!(all.drafts.iter().any(|d| d.fields && !d.id.is_empty()), "{:?} {:?}", all.drafts, all.errors);
    assert_eq!(drafts_in_drafts(&server), 1, "the one made now");
}

#[test]
fn a_purge_reads_the_whole_list_of_copies_however_long() {
    let server = StandIn::start().unwrap();
    let clear = clear_draft(&server);
    let mark = server.state().mailbox.messages.get(&clear).map(|m| m.extended.clone()).unwrap();
    let drafts = server.state().well_known("drafts");
    for _ in 0..70 {
        let id = server.state().mailbox.add(&drafts, "Plans", NOW);
        server.state().mailbox.update(&id, |m| {
            m.draft = true;
            m.extended = mark.clone();
        });
    }
    let all = run(&server, purge_only());
    assert!(all.drafts.iter().any(|d| d.purged), "{:?} {:?}", all.drafts, all.errors);
    assert_eq!(drafts_in_drafts(&server), 0, "all seventy-one, past the first page of fifty");
    assert_eq!(server.state().mailbox.purged.len(), 71);
}

#[test]
fn the_one_shot_send_of_an_encrypted_message_files_one_copy_and_says_who_had_theirs() {
    let server = StandIn::start().unwrap();
    let mut went = Vec::new();
    let message = encrypted(&["zoe@example.org"], &["erin@example.org", "finn@example.org"], &[]);
    crate::graph::send_recording_at(&server.url(), TOKEN, &message, &Default::default(), &mut went).unwrap();
    assert_eq!(went, ["erin@example.org", "finn@example.org"]);
    assert_eq!(server.state().sent.len(), 3);
    assert_eq!(in_sent_items(&server), 1, "one copy in Sent Items, not one a recipient");
    // And stopped at the message itself: theirs are remembered.
    let server = StandIn::start().unwrap();
    for _ in 0..3 {
        server.state().fail_next.push(("POST /v1.0/me/sendMail".into(), 503, "TooManyRequests".into()));
    }
    let mut went = Vec::new();
    let stopped = crate::graph::send_recording_at(&server.url(), TOKEN, &message, &Default::default(), &mut went);
    assert!(stopped.is_err());
    assert_eq!(went, ["erin@example.org", "finn@example.org"]);
}

#[test]
fn another_draft_that_shares_the_messages_own_message_id_is_never_swept() {
    let server = StandIn::start().unwrap();
    let clear = clear_draft(&server);
    let mut mark = server.state().mailbox.messages.get(&clear).map(|m| m.extended.clone()).unwrap();
    for entry in &mut mark {
        entry.1 = "mailcraft-draft-9-9".into();
    }
    // A conflict copy, or one made in Outlook: it carries the message's own Message-ID and a marker of its own.
    let drafts = server.state().well_known("drafts");
    let other = server.state().mailbox.add(&drafts, "Plans", NOW);
    server.state().mailbox.update(&other, |m| {
        m.draft = true;
        m.internet_id = "<plans.1@example.org>".into();
        m.extended = mark.clone();
    });
    let all = run(
        &server,
        GraphMailRequest {
            drafts: vec![DraftUp {
                message: 5,
                id: clear,
                marker: MARKER.into(),
                saves: 1,
                purge: true,
                sealed: Arc::new(sealed()),
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    assert!(all.drafts.iter().any(|d| d.fields && d.purged), "{:?} {:?}", all.drafts, all.errors);
    let state = server.state();
    assert!(state.mailbox.messages.contains_key(&other), "the other draft is not this one's to take away");
    assert!(!state.mailbox.purged.contains(&other));
}

#[test]
fn an_encrypted_drafts_message_id_is_its_own_and_never_the_messages() {
    use mailcraft_model::encrypting::{is_sealed_draft_message_id, sealed_draft_message_id};
    assert_eq!(sealed_draft_message_id("plans.1@example.org", 3), "plans.1-s3@example.org");
    assert!(is_sealed_draft_message_id("<plans.1-s3@example.org>"));
    for not in ["plans.1@example.org", "<plans-s@example.org>", "plans-sx1@example.org", ""] {
        assert!(!is_sealed_draft_message_id(not), "{not}");
    }
}
