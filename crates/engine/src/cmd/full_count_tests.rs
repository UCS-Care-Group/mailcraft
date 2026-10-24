// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send/Receive Progress's count of full messages while a Graph run brings its pages: counted at
//! most once a second of the clock while the run goes on (each count goes through every message),
//! and when the account's part of the run ends, exactly what a count from the mailbox says.

use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, MessagePage};
use mailcraft_model::{FolderKind, Id, Mailbox};
use serde_json::json;

use super::graph_down::{self, FullCounts};
use super::sync_progress::{FullMessages, SyncProgress};
use crate::Session;

const NOW: i64 = 1_791_993_600;

/// A session whose clock stands still, with a Microsoft account whose Inbox is listed.
fn session() -> (Session, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
    graph(
        &mut s,
        GraphMailResult { account, listing: Some(FolderListing { folders: vec![inbox], complete: true }), partial: true, ..Default::default() },
    );
    (s, account)
}

fn graph(s: &mut Session, piece: GraphMailResult) {
    s.run("sync.applyGraph", &json!({"result": piece})).unwrap();
}

/// A page of three new messages, numbered from `first`.
fn page(first: usize) -> MessagePage {
    let messages: Vec<_> =
        (first..first + 3).map(|n| json!({"server": {"id": format!("AAA{n}"), "folder": "in"}, "subject": format!("Message {n}")})).collect();
    serde_json::from_value(json!({"folder": "in", "messages": messages})).unwrap()
}

fn piece(account: Id, first: usize) -> GraphMailResult {
    GraphMailResult { account, pages: vec![page(first)], partial: true, ..Default::default() }
}

/// What a count from the mailbox, now, says of the account's full messages.
fn counted_afresh(s: &Session, account: Id) -> Option<FullMessages> {
    let mut fresh = SyncProgress::default();
    fresh.count_full(&s.mailbox, account, s.now());
    fresh.full(account)
}

#[test]
fn counted_now_and_then_while_a_run_goes_on_and_exactly_when_it_ends() {
    let (mut s, account) = session();
    let start = s.sync_progress.recounts();
    // The first page is counted; another in the same second waits.
    graph(&mut s, piece(account, 0));
    assert_eq!(s.sync_progress.full(account), Some(FullMessages { done: 0, total: 3 }));
    graph(&mut s, piece(account, 3));
    assert_eq!(s.sync_progress.full(account), Some(FullMessages { done: 0, total: 3 }), "not counted again in the same second");
    assert_eq!(s.sync_progress.recounts() - start, 1);
    // A second later, the next page is counted with what came before it.
    s.clock.now = Some(NOW + 1);
    graph(&mut s, piece(account, 6));
    assert_eq!(s.sync_progress.full(account), Some(FullMessages { done: 0, total: 9 }));
    graph(&mut s, piece(account, 9));
    assert_eq!(s.sync_progress.recounts() - start, 2);
    // Stopped (the last piece brings no page): the page not counted yet is counted now.
    graph(&mut s, GraphMailResult { account, stopped: true, ..Default::default() });
    assert_eq!(s.sync_progress.full(account), Some(FullMessages { done: 0, total: 12 }));
    assert_eq!(s.sync_progress.full(account), counted_afresh(&s, account));
    assert_eq!(s.sync_progress.recounts() - start, 3);
    // Ended with nothing to count again: not counted.
    graph(&mut s, GraphMailResult { account, ..Default::default() });
    assert_eq!(s.sync_progress.recounts() - start, 3);

    // A message fetched whole: counted in the same pass that answers how many wait.
    let mime = b"Subject: Message 0\r\n\r\nWords.\r\n".to_vec();
    let fetched = s.run("sync.applyGraphFetch", &json!({"result": {"account": account, "fetched": [{"id": "AAA0", "mime": mime}]}})).unwrap();
    let counts = graph_down::full_counts(&s.mailbox, account, s.now());
    assert_eq!(counts, FullCounts { all: 12, waiting: 11, large: 0 });
    assert_eq!((fetched["all"].as_u64(), fetched["waiting"].as_u64(), fetched["large"].as_u64()), (Some(12), Some(11), Some(0)));
    assert_eq!(s.sync_progress.full(account), Some(FullMessages { done: 1, total: 12 }));
    assert_eq!(s.sync_progress.recounts() - start, 4);

    // The next run carries on: many pages in one second are one count, and the run's end (the
    // mailbox read whole) another, with what a count from the mailbox says.
    s.clock.now = Some(NOW + 2);
    let before = s.sync_progress.recounts();
    for n in 0..50 {
        graph(&mut s, piece(account, 100 + n * 3));
    }
    assert_eq!(s.sync_progress.recounts() - before, 1, "not counted after every page");
    graph(&mut s, GraphMailResult { account, pages: vec![page(1_000)], whole: true, ..Default::default() });
    assert_eq!(s.sync_progress.recounts() - before, 2);
    assert_eq!(s.sync_progress.full(account), Some(FullMessages { done: 1, total: 12 + 150 + 3 }));
    assert_eq!(s.sync_progress.full(account), counted_afresh(&s, account));
}

#[test]
fn a_page_counted_late_is_counted_when_a_host_ends_the_run() {
    let (mut s, account) = session();
    graph(&mut s, piece(account, 0));
    graph(&mut s, piece(account, 3));
    assert_eq!(s.sync_progress.full(account), Some(FullMessages { done: 0, total: 3 }));
    // The host gives up on the account's part of the run: it counts what is due first.
    let now = s.now();
    s.sync_progress.count_full_if_due(&s.mailbox.clone(), account, now);
    s.sync_progress.end(account, true);
    assert_eq!(s.sync_progress.full(account), Some(FullMessages { done: 0, total: 6 }));
    assert_eq!(s.sync_progress.full(account), counted_afresh(&s, account));
    // Nothing due: nothing counted.
    let before = s.sync_progress.recounts();
    s.sync_progress.count_full_if_due(&s.mailbox.clone(), account, now);
    assert_eq!(s.sync_progress.recounts(), before);
}
