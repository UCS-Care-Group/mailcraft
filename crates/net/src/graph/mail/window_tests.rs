// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A phone's window (MB3.16): a folder listed from its start lists only mail received within it,
//! and a folder to be listed afresh is, whatever link it had. Through the strict Graph stand-in;
//! nothing leaves this machine and every message is invented.

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

fn run(server: &StandIn, since: Option<i64>, links: Vec<GraphLink>) -> Vec<GraphMailResult> {
    let request = GraphMailRequest { account: 3, email: "owner@example.org".into(), links, since, ..Default::default() };
    whole_run(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false))
}

fn listed(pieces: &[GraphMailResult], folder: &str) -> Vec<String> {
    pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == folder).flat_map(|p| &p.messages).map(|m| m.subject.clone()).collect()
}

fn links_after(pieces: &[GraphMailResult]) -> Vec<GraphLink> {
    let mut links: Vec<GraphLink> = Vec::new();
    for page in pieces.iter().flat_map(|p| &p.pages) {
        links.retain(|l| l.folder != page.folder);
        links.push(GraphLink { folder: page.folder.clone(), link: page.link.clone(), caught_up: page.caught_up, counts: page.counts, afresh: false });
    }
    links
}

#[test]
fn a_window_lists_only_its_mail_and_its_delta_link_keeps_to_it() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    {
        let mut state = server.state();
        state.mailbox.add(&inbox, "This week", NOW - 2 * DAY);
        state.mailbox.add(&inbox, "Last year", NOW - 400 * DAY);
    }
    let pieces = run(&server, Some(NOW - 30 * DAY), Vec::new());
    assert!(pieces.last().unwrap().whole, "{:?}", pieces.last().unwrap().errors);
    assert_eq!(listed(&pieces, &inbox), ["This week"]);
    // New mail comes by the link; mail older than the window, new to the server, never does.
    {
        let mut state = server.state();
        state.mailbox.add(&inbox, "Today", NOW);
        state.mailbox.add(&inbox, "Old, filed late", NOW - 300 * DAY);
    }
    let again = run(&server, Some(NOW - 30 * DAY), links_after(&pieces));
    assert_eq!(listed(&again, &inbox), ["Today"]);
}

#[test]
fn a_computer_lists_everything() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.add(&inbox, "Last year", NOW - 400 * DAY);
    assert_eq!(listed(&run(&server, None, Vec::new()), &inbox), ["Last year"]);
}

#[test]
fn a_folder_to_be_listed_afresh_is_listed_from_its_start_with_the_new_window() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    {
        let mut state = server.state();
        state.mailbox.add(&inbox, "This week", NOW - 2 * DAY);
        state.mailbox.add(&inbox, "Two months ago", NOW - 60 * DAY);
    }
    let pieces = run(&server, Some(NOW - 30 * DAY), Vec::new());
    assert_eq!(listed(&pieces, &inbox), ["This week"]);
    // The window made three months: the folder is listed afresh, its link not followed.
    let links: Vec<GraphLink> =
        links_after(&pieces).into_iter().map(|l| if l.folder == inbox { GraphLink { afresh: true, ..l } } else { l }).collect();
    let again = run(&server, Some(NOW - 90 * DAY), links);
    let first = again.iter().flat_map(|p| &p.pages).find(|p| p.folder == inbox).unwrap();
    assert!(first.afresh);
    assert_eq!(listed(&again, &inbox), ["This week", "Two months ago"]);
}
