// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversation of an Exchange server's message (#795): read with the folder's changes, and
//! asked for once (`GetItem`, a hundred to a request) of mail listed before it was. Against the
//! strict stand-in; every message is invented.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{EwsMailRequest, EwsMailResult};

use crate::standin::{AUTHORIZATION, StandIn, block_on};
use crate::*;

const URL: &str = "https://mail.example.org/EWS/Exchange.asmx";

fn run(standin: &StandIn, threads: Vec<String>) -> Vec<EwsMailResult> {
    let client = Client::new(URL, Secret::new(AUTHORIZATION)).unwrap();
    let request = EwsMailRequest { email: "ada@example.org".into(), url: URL.into(), list_folders: true, threads, ..Default::default() };
    let mut pieces = Vec::new();
    block_on(sync_mail(&client, standin, &request, &AtomicBool::new(false), &mut |_| true, &mut |piece| pieces.push(piece))).unwrap();
    pieces
}

#[test]
fn the_changes_bring_each_messages_conversation() {
    let standin = StandIn::new();
    let (a, b) = {
        let mut s = standin.state();
        (s.deliver("inbox", "Budget", false), s.deliver("inbox", "RE: Budget", false))
    };
    for item in standin.state().items.iter_mut() {
        item.conversation = "AAQkconvBudget".into();
    }
    let pieces = run(&standin, Vec::new());
    let listed: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).collect();
    for id in [&a, &b] {
        assert_eq!(listed.iter().find(|m| m.server.id == *id).unwrap().server.thread, "AAQkconvBudget");
    }
    assert!(pieces.iter().all(|p| p.threads.is_empty()));
}

#[test]
fn mail_listed_before_is_asked_for_its_conversation_once() {
    let standin = StandIn::new();
    let ids: Vec<String> = (0..150).map(|n| standin.state().deliver("inbox", &format!("Mail {n}"), true)).collect();
    let gone = "AAMkAGone".to_string();
    let mut asked = ids.clone();
    asked.push(gone.clone());
    let pieces = run(&standin, asked);
    let last = pieces.last().unwrap();
    assert!(last.errors.is_empty(), "{:?}", last.errors);
    assert_eq!(last.threads.len(), 151);
    let said = |id: &str| last.threads.iter().find(|t| t.id == id).map(|t| t.thread.clone());
    assert_eq!(said(&ids[0]), standin.state().item(&ids[0]).map(|i| i.conversation_id()));
    // Not on the server: none, so that it isn't asked again.
    assert_eq!(said(&gone).as_deref(), Some(""));
    assert!(last.check().is_ok());
}

#[test]
fn a_server_that_doesnt_know_the_conversation_still_lists_its_mail() {
    let standin = StandIn::new();
    let id = standin.state().deliver("inbox", "Plain", true);
    standin.state().lacks_preview = true;
    let pieces = run(&standin, vec![id.clone()]);
    let listed: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).collect();
    // Read in the lean shape, without a conversation; the look-up then asks for it alone.
    assert!(listed.iter().any(|m| m.server.id == id && m.server.thread.is_empty()));
    assert!(pieces.last().unwrap().errors.is_empty());
}
