// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A run whose token stops being taken part-way (#306): renewed once, the walk goes on; a second
//! refusal ends it. Nothing here leaves this machine; every message is invented.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{GraphMailRequest, GraphMailResult};

use crate::NetError;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;
const RENEWED: &str = "standin.renewed_tok.en-9876543210";
const LATER: &str = "standin.later_tok.en-5555555555";

fn request() -> GraphMailRequest {
    GraphMailRequest { account: 3, email: "owner@example.org".into(), ..Default::default() }
}

/// A mailbox of three folders, the Inbox with three pages of mail.
fn mailbox() -> StandIn {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let projects = server.state().add_folder("Projects", None);
    let travel = server.state().add_folder("Travel", None);
    let mut state = server.state();
    state.mailbox.generate(&inbox, 250, NOW);
    state.mailbox.generate(&projects, 5, NOW);
    state.mailbox.generate(&travel, 5, NOW);
    drop(state);
    server
}

/// A run in which the server stops taking the token in hand once the `n`th page of mail has
/// come (for each `n` of `after`), taking the one of `tokens` beside it instead.
fn run(server: &StandIn, renew: Option<crate::graph::Renew<'_>>, after: &[usize], tokens: &[&str]) -> Vec<GraphMailResult> {
    let mut pieces = Vec::new();
    let mut pages = 0usize;
    let cancel = AtomicBool::new(false);
    super::sync_mail_renewing_with(&server.url(), TOKEN, renew, &request(), Arc::new(Gate::new()), QUICK, &cancel, &mut |piece| {
        let before = pages;
        pages += piece.pages.len();
        for (at, token) in after.iter().zip(tokens) {
            if before < *at && pages >= *at {
                server.state().token = (*token).to_string();
            }
        }
        pieces.push(piece);
    });
    pieces
}

fn refusals(server: &StandIn) -> usize {
    server.state().requests.iter().filter(|asked| asked.status == 401).count()
}

#[test]
fn a_token_refused_part_way_is_renewed_once_and_the_walk_goes_to_its_end() {
    let server = mailbox();
    let renewals = Cell::new(0);
    let renew = || {
        renewals.set(renewals.get() + 1);
        Ok(RENEWED.to_string())
    };
    let pieces = run(&server, Some(&renew), &[1], &[RENEWED]);
    let last = pieces.last().unwrap();
    assert!(last.whole && last.errors.is_empty(), "{:?}", last.errors);
    assert_eq!(renewals.get(), 1);
    assert_eq!(refusals(&server), 1, "the refused request was asked again, once");
    let messages: usize = pieces.iter().flat_map(|p| &p.pages).map(|p| p.messages.len()).sum();
    assert_eq!(messages, 260, "nothing missed");
    // Nothing of either token in what comes back.
    let said = format!("{pieces:?}");
    assert!(!said.contains(TOKEN) && !said.contains(RENEWED));
}

#[test]
fn a_second_refusal_ends_the_run_without_renewing_again() {
    let server = mailbox();
    let renewals = Cell::new(0);
    let renew = || {
        renewals.set(renewals.get() + 1);
        Ok(RENEWED.to_string())
    };
    let pieces = run(&server, Some(&renew), &[1, 2], &[RENEWED, LATER]);
    let last = pieces.last().unwrap();
    assert!(!last.whole && !last.errors.is_empty());
    assert_eq!(renewals.get(), 1, "renewed once a run, never in a loop");
    assert_eq!(refusals(&server), 2);
}

#[test]
fn a_renewal_that_fails_ends_the_run_as_before() {
    let server = mailbox();
    let renew = || Err(NetError::Proto(mailcraft_proto::ProtoError::Auth("the sign-in was refused".into())));
    let pieces = run(&server, Some(&renew), &[1], &[RENEWED]);
    let last = pieces.last().unwrap();
    assert!(!last.whole && !last.errors.is_empty());
    assert_eq!(refusals(&server), 1);
    // Without a way to renew, the same.
    let server = mailbox();
    let pieces = run(&server, None, &[1], &[RENEWED]);
    assert!(!pieces.last().unwrap().whole);
    assert_eq!(refusals(&server), 1);
}

#[test]
fn a_renewed_token_that_cant_go_in_a_header_is_not_sent() {
    let server = mailbox();
    let renew = || Ok("bad\r\ntoken".to_string());
    let pieces = run(&server, Some(&renew), &[1], &[RENEWED]);
    assert!(!pieces.last().unwrap().whole);
    assert!(server.state().requests.iter().all(|asked| !asked.header("authorization").contains("bad")));
}
