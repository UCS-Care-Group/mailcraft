// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #276: folders whose counts haven't changed are passed over, through the strict Graph
//! stand-in. Nothing here leaves this machine; every message is invented.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mailcraft_model::sync::{GraphLink, GraphMailRequest, GraphMailResult};

use super::whole_run;
use crate::graph::pace::{Gate, Retry};
use crate::graph::standin::{StandIn, TOKEN};

const QUICK: Retry = Retry { tries: 4, first_wait: Duration::from_millis(5), longest_wait: Duration::from_secs(60) };
const NOW: i64 = 1_790_000_000;

fn request(links: Vec<GraphLink>, full: bool) -> GraphMailRequest {
    GraphMailRequest { account: 3, email: "owner@example.org".into(), links, full, ..Default::default() }
}

fn run(server: &StandIn, links: Vec<GraphLink>, full: bool) -> Vec<GraphMailResult> {
    whole_run(&server.url(), TOKEN, &request(links, full), Arc::new(Gate::new()), QUICK, &AtomicBool::new(false))
}

/// The links a client keeps after `pieces`, as the mailbox file would: each folder's latest, with
/// the counts that came with it.
fn links_after(pieces: &[GraphMailResult], mut links: Vec<GraphLink>) -> Vec<GraphLink> {
    for page in pieces.iter().flat_map(|p| &p.pages) {
        links.retain(|l| l.folder != page.folder);
        links.push(GraphLink { folder: page.folder.clone(), link: page.link.clone(), caught_up: page.caught_up, counts: page.counts });
    }
    links
}

/// How many delta requests were made for `folder` since request number `from`.
fn deltas_of(server: &StandIn, folder: &str, from: usize) -> usize {
    let is = |r: &&crate::graph::standin::Asked| r.path.last().is_some_and(|p| p == "delta") && r.path.iter().any(|p| p == folder);
    server.state().requests[from..].iter().filter(is).count()
}

/// The messages that came for `folder`, by id.
fn messages_of(pieces: &[GraphMailResult], folder: &str) -> Vec<String> {
    pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == folder).flat_map(|p| &p.messages).map(|m| m.server.id.clone()).collect()
}

/// A mailbox with mail in the Inbox and in two folders of the owner's, read once to its end.
struct Rig {
    server: StandIn,
    inbox: String,
    projects: String,
    receipts: String,
    links: Vec<GraphLink>,
}

fn rig() -> Rig {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let projects = server.state().add_folder("Projects", None);
    let receipts = server.state().add_folder("Receipts", None);
    {
        let mut state = server.state();
        state.mailbox.generate(&inbox, 12, NOW);
        state.mailbox.generate(&projects, 7, NOW);
        state.mailbox.generate(&receipts, 5, NOW);
    }
    let first = run(&server, Vec::new(), true);
    assert!(first.last().unwrap().whole && first.last().unwrap().timings.skipped == 0);
    let links = links_after(&first, Vec::new());
    // Every round ended in this run: its link has the listing's counts.
    assert!(links.iter().all(|l| l.caught_up && l.counts.is_some()), "{links:?}");
    let projects_counts = links.iter().find(|l| l.folder == projects).and_then(|l| l.counts).unwrap();
    assert_eq!((projects_counts.total, projects_counts.unread), (7, 7));
    Rig { server, inbox, projects, receipts, links }
}

#[test]
fn folders_of_the_owners_whose_counts_are_the_same_are_passed_over_and_empty_pages_not_handed_on() {
    let r = rig();
    let before = r.server.state().requests.len();
    let pieces = run(&r.server, r.links.clone(), false);
    let last = pieces.last().unwrap();
    assert!(last.whole && last.errors.is_empty(), "{:?}", last.errors);
    assert_eq!(last.timings.skipped, 3);
    assert_eq!((deltas_of(&r.server, &r.projects, before), deltas_of(&r.server, &r.receipts, before)), (0, 0));
    // The Inbox and the other special folders are always read; nothing came, so no page.
    assert_eq!(deltas_of(&r.server, &r.inbox, before), 1);
    assert!(pieces.iter().all(|p| p.pages.is_empty()), "{pieces:?}");
}

#[test]
fn a_folder_that_changed_is_read_and_its_counts_move_on() {
    let r = rig();
    // New mail in Projects; a message read in Receipts.
    let fresh = r.server.state().mailbox.add(&r.projects, "Fresh", NOW + 60);
    let some_receipt = r.server.state().mailbox.messages.values().find(|m| m.folder == r.receipts).unwrap().id.clone();
    r.server.state().mailbox.update(&some_receipt, |m| m.read = true);
    let pieces = run(&r.server, r.links.clone(), false);
    assert_eq!(pieces.last().unwrap().timings.skipped, 1);
    assert_eq!(messages_of(&pieces, &r.projects), [fresh]);
    let receipts = pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == r.receipts).count();
    assert_eq!(receipts, 1, "the read state came");
    let links = links_after(&pieces, r.links.clone());
    let counts = |folder: &str| links.iter().find(|l| l.folder == folder).and_then(|l| l.counts).map(|c| (c.total, c.unread));
    assert_eq!((counts(&r.projects), counts(&r.receipts)), (Some((8, 8)), Some((5, 4))));
    // Nothing more: both passed over again.
    let again = run(&r.server, links, false);
    assert_eq!(again.last().unwrap().timings.skipped, 3);
}

/// Counts can hide a change (here a flag, which moves neither count): a folder passed over
/// misses it, and the full sweep, from the same link, brings it. Late, never lost.
#[test]
fn what_leaves_the_counts_as_they_were_comes_with_the_full_sweep() {
    let r = rig();
    let flagged = r.server.state().mailbox.messages.values().find(|m| m.folder == r.projects).unwrap().id.clone();
    r.server.state().mailbox.update(&flagged, |m| m.flag = "flagged".into());
    let skipping = run(&r.server, r.links.clone(), false);
    assert_eq!(skipping.last().unwrap().timings.skipped, 3);
    assert!(messages_of(&skipping, &r.projects).is_empty());
    let links = links_after(&skipping, r.links.clone());
    let before = r.server.state().requests.len();
    let sweep = run(&r.server, links, true);
    let last = sweep.last().unwrap();
    assert!(last.whole && last.timings.skipped == 0);
    assert_eq!(deltas_of(&r.server, &r.receipts, before), 1, "every folder is read");
    assert_eq!(messages_of(&sweep, &r.projects), [flagged]);
}

/// Mail moved into a folder whose counts stay the same (one in, one deleted there): the folder
/// it left is read and says it went; it turned up in no folder read, so the folders passed over
/// are read after all, and it is found where it went before the run is whole.
#[test]
fn mail_that_left_a_folder_for_one_passed_over_is_found_there() {
    let r = rig();
    let (moved, deleted) = {
        let state = r.server.state();
        let of = |folder: &str| state.mailbox.messages.values().find(|m| m.folder == folder).unwrap().id.clone();
        (of(&r.projects), of(&r.receipts))
    };
    {
        let mut state = r.server.state();
        state.mailbox.delete(&deleted);
        state.mailbox.move_to(&moved, &r.receipts);
        assert_eq!(state.mailbox.counts(&r.receipts), (5, 5), "Receipts' counts are as they were");
    }
    let pieces = run(&r.server, r.links.clone(), false);
    let last = pieces.last().unwrap();
    assert!(last.whole && last.timings.skipped == 0, "{:?}", last.timings);
    let removed: Vec<&String> = pieces.iter().flat_map(|p| &p.pages).filter(|p| p.folder == r.projects).flat_map(|p| &p.removed).collect();
    assert_eq!(removed, [&moved]);
    assert_eq!(messages_of(&pieces, &r.receipts), [moved]);
}

/// A move between folders read (here into the Inbox) is explained by the run itself: the folders
/// passed over stay passed over.
#[test]
fn mail_that_turned_up_in_a_folder_read_leaves_the_others_passed_over() {
    let r = rig();
    let moved = r.server.state().mailbox.messages.values().find(|m| m.folder == r.projects).unwrap().id.clone();
    r.server.state().mailbox.move_to(&moved, &r.inbox);
    let before = r.server.state().requests.len();
    let pieces = run(&r.server, r.links.clone(), false);
    let last = pieces.last().unwrap();
    assert!(last.whole);
    assert_eq!(last.timings.skipped, 2, "Receipts");
    assert_eq!(deltas_of(&r.server, &r.receipts, before), 0);
    assert_eq!(messages_of(&pieces, &r.inbox), [moved]);
}

/// A round begun in an earlier run (stopped half-way) ends without counts: what changed before
/// this run's listing may not be in it. The round begun after it ends with them.
#[test]
fn a_round_begun_in_an_earlier_run_ends_without_counts() {
    let server = StandIn::start().unwrap();
    let projects = server.state().add_folder("Projects", None);
    server.state().mailbox.generate(&projects, 250, NOW);
    let cancel = AtomicBool::new(false);
    let mut pieces = Vec::new();
    super::sync_mail_with(&server.url(), TOKEN, &request(Vec::new(), true), Arc::new(Gate::new()), QUICK, &cancel, &mut |piece| {
        if piece.pages.iter().any(|p| p.folder == projects) {
            cancel.store(true, Ordering::Relaxed);
        }
        pieces.push(piece);
    });
    let links = links_after(&pieces, Vec::new());
    let mid = links.iter().find(|l| l.folder == projects).unwrap();
    assert!(!mid.caught_up && mid.counts.is_none());
    let rest = run(&server, links, false);
    let ends: Vec<_> = rest.iter().flat_map(|p| &p.pages).filter(|p| p.folder == projects && p.caught_up).map(|p| p.counts).collect();
    assert_eq!(ends.len(), 2, "the old round's end, then the new one's");
    assert!(ends[0].is_none() && ends[1].is_some(), "{ends:?}");
}

/// A link kept without counts (a mailbox from before, or a round begun earlier) is never passed
/// over, and its page is handed on even when it brings nothing, so that the counts are kept.
#[test]
fn a_link_without_counts_is_read_and_its_page_handed_on() {
    let r = rig();
    let bare: Vec<GraphLink> = r.links.iter().map(|l| GraphLink { counts: None, ..l.clone() }).collect();
    let pieces = run(&r.server, bare, false);
    assert_eq!(pieces.last().unwrap().timings.skipped, 0);
    let pages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).collect();
    assert!(pages.iter().any(|p| p.folder == r.projects && p.counts.is_some() && p.messages.is_empty()));
    assert!(pages.iter().any(|p| p.folder == r.inbox && p.counts.is_some()));
}

/// A delta link that is a next page (a round under way) is read on, whatever its counts.
#[test]
fn a_folder_with_a_round_under_way_is_never_passed_over() {
    let r = rig();
    let pretend: Vec<GraphLink> = r.links.iter().map(|l| GraphLink { caught_up: l.folder != r.projects, ..l.clone() }).collect();
    let before = r.server.state().requests.len();
    let pieces = run(&r.server, pretend, false);
    assert_eq!(pieces.last().unwrap().timings.skipped, 2, "Receipts alone");
    assert!(deltas_of(&r.server, &r.projects, before) >= 1);
}

/// The Inbox pass never passes anything over, and gives no counts: it lists nothing.
#[test]
fn the_inbox_pass_gives_no_counts() {
    use mailcraft_model::FolderKind;
    use mailcraft_model::sync::GraphFolder;
    let r = rig();
    r.server.state().mailbox.add(&r.inbox, "Fresh", NOW + 60);
    let folder = GraphFolder { id: r.inbox.clone(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
    let links = r.links.iter().filter(|l| l.folder == r.inbox).cloned().collect();
    let asked = GraphMailRequest { inbox_pass: Some(folder), ..request(links, false) };
    let pieces = whole_run(&r.server.url(), TOKEN, &asked, Arc::new(Gate::new()), QUICK, &AtomicBool::new(false));
    let pages: Vec<_> = pieces.iter().flat_map(|p| &p.pages).collect();
    assert_eq!(pages.len(), 1);
    assert!(pages[0].counts.is_none());
    assert_eq!(pieces.last().unwrap().timings.skipped, 0);
}

/// A listing that doesn't give a folder's counts says nothing of whether it changed: read as 0,
/// they would match counts of 0 kept with its link. Such a folder is read, and its link keeps none.
#[test]
fn a_folder_listed_without_its_counts_is_never_passed_over() {
    let r = rig();
    let zero = Some(mailcraft_model::FolderCounts { total: 0, unread: 0 });
    let links: Vec<GraphLink> = r.links.iter().cloned().map(|l| GraphLink { counts: zero, ..l }).collect();
    r.server.state().uncounted = true;
    let before = r.server.state().requests.len();
    let pieces = run(&r.server, links, false);
    let last = pieces.last().unwrap();
    assert!(last.whole && last.errors.is_empty(), "{:?}", last.errors);
    assert_eq!(last.timings.skipped, 0);
    assert_eq!((deltas_of(&r.server, &r.projects, before), deltas_of(&r.server, &r.receipts, before)), (1, 1));
    assert!(pieces.iter().flat_map(|p| &p.pages).all(|p| p.counts.is_none()), "{pieces:?}");
}

/// A listing cut short (#305): the folders it missed aren't read, so what left a folder read may
/// be in one of them. The run reads what it listed and isn't whole; nor is it whole when it reads
/// everything, folders passed over included.
#[test]
fn a_run_whose_listing_was_cut_short_is_never_whole() {
    let r = rig();
    let apollo = r.server.state().add_folder("Apollo", Some(&r.projects));
    let moved = r.server.state().mailbox.generate(&r.inbox, 1, NOW + 60);
    let first = run(&r.server, r.links.clone(), true);
    assert!(first.last().unwrap().whole);
    let links = links_after(&first, r.links.clone());
    {
        let mut state = r.server.state();
        for id in moved {
            assert!(state.mailbox.move_to(&id, &apollo));
        }
        state.broken_lists.push(r.projects.clone());
    }
    for full in [false, true] {
        let pieces = run(&r.server, links.clone(), full);
        assert!(pieces.first().and_then(|p| p.listing.as_ref()).is_some_and(|l| !l.complete));
        let last = pieces.last().unwrap();
        assert!(!last.whole && !last.partial, "full: {full}");
        assert!(messages_of(&pieces, &apollo).is_empty());
    }
}
