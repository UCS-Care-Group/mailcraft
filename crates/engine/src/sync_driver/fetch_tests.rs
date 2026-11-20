// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::sync::{GraphFetchResult, GraphFetched, ServerMessage};
use mailcraft_model::{FolderKind, Id, Mailbox, Message};
use serde_json::json;

use super::fetch::*;
use super::{FETCH_IDLE_MS, FETCH_MOST_IDLE_MS, fetch_backoff_ms};
use crate::Session;

/// A session with `n` unfetched messages of one account, each with a server id `M<n>`.
fn window(n: usize) -> (Session, Vec<Id>) {
    let mut mailbox = Mailbox::with_local_account("", "");
    let account = mailbox.accounts[0].id;
    let inbox = mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let ids = (0..n)
        .map(|i| {
            let server = ServerMessage { id: format!("M{i}"), folder: "in".into(), ..Default::default() };
            mailbox.add_message(Message { account, folder: inbox, unfetched: true, server: Some(server), ..Default::default() }).unwrap()
        })
        .collect();
    (Session::new(mailbox), ids)
}

fn stopped_batch() -> GraphFetchResult {
    GraphFetchResult { errors: vec!["no".into()], ..Default::default() }
}

#[test]
fn a_background_that_stops_waits_longer_each_time_and_the_others_go_on() {
    let mut f = FetchDriver::default();
    let first = f.background_came(1, false, &stopped_batch(), 1_000.0).unwrap();
    assert!(first.first);
    assert_eq!(first.wait_ms, fetch_backoff_ms(0));
    assert!(f.account_waits(1, 1_000.0 + first.wait_ms - 1.0) && !f.account_waits(1, 1_000.0 + first.wait_ms));
    assert!(!f.account_waits(2, 1_000.0), "another account is its own");
    let second = f.background_came(1, false, &stopped_batch(), 50_000.0).unwrap();
    assert!(!second.first && second.wait_ms > first.wait_ms);
    // The server's own "busy for" counts when it is longer.
    let busy = GraphFetchResult { busy_for: Some(3_600), ..Default::default() };
    assert_eq!(f.background_came(3, false, &busy, 0.0).unwrap().wait_ms, FETCH_MOST_IDLE_MS.max(3_600_000.0));
    // Something fetched ends it; stopped by the owner is no server's doing.
    let fetched = GraphFetchResult { fetched: vec![GraphFetched { id: "M0".into(), mime: Vec::new() }], ..Default::default() };
    assert!(f.background_came(1, false, &fetched, 60_000.0).is_none());
    assert!(!f.account_waits(1, 60_001.0));
    assert!(f.background_came(2, true, &stopped_batch(), 0.0).is_none() && !f.account_waits(2, 0.0));
    let gone = GraphFetchResult { offline: true, ..Default::default() };
    f.background_came(2, false, &gone, 0.0);
    assert!(f.offline && f.account_waits(2, 1.0));
    f.resume();
    assert!(!f.account_waits(2, 1.0));
}

#[test]
fn the_background_rests_when_there_was_nothing_and_wakes_when_asked() {
    let mut f = FetchDriver::default();
    assert!(!f.resting(0.0));
    f.found_nothing(10.0);
    assert!(f.resting(10.0 + FETCH_IDLE_MS - 1.0) && !f.resting(10.0 + FETCH_IDLE_MS));
    f.wake();
    assert!(!f.resting(11.0));
    f.found_nothing(10.0);
    f.offline = true;
    f.forget();
    assert!(!f.resting(11.0) && !f.offline);
}

#[test]
fn progress_is_read_from_what_the_engine_said_and_junk_says_nothing() {
    let p = FetchProgress::of(&json!({"waiting": 30, "large": 5, "all": 100}));
    assert_eq!((p.done, p.all, p.large, p.background()), (70, 100, 5, 25));
    assert_eq!(FetchProgress::of(&json!({})).waiting, 0);
    assert_eq!(FetchProgress::of(&json!("junk")).all, 0);
    let odd = FetchProgress::of(&json!({"waiting": 9, "large": 99, "all": 3}));
    assert_eq!((odd.done, odd.background()), (0, 0), "never below zero");
}

#[test]
fn a_command_waits_only_for_the_message_it_names_and_only_when_it_is_not_whole() {
    let (mut s, ids) = window(2);
    s.sel.messages = vec![ids[0]];
    assert_eq!(needs_whole(&s, "mail.reply", &json!({})), Some(ids[0]));
    assert_eq!(needs_whole(&s, "mail.reply", &json!({"id": ids[1]})), Some(ids[1]));
    assert_eq!(needs_whole(&s, "mail.flag", &json!({"id": ids[1]})), None, "another command");
    assert_eq!(needs_whole(&s, "mail.reply", &json!({"id": "x"})), None, "named wrongly: refused by the command");
    assert_eq!(needs_whole(&s, "mail.reply", &json!({"id": 99_999})), None);
    s.mailbox.message_mut(ids[1]).unwrap().unfetched = false;
    assert_eq!(needs_whole(&s, "mail.reply", &json!({"id": ids[1]})), None, "whole already");
    s.mailbox.message_mut(ids[0]).unwrap().incomplete = true;
    assert_eq!(to_fetch(&s, "mail.forwardAsAttachment", ids[0]), None, "gone from the server for good");
    assert_eq!(to_fetch(&s, "mail.forward", ids[0]), Some(ids[0]));
    s.mailbox.message_mut(ids[0]).unwrap().server = None;
    assert_eq!(to_fetch(&s, "mail.forward", ids[0]), None, "not on a server");
}

#[test]
fn a_forward_is_read_as_the_engine_reads_it_and_at_most_a_batch_is_fetched_for_it() {
    let (s, ids) = window(BATCH + 5);
    assert_eq!(still_to_fetch(&s, &ids, &[]).len(), BATCH);
    assert_eq!(still_to_fetch(&s, &ids, &ids[..BATCH]).len(), 5, "not those tried");
    assert!(still_to_fetch(&s, &ids, &ids).is_empty());
    assert!(still_to_fetch(&s, &[99_999], &[]).is_empty());
    let (command_ids, focus, discard) = forward_of("mail.forward", &json!({"ids": [7, 3, 5], "discard": true})).unwrap();
    assert_eq!((command_ids, focus, discard), (vec![3, 5, 7], Some(5), true));
    assert!(forward_of("mail.reply", &json!({"ids": [1]})).is_none() && forward_of("mail.forward", &json!({})).is_none());
}

#[test]
fn a_batch_is_judged_message_by_message() {
    let (s, ids) = window(4);
    let result = GraphFetchResult {
        fetched: vec![GraphFetched { id: "M0".into(), mime: Vec::new() }],
        missing: vec!["M1".into()],
        failed: vec!["M2".into()],
        errors: vec!["the server said so".into()],
        ..Default::default()
    };
    let came = Came::of(&s, &ids, &result);
    assert_eq!((came.sent.as_slice(), came.answered.as_slice()), (&ids[..1], &ids[..3]), "the fourth was not answered for");
    assert_eq!(came.not_sent, Some((ids[1], true)), "the first that did not come, and that it is gone");
    assert_eq!(came.why.as_deref(), Some("the server said so"));
    // One that is no longer here is nothing to wait for.
    assert_eq!(Came::of(&s, &[99_999], &result).answered, [99_999]);
}

#[test]
fn what_came_is_taken_in_by_the_kind_of_account_and_a_stranger_changes_nothing() {
    let (mut s, ids) = window(1);
    let result = GraphFetchResult { account: 777, fetched: vec![GraphFetched { id: "M0".into(), mime: Vec::new() }], ..Default::default() };
    let _ = apply_fetch(&mut s, result);
    assert!(s.mailbox.message(ids[0]).unwrap().unfetched);
}
