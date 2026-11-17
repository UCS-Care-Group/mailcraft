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

#[test]
fn a_window_of_more_than_graphs_filtered_delta_limit_is_listed_whole_by_a_plain_list() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    // More mail in the window than a filtered delta gives (5,000), and one message outside it.
    server.state().mailbox.generate(&inbox, 5_150, NOW);
    server.state().mailbox.add(&inbox, "Last year", NOW - 400 * DAY);
    let pieces = run(&server, Some(NOW - 30 * DAY), Vec::new());
    assert!(pieces.last().unwrap().whole, "{:?}", pieces.last().unwrap().errors);
    let subjects = listed(&pieces, &inbox);
    let distinct: std::collections::HashSet<&String> = subjects.iter().collect();
    assert_eq!(distinct.len(), 5_150, "every message of the window comes, not only the first 5,000");
    assert!(!subjects.iter().any(|s| s == "Last year"));
    // The folder goes on from the round's delta link, given once the plain list had ended.
    let pages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).collect();
    let end = pages.last().unwrap();
    assert!(end.caught_up && end.link.contains("deltatoken"), "{}", end.link);
    assert!(pages.iter().filter(|p| p.caught_up).count() == 1, "only the last page carries the delta link");
    assert!(pages.iter().all(|p| p.link.is_empty() || p.link.contains("skiptoken") || p.link.contains("deltatoken")));
    // New mail then comes by that link, and nothing is listed again.
    server.state().mailbox.add(&inbox, "Today", NOW);
    let again = run(&server, Some(NOW - 30 * DAY), links_after(&pieces));
    assert_eq!(listed(&again, &inbox), ["Today"]);
}

#[test]
fn a_window_under_the_limit_is_not_listed_twice() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 250, NOW);
    let pieces = run(&server, Some(NOW - 30 * DAY), Vec::new());
    assert_eq!(listed(&pieces, &inbox).len(), 250);
}

#[test]
fn a_delta_link_from_a_wider_window_does_not_admit_mail_outside_a_narrower_one() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let old = {
        let mut state = server.state();
        state.mailbox.add(&inbox, "This week", NOW - 2 * DAY);
        state.mailbox.add(&inbox, "Two months ago", NOW - 60 * DAY)
    };
    let pieces = run(&server, Some(NOW - 90 * DAY), Vec::new());
    assert_eq!(listed(&pieces, &inbox).len(), 2);
    // The window is made a month: the kept link still names the three months, and the older
    // message changing is no reason to take it in.
    server.state().mailbox.update(&old, |m| m.read = !m.read);
    server.state().mailbox.add(&inbox, "Today", NOW);
    let again = run(&server, Some(NOW - 30 * DAY), links_after(&pieces));
    assert_eq!(listed(&again, &inbox), ["Today"]);
}

#[test]
fn the_first_page_of_a_listing_afresh_says_which_request_it_answers() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.add(&inbox, "This week", NOW - 2 * DAY);
    let links = vec![GraphLink { folder: inbox.clone(), afresh: true, relist_gen: 7, ..Default::default() }];
    let pieces = run(&server, Some(NOW - 30 * DAY), links);
    let pages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).collect();
    assert!(pages[0].afresh);
    assert_eq!(pages[0].relist_gen, 7);
    assert!(pages.iter().skip(1).all(|p| p.relist_gen == 0 && !p.afresh));
}
