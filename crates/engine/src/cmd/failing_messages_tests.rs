// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1581: a message that failed to download is counted in the runs it failed in, handed to the
//! next request with its count, forgotten once it comes, and taken up again on the owner's word.

use mailcraft_model::sync::{FolderDelta, MAX_FAILING, MAX_TRIES, SyncResult};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::Session;
use crate::cmd::account::{apply_sync, sync_request};

fn account(s: &mut Session) -> Id {
    let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let a = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    apply_sync(s, SyncResult { account: a, folders: vec![delta(7, vec![], vec![])], ..Default::default() }).unwrap();
    a
}

fn delta(validity: u32, new: Vec<u32>, failed: Vec<u32>) -> FolderDelta {
    let new = new.into_iter().map(|uid| mailcraft_model::Message { uid, ..Default::default() }).collect();
    FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: validity, uid_seen: 1, new, failed, ..Default::default() }
}

fn apply(s: &mut Session, account: Id, delta: FolderDelta) {
    apply_sync(s, SyncResult { account, folders: vec![delta], ..Default::default() }).unwrap();
}

fn failing(s: &Session, account: Id) -> Vec<(u32, u8)> {
    let asked = sync_request(s, account).unwrap();
    asked.folders.iter().flat_map(|f| f.failing.iter().map(|(uid, tries)| (*uid, *tries))).collect()
}

#[test]
fn a_failing_message_is_counted_per_run_up_to_the_limit() {
    let mut s = Session::empty();
    let a = account(&mut s);
    assert_eq!(failing(&s, a), []);
    for run in 1..=MAX_TRIES + 2 {
        apply(&mut s, a, delta(7, vec![], vec![4]));
        assert_eq!(failing(&s, a), [(4, run.min(MAX_TRIES))]);
    }
}

#[test]
fn a_message_that_comes_or_goes_is_forgotten() {
    let mut s = Session::empty();
    let a = account(&mut s);
    apply(&mut s, a, delta(7, vec![], vec![4, 5, 6]));
    apply(&mut s, a, delta(7, vec![4], vec![]));
    assert_eq!(failing(&s, a), [(5, 1), (6, 1)]);
    let mut gone = delta(7, vec![], vec![]);
    gone.removed = vec![5];
    apply(&mut s, a, gone);
    assert_eq!(failing(&s, a), [(6, 1)]);
}

#[test]
fn numbered_anew_the_counts_mean_nothing() {
    let mut s = Session::empty();
    let a = account(&mut s);
    apply(&mut s, a, delta(7, vec![], vec![4]));
    apply(&mut s, a, delta(8, vec![], vec![]));
    assert_eq!(failing(&s, a), []);
}

#[test]
fn what_failed_in_the_run_that_found_the_folder_numbered_anew_is_counted() {
    let mut s = Session::empty();
    let a = account(&mut s);
    apply(&mut s, a, delta(7, vec![], vec![4]));
    apply(&mut s, a, delta(8, vec![], vec![9]));
    assert_eq!(failing(&s, a), [(9, 1)]);
}

#[test]
fn what_a_result_says_of_failing_messages_is_bounded() {
    let mut s = Session::empty();
    let a = account(&mut s);
    apply(&mut s, a, delta(7, vec![], (1..=(MAX_FAILING as u32 + 500)).collect()));
    assert_eq!(failing(&s, a).len(), MAX_FAILING);
    // One more that is tracked still counts when the map is full.
    apply(&mut s, a, delta(7, vec![], vec![MAX_FAILING as u32 + 900, 4]));
    assert!(failing(&s, a).contains(&(4, 2)));
}

#[test]
fn retry_takes_up_the_messages_left_out_again() {
    let mut s = Session::empty();
    let a = account(&mut s);
    apply(&mut s, a, delta(7, vec![], vec![4, 5]));
    assert_eq!(s.run("sync.retryFailed", &json!({"account": a + 1000})).unwrap()["cleared"], 0);
    assert_eq!(s.run("sync.retryFailed", &json!({"account": a})).unwrap()["cleared"], 2);
    // Named with no tries: asked for wherever the folder was read up to.
    assert_eq!(failing(&s, a), [(4, 0), (5, 0)]);
    assert_eq!(s.run("sync.retryFailed", &json!({})).unwrap()["cleared"], 0);
    // And counted from the start again.
    apply(&mut s, a, delta(7, vec![], vec![4]));
    assert_eq!(failing(&s, a), [(4, 1), (5, 0)]);
}

#[test]
fn counts_the_server_no_longer_asks_about_are_dropped() {
    let mut s = Session::empty();
    let a = account(&mut s);
    apply(&mut s, a, delta(7, vec![], vec![4, 5]));
    let mut later = delta(7, vec![], vec![]);
    later.failing_gone = vec![4];
    apply(&mut s, a, later);
    assert_eq!(failing(&s, a), [(5, 1)]);
}
