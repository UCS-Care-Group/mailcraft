// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A phone's window (MB3.16), the rounds that go on from a saved delta link and the plain list's
//! end: through the strict Graph stand-in; nothing leaves this machine and every message is invented.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use mailcraft_model::sync::{GraphLink, GraphMailRequest, GraphMailResult};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;
const DAY: i64 = 86_400;

fn run(server: &StandIn, links: Vec<GraphLink>) -> Vec<GraphMailResult> {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), links, since: Some(NOW - 30 * DAY), ..Default::default() };
    whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false))
}

fn links_after(pieces: &[GraphMailResult]) -> Vec<GraphLink> {
    let mut links: Vec<GraphLink> = Vec::new();
    for page in pieces.iter().flat_map(|p| &p.pages) {
        links.retain(|l| l.folder != page.folder);
        links.push(GraphLink {
            folder: page.folder.clone(),
            link: page.link.clone(),
            caught_up: page.caught_up,
            counts: page.counts,
            afresh: false,
            ..Default::default()
        });
    }
    links
}

#[test]
fn a_round_that_goes_on_from_a_saved_link_and_brings_more_than_the_limit_is_read_by_a_plain_list() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.add(&inbox, "Before", NOW - DAY);
    let first = run(&server, Vec::new());
    assert!(first.last().unwrap().whole, "{:?}", first.last().unwrap().errors);
    // Past what a filtered delta gives (5,000) arrives by the saved link.
    server.state().mailbox.generate(&inbox, 5_150, NOW);
    let again = run(&server, links_after(&first));
    assert!(again.last().unwrap().whole, "{:?}", again.last().unwrap().errors);
    let subjects: HashSet<String> =
        again.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).flat_map(|p| &p.messages).map(|m| m.subject.clone()).collect();
    assert_eq!(subjects.iter().filter(|s| s.starts_with("Message ")).count(), 5_150, "none of the new mail is left behind");
}

#[test]
fn a_plain_list_whose_next_link_can_t_be_followed_does_not_end_the_folder() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 5_150, NOW);
    server.state().mailbox.foreign_next = true;
    let pieces = run(&server, Vec::new());
    let pages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).collect();
    assert!(pages.iter().all(|p| !p.caught_up && !p.link.contains("deltatoken")), "the delta link held is not given before the list's end");
    assert!(!pieces.last().unwrap().whole);
}
