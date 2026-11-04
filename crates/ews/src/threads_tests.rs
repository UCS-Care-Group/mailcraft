// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The conversation of an Exchange server's message (#795): asked for by itself (`GetItem`, a
//! hundred to a request) of mail listed without it, since the folders' changes don't ask for it. Against the
//! strict stand-in; every message is invented.

use std::sync::atomic::AtomicBool;

use mailcraft_model::sync::{EwsMailRequest, EwsMailResult, ThreadOf};

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
fn the_changes_dont_ask_for_the_conversation_and_the_look_up_brings_it() {
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
    assert!(listed.iter().all(|m| m.server.thread.is_empty()));
    assert!(pieces.iter().all(|p| p.threads.is_empty()));
    let pieces = run(&standin, vec![a.clone(), b.clone()]);
    let said = &pieces.last().unwrap().threads;
    assert!(said.iter().all(|t| t.thread == "AAQkconvBudget" && !t.refused) && said.len() == 2, "{said:?}");
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
fn a_server_that_doesnt_know_the_conversation_still_gives_previews_and_flags() {
    let standin = StandIn::new();
    let id = standin.state().deliver("inbox", "Plain", true);
    for item in standin.state().items.iter_mut() {
        (item.preview, item.flagged, item.flag) = ("Hello there".into(), true, "Flagged".into());
    }
    standin.state().lacks_conversation = true;
    let pieces = run(&standin, vec![id.clone()]);
    let listed: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).collect();
    let message = listed.iter().find(|m| m.server.id == id).unwrap();
    assert_eq!(message.preview, "Hello there");
    assert_eq!(message.server.flag, mailcraft_model::FlagStatus::Flagged, "the flag still comes");
    let last = pieces.last().unwrap();
    assert!(last.errors.is_empty() && last.busy_for.is_none(), "{:?}", last.errors);
    // Refused, said so: the engine asks again only a few runs.
    assert_eq!(last.threads, vec![ThreadOf::refused(&id)]);
}

#[test]
fn a_busy_server_during_the_look_up_is_said() {
    let standin = StandIn::new();
    let id = standin.state().deliver("inbox", "Plain", true);
    standin.state().busy_get_item = true;
    let pieces = run(&standin, vec![id]);
    let last = pieces.last().unwrap();
    assert!(last.busy_for.is_some() && last.threads.is_empty(), "{last:?}");
}
