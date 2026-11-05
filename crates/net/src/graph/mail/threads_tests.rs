// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversation of a Microsoft account's message (#795): read with its listing, and asked for
//! once, twenty to a batch, of mail listed before it was. Through the strict Graph stand-in;
//! nothing here leaves this machine, and every message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{GraphMailRequest, GraphMailResult, ThreadOf};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn run(server: &StandIn, threads: Vec<String>) -> Vec<GraphMailResult> {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), threads, ..Default::default() };
    whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false))
}

#[test]
fn a_listing_brings_each_messages_conversation() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 3, NOW);
    for id in &ids[..2] {
        server.state().mailbox.update(id, |m| m.conversation = "AAQkADconvShared".into());
    }
    let pieces = run(&server, Vec::new());
    let listed: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).collect();
    let thread_of = |id: &str| listed.iter().find(|m| m.server.id == id).map(|m| m.server.thread.clone()).unwrap();
    assert_eq!(thread_of(&ids[0]), "AAQkADconvShared");
    assert_eq!(thread_of(&ids[1]), "AAQkADconvShared");
    let own = server.state().mailbox.messages.get(&ids[2]).map(|m| m.conversation_id()).unwrap();
    assert_eq!(thread_of(&ids[2]), own);
    // Nothing was asked: nothing comes.
    assert!(pieces.iter().all(|p| p.threads.is_empty()));
}

#[test]
fn mail_listed_before_is_asked_for_its_conversation_twenty_to_a_batch() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 25, NOW);
    server.state().mailbox.update(&ids[0], |m| m.conversation = "AAQkADconvA".into());
    // One that has gone from the server since.
    let gone = ids[24].clone();
    server.state().mailbox.delete(&gone);
    let pieces = run(&server, ids.clone());
    let last = pieces.last().unwrap();
    assert!(!last.partial && last.errors.is_empty(), "{:?}", last.errors);
    let said = |id: &str| last.threads.iter().find(|t| t.id == id).map(|t| t.thread.clone());
    assert_eq!(said(&ids[0]).as_deref(), Some("AAQkADconvA"));
    let own = server.state().mailbox.messages.get(&ids[1]).map(|m| m.conversation_id()).unwrap();
    assert_eq!(said(&ids[1]), Some(own));
    // Gone: said with none, so that it isn't asked again.
    assert_eq!(said(&gone).as_deref(), Some(""));
    assert_eq!(last.threads.len(), 25);
    // Two batches for 25, and only the conversation asked of each.
    let state = server.state();
    let batches: Vec<_> = state.requests.iter().filter(|r| r.method == "POST" && r.path.last().is_some_and(|p| p == "$batch")).collect();
    let asked: Vec<_> =
        state.requests.iter().filter(|r| r.method == "GET" && r.query.iter().any(|(k, v)| k == "$select" && v == "conversationId")).collect();
    assert_eq!(asked.len(), 25);
    assert!(batches.len() >= 2, "{} batches", batches.len());
}

#[test]
fn a_server_that_wont_answer_ends_the_look_up_and_never_the_run() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 3, NOW);
    server.state().refuse_batches = true;
    let pieces = run(&server, ids);
    let last = pieces.last().unwrap();
    assert!(!last.partial && last.errors.is_empty(), "{:?}", last.errors);
    // A server that takes no batch (400) refuses nothing of the messages themselves.
    assert_eq!(last.threads, Vec::<ThreadOf>::new());
    // Ids that are none, or more than a run asks, are not sent.
    let server = StandIn::start().unwrap();
    let mut junk: Vec<String> = vec!["../../me/sendMail".into(), String::new(), "x".repeat(5_000)];
    junk.extend((0..1_500).map(|n| format!("AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A{n:024}")));
    let pieces = run(&server, junk);
    let last = pieces.last().unwrap();
    assert!(last.errors.is_empty(), "{:?}", last.errors);
    let state = server.state();
    let asked = state.requests.iter().filter(|r| r.query.iter().any(|(k, v)| k == "$select" && v == "conversationId")).count();
    assert!(asked <= mailcraft_model::sync::MAX_THREAD_ASKS, "{asked}");
}

#[test]
fn a_mailbox_that_may_not_be_asked_says_refused_and_a_busy_one_says_busy() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 3, NOW);
    server.state().conversations_answer = Some(403);
    let pieces = run(&server, ids.clone());
    let last = pieces.last().unwrap();
    assert!(last.errors.is_empty() && last.busy_for.is_none(), "{:?}", last.errors);
    assert_eq!(last.threads, ids.iter().map(|id| ThreadOf::refused(id)).collect::<Vec<_>>());
    // Busy: nothing said of the messages, and the run says how long to leave the server alone.
    server.state().conversations_answer = Some(429);
    let pieces = run(&server, ids);
    let last = pieces.last().unwrap();
    assert!(last.threads.is_empty(), "{:?}", last.threads);
    assert!(last.busy_for.is_some());
}
