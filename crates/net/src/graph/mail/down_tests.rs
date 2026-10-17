//! A Microsoft account's mail down through the strict Graph stand-in (R3.2). Nothing here leaves
//! this machine; every message is invented.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_model::sync::{GraphLink, GraphMailRequest, GraphMailResult};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn request(links: Vec<GraphLink>) -> GraphMailRequest {
    GraphMailRequest { account: 3, email: "owner@example.org".into(), links, ..Default::default() }
}

fn run(server: &StandIn, links: Vec<GraphLink>, cancel: &AtomicBool) -> Vec<GraphMailResult> {
    whole_run(&server.url(), TOKEN, &request(links), Arc::new(Gate::new()), QUICK, cancel)
}

/// The links a client keeps after `pieces`, as the mailbox file would: each folder's latest.
fn links_after(pieces: &[GraphMailResult], mut links: Vec<GraphLink>) -> Vec<GraphLink> {
    for page in pieces.iter().flat_map(|p| &p.pages) {
        links.retain(|l| l.folder != page.folder);
        links.push(GraphLink { folder: page.folder.clone(), link: page.link.clone(), caught_up: page.caught_up });
    }
    links
}

/// The pages' folders in the order they came.
fn folders_in_order(pieces: &[GraphMailResult]) -> Vec<String> {
    pieces.iter().flat_map(|p| &p.pages).map(|p| p.folder.clone()).collect()
}

#[test]
fn the_folders_come_first_then_the_newest_page_of_every_folder_then_the_rest() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let archive = server.state().well_known("archive");
    let projects = server.state().add_folder("Projects", None);
    {
        let mut state = server.state();
        state.mailbox.generate(&inbox, 250, NOW);
        state.mailbox.generate(&archive, 150, NOW);
        state.mailbox.generate(&projects, 5, NOW);
    }
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let first = pieces.first().unwrap();
    assert!(first.listing.is_some() && first.partial && first.pages.is_empty());
    let last = pieces.last().unwrap();
    assert!(!last.partial && last.whole && last.errors.is_empty(), "{:?}", last.errors);
    let order = folders_in_order(&pieces);
    // Inbox, Archive and Projects once each (and the empty special folders), then the rest of the
    // Inbox and the Archive.
    let at = |id: &str| order.iter().position(|f| f == id).unwrap();
    assert!(at(&inbox) < at(&archive) && at(&archive) < at(&projects));
    let second_inbox = order.iter().enumerate().filter(|(_, f)| **f == inbox).nth(1).map(|(n, _)| n).unwrap();
    assert!(second_inbox > at(&projects), "{order:?}");
    // Every message once, newest first in its folder; the Inbox's counts for progress.
    let inbox_messages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == inbox).flat_map(|p| &p.messages).collect();
    assert_eq!(inbox_messages.len(), 250);
    assert!(inbox_messages.windows(2).all(|w| w[0].received >= w[1].received));
    assert_eq!(inbox_messages[0].subject, "Message 0");
    assert_eq!(inbox_messages[0].server.folder, inbox);
    let progress = pieces.iter().filter_map(|p| p.progress.as_ref()).find(|p| p.folder == inbox).unwrap();
    assert_eq!(progress.total, 250);
    // Every folder's last page ends its round with a delta link.
    let links = links_after(&pieces, Vec::new());
    assert!(links.iter().all(|l| l.caught_up && l.link.contains("$deltatoken=")));

    // Run again from the links: nothing but what changed.
    let gone = server.state().mailbox.messages.values().find(|m| m.folder == inbox).unwrap().id.clone();
    server.state().mailbox.delete(&gone);
    let again = run(&server, links, &AtomicBool::new(false));
    let pages: Vec<_> = again.iter().flat_map(|p| &p.pages).collect();
    assert_eq!(pages.iter().map(|p| p.messages.len()).sum::<usize>(), 0);
    assert_eq!(pages.iter().flat_map(|p| &p.removed).collect::<Vec<_>>(), [&gone]);
    assert!(again.last().unwrap().whole);
}

#[test]
fn cancel_stops_after_the_page_in_hand_and_the_next_run_goes_on_from_there() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 500, NOW);
    let cancel = AtomicBool::new(false);
    let mut pieces = Vec::new();
    super::sync_mail_with(&server.url(), TOKEN, &request(Vec::new()), Arc::new(Gate::new()), QUICK, &cancel, &mut |piece| {
        // Stop when the second page of the Inbox is in hand.
        if piece.pages.iter().filter(|p| p.folder == inbox).count() > 0
            && pieces.iter().flat_map(|p: &GraphMailResult| &p.pages).filter(|p| p.folder == inbox).count() == 1
        {
            cancel.store(true, Ordering::Relaxed);
        }
        pieces.push(piece);
    });
    let last = pieces.last().unwrap();
    assert!(!last.whole && !last.partial);
    assert!(last.stopped && last.errors.is_empty());
    let got: usize = pieces.iter().flat_map(|p| &p.pages).map(|p| p.messages.len()).sum();
    assert_eq!(got, 200);
    let links = links_after(&pieces, Vec::new());
    let rest = run(&server, links, &AtomicBool::new(false));
    let more: Vec<String> = rest.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).map(|m| m.server.id.clone()).collect();
    assert_eq!(more.len(), 300, "nothing twice, nothing missed");
    assert!(rest.last().unwrap().whole);
}

#[test]
fn a_link_no_longer_taken_or_not_graphs_own_starts_the_folder_afresh() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 30, NOW);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let links = links_after(&pieces, Vec::new());

    server.state().mailbox.forget_links();
    let again = run(&server, links.clone(), &AtomicBool::new(false));
    let first_inbox = again.iter().flat_map(|p| &p.pages).find(|p| p.folder == inbox).unwrap();
    assert!(first_inbox.afresh && first_inbox.messages.len() == 30);
    assert!(again.last().unwrap().whole);

    server.state().mailbox.resets = 1;
    let links = links_after(&again, links);
    let reset = run(&server, links.clone(), &AtomicBool::new(false));
    assert!(reset.iter().flat_map(|p| &p.pages).any(|p| p.folder == inbox && p.afresh));

    // A link the mailbox file says goes elsewhere is never followed.
    let astray: Vec<GraphLink> = links.into_iter().map(|l| GraphLink { link: l.link.replace("127.0.0.1", "graph.example.org"), ..l }).collect();
    let before = server.state().requests.len();
    let fresh = run(&server, astray, &AtomicBool::new(false));
    assert!(fresh.iter().flat_map(|p| &p.pages).any(|p| p.folder == inbox && p.afresh));
    assert!(server.state().requests[before..].iter().all(|r| !r.target.contains("graph.example.org")));
}

#[test]
fn a_busy_server_ends_the_run_with_how_long_to_wait() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 30, NOW);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let links = links_after(&pieces, Vec::new());
    {
        let mut state = server.state();
        state.retry_after = Some(600);
        for _ in 0..10 {
            state.fail_next.push((format!("GET /v1.0/me/mailFolders/{inbox}/messages/delta"), 429, "TooManyRequests".into()));
        }
    }
    let busy = run(&server, links, &AtomicBool::new(false));
    let last = busy.last().unwrap();
    assert!(!last.whole);
    assert!(last.busy_for.is_some_and(|seconds| seconds >= 590), "{last:?}");
}

#[test]
fn a_round_begun_in_an_earlier_run_is_followed_by_one_that_brings_what_changed_since() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 300, NOW);
    let cancel = AtomicBool::new(false);
    let mut pieces = Vec::new();
    super::sync_mail_with(&server.url(), TOKEN, &request(Vec::new()), Arc::new(Gate::new()), QUICK, &cancel, &mut |piece| {
        if piece.pages.iter().any(|p| p.folder == inbox) {
            cancel.store(true, Ordering::Relaxed);
        }
        pieces.push(piece);
    });
    // The newest came; then it is deleted there.
    assert!(pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).any(|m| m.server.id == ids[0]));
    server.state().mailbox.delete(&ids[0]);
    let links = links_after(&pieces, Vec::new());
    let rest = run(&server, links, &AtomicBool::new(false));
    assert!(rest.last().unwrap().whole);
    assert!(rest.iter().flat_map(|p| &p.pages).any(|p| p.removed.contains(&ids[0])), "the deletion comes in the round after");
}

#[test]
fn messages_come_without_their_size_and_are_fetched_whole_by_their_immutable_id() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let ids = server.state().mailbox.generate(&inbox, 3, NOW);
    server.state().mailbox.update(&ids[1], |m| m.size = 30 << 20);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let sizes: Vec<u64> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).map(|m| m.server.size).collect();
    // A delta can't be asked for the sizes: none is said.
    assert_eq!(sizes, [0, 0, 0]);

    server.state().mailbox.delete(&ids[2]);
    let request = mailcraft_model::sync::GraphFetchRequest {
        account: 3,
        email: "owner@example.org".into(),
        ids: ids.clone(),
        background: true,
        ..Default::default()
    };
    let got = super::fetch::fetch_messages_with(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    assert_eq!(got.fetched.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), [ids[0].as_str(), ids[1].as_str()]);
    assert!(String::from_utf8_lossy(&got.fetched[0].mime).contains("Subject: Message 0\r\n"));
    assert_eq!(got.missing, [ids[2].clone()]);
    assert!(got.errors.is_empty() && !got.offline);
    // Every one asked for by its immutable id.
    assert!(server.state().requests.iter().filter(|r| r.target.contains("$value")).all(|r| r.header("prefer").contains("ImmutableId")));

    // Cancel: nothing more is asked.
    let cancelled = super::fetch::fetch_messages_with(&server.url(), TOKEN, &request, Arc::new(Gate::new()), QUICK, &AtomicBool::new(true));
    assert!(cancelled.fetched.is_empty());
}

#[test]
fn a_delta_is_never_asked_to_expand_the_sizes_which_exchange_online_refuses() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    server.state().mailbox.generate(&inbox, 3, NOW);
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let last = pieces.last().unwrap();
    assert!(!last.partial && last.whole && last.errors.is_empty(), "{:?}", last.errors);
    let messages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).flat_map(|p| &p.messages).collect();
    assert_eq!(messages.len(), 3);
    assert!(messages.iter().all(|m| m.server.size == 0), "a size not said is 0");
    let deltas = server.state().requests.iter().filter(|r| r.target.contains("/messages/delta")).count();
    assert!(deltas > 1 && server.state().requests.iter().all(|r| !(r.target.contains("/messages/delta") && r.target.contains("expand"))));
}

#[test]
fn folders_the_server_refuses_to_list_are_said_with_graphs_code() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    {
        let mut state = server.state();
        state.mailbox.generate(&inbox, 3, NOW);
        state.mailbox.refuse_delta = true;
    }
    let pieces = run(&server, Vec::new(), &AtomicBool::new(false));
    let last = pieces.last().unwrap();
    assert!(!last.partial && !last.whole);
    assert_eq!(last.errors.len(), 1, "{:?}", last.errors);
    let said = &last.errors[0];
    assert!(said.starts_with("Microsoft Graph refused to list the mail in ") && said.contains("folders (ErrorInvalidRequest)"), "{said}");
    assert!(!said.contains(&inbox), "no folder's id: {said}");
    assert!(pieces.iter().all(|p| p.pages.is_empty()));
}
