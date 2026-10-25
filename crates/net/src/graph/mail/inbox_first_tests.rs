// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Inbox comes before the housekeeping (#276), measured in the order of the requests the
//! Graph stand-in saw, never in time. Every message is invented.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::FolderKind;
use mailcraft_model::sync::{GraphFolder, GraphLink, GraphMailRequest, GraphMailResult};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn run_of(server: &StandIn, request: &GraphMailRequest) -> Vec<GraphMailResult> {
    whole_run(&server.url(), TOKEN, request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false))
}

#[test]
fn the_inbox_delta_is_asked_before_any_folder_walk_and_the_run_after_it_reads_nothing_of_the_inbox() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 5, NOW);
    server.state().add_folder("Projects", None);
    let before = server.state().requests.len();
    // The pass and then the run of the same round, as the window starts them; the Inbox's id is
    // known, its delta never read.
    let folder = GraphFolder { id: inbox.clone(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
    let pass = GraphMailRequest { account: 3, email: "owner@example.org".into(), inbox_pass: Some(folder), ..Default::default() };
    let pieces = run_of(&server, &pass);
    let got: usize = pieces.iter().flat_map(|p| &p.pages).map(|p| p.messages.len()).sum();
    assert_eq!(got, 5);
    let links = pieces
        .iter()
        .flat_map(|p| &p.pages)
        .map(|p| GraphLink { folder: p.folder.clone(), link: p.link.clone(), caught_up: p.caught_up, counts: p.counts })
        .collect();
    let rest = GraphMailRequest { account: 3, email: "owner@example.org".into(), links, ..Default::default() };
    let run = run_of(&server, &rest);
    let seen: Vec<String> = server.state().requests[before..].iter().map(|r| format!("{} {}", r.method, r.target)).collect();
    let first_walk = seen.iter().position(|r| !r.contains("/messages/delta")).unwrap();
    // Every request before the first housekeeping request (folders, well-known names) is the Inbox's delta, and there is one.
    assert!(first_walk >= 1 && seen[..first_walk].iter().all(|r| r.contains("/messages/delta")), "{seen:?}");
    let again: usize = run.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).map(|p| p.messages.len()).sum();
    assert_eq!(again, 0);
}
