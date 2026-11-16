// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a review of the phone's fetching found (MB3.17): shutdown with a call waiting, the queue
//! of an account, the results of a search, what "whole" says, an attachment asked for in another
//! mailbox, and a connection that can't be told from a metered one. Nothing here leaves this
//! machine; every message is invented.

use std::sync::Arc;
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_host::secrets::secret_key;
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, GraphSearchResult, MessagePage};
use mailcraft_model::{FolderKind, Mailbox};
use serde_json::json;

use crate::fake_tests::{FakePlatform, block_on, now};
use crate::fetch_tests::{EMAIL, Find, core_with, eventually, id_of, is_whole, message, nothing_found, runner, session, until, whole};
use crate::{Core, CoreError, Event, FetchFailure, NetworkKind, OpenState};

const SECOND: &str = "second@example.org";
const DAY: i64 = 86_400;

#[test]
fn a_call_that_waits_for_a_message_when_the_core_shuts_down_ends_and_does_not_wait_for_ever() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), false);
    let (core, _, _) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    // Everything is called off (as `shutdown` does first) and an attachment is asked for after.
    core.sync.cancel_fetches(None);
    let (tx, rx) = std::sync::mpsc::channel();
    let asking = core.clone();
    std::thread::spawn(move || {
        let _ = tx.send(block_on(asking.attachment(m1, 0)));
    });
    let answer = rx.recv_timeout(Duration::from_secs(5)).expect("the call waits for ever");
    assert!(matches!(answer, Err(CoreError::Cancelled)), "{answer:?}");
    fake.release();
}

/// A core over two Microsoft accounts, each with an Inbox of messages that are headers only: the
/// first's `m1` to `m3`, the second's `b1` to `b9`.
fn two_accounts() -> Session {
    let mut s = session();
    s.run("account.add", &json!({"email": SECOND, "kind": "microsoft"})).unwrap();
    let account = s.mailbox.accounts.iter().find(|a| a.email == SECOND).unwrap().id;
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let messages = (1..=9).map(|i| message(&format!("b{i}"), &format!("Second {i}"), now() - i * 60, "in")).collect();
    let result = GraphMailResult {
        account,
        email: SECOND.into(),
        listing: Some(FolderListing { folders, complete: true }),
        pages: vec![MessagePage { folder: "in".into(), messages, link: "link-b".into(), caught_up: true, ..Default::default() }],
        whole: true,
        ..Default::default()
    };
    mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    s
}

#[test]
fn an_accounts_waiting_jobs_are_limited_without_calling_off_another_accounts() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), false);
    let platform = FakePlatform::new();
    let core = Core::start(platform.clone(), two_accounts).unwrap();
    core.sync.set_fetch_runner(run);
    let kept = block_on(core.call(|s| {
        Ok(s.mailbox.accounts.iter().filter(|a| a.email == EMAIL || a.email == SECOND).map(|a| secret_key("refresh", a)).collect::<Vec<_>>())
    }))
    .unwrap();
    for key in kept {
        platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    }
    block_on(core.call(|s| {
        s.sync_progress.full_paused = true;
        Ok(())
    }))
    .unwrap();
    // The first account: one under way and two waiting.
    let first: Vec<u64> = ["m1", "m2", "m3"].iter().map(|id| id_of(&core, id)).collect();
    for id in &first {
        assert_eq!(block_on(core.open_message(*id)).unwrap(), OpenState::Fetching);
    }
    eventually("the first job is under way", || !fake.fetches().is_empty());
    // The second: one under way, then more waiting than any one account's queue holds.
    for i in 1..=9 {
        let id = id_of(&core, &format!("b{i}"));
        assert_eq!(block_on(core.open_message(id)).unwrap(), OpenState::Fetching);
    }
    let events = crate::fetch_tests::drain(&core);
    let called_off: Vec<u64> = events
        .iter()
        .filter_map(|e| if let Event::FetchFailed { message, reason: FetchFailure::Cancelled } = e { Some(*message) } else { None })
        .collect();
    // Neither account holds more than its queue takes: nothing of the first is called off for the
    // second's (eight waiting in all with its own seven would have been).
    assert!(called_off.is_empty(), "{called_off:?}");
    fake.release();
    eventually("the first account's mail came", || first.iter().all(|id| is_whole(&core, *id)));
}

#[test]
fn a_search_result_already_on_the_phone_is_kept_for_the_session_too() {
    let found: Arc<Find> = Arc::new(|_, _| GraphSearchResult {
        email: EMAIL.into(),
        messages: vec![message("m1", "Budget review", now() - DAY, "in")],
        ..Default::default()
    });
    let (run, _fake) = runner(Arc::new(whole), found, true);
    let (core, _, account) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    assert!(!block_on(core.call(move |s| Ok(s.kept_for_now(m1)))).unwrap());
    let answer = block_on(core.search_server(account, "budget".into())).unwrap();
    assert_eq!(answer.messages, [m1]);
    assert!(block_on(core.call(move |s| Ok(s.kept_for_now(m1)))).unwrap(), "trimming may not take it away");
}

#[test]
fn a_message_is_whole_only_when_nothing_of_it_is_missing() {
    let (run, _fake) = runner(Arc::new(whole), nothing_found(), true);
    let (core, _, _) = core_with(run, false);
    let m1 = id_of(&core, "m1");
    block_on(core.call(move |s| {
        let m = s.mailbox.messages.get_mut(&m1).map(Arc::make_mut).unwrap();
        m.unfetched = false;
        m.incomplete = true;
        Ok(())
    }))
    .unwrap();
    assert!(!block_on(core.message(m1, 360.0, false)).unwrap().whole, "still incomplete");
    block_on(core.call(move |s| {
        let m = s.mailbox.messages.get_mut(&m1).map(Arc::make_mut).unwrap();
        (m.incomplete, m.not_downloaded) = (false, 10_000);
        Ok(())
    }))
    .unwrap();
    assert!(!block_on(core.message(m1, 360.0, false)).unwrap().whole, "too large to download");
    block_on(core.call(move |s| {
        let m = s.mailbox.messages.get_mut(&m1).map(Arc::make_mut).unwrap();
        m.not_downloaded = 0;
        Ok(())
    }))
    .unwrap();
    assert!(block_on(core.message(m1, 360.0, false)).unwrap().whole);
}

#[test]
fn an_attachment_is_never_read_from_another_mailbox_than_the_one_it_was_asked_in() {
    let mut s = session();
    let opening = s.openings();
    let m1 = s.mailbox.messages.values().find(|m| m.server.as_ref().is_some_and(|sv| sv.id == "m1")).map(|m| m.id).unwrap();
    // The same mailbox: the message is looked at (and has no such attachment).
    assert!(
        matches!(crate::fetch_api::in_opening(&s, opening, |s| crate::attachments::read_in(s, m1, 0, usize::MAX, None)), Err(e) if !matches!(e, CoreError::Cancelled))
    );
    // Another mailbox is opened (File › Open) while the call waited.
    s.replace_mailbox(Mailbox::with_local_account("", ""));
    assert!(matches!(
        crate::fetch_api::in_opening(&s, opening, |s| crate::attachments::read_in(s, m1, 0, usize::MAX, None)),
        Err(CoreError::Cancelled)
    ));
}

#[test]
fn the_background_does_not_fetch_when_the_phone_cannot_tell_whether_the_connection_is_metered() {
    let (run, fake) = runner(Arc::new(whole), nothing_found(), true);
    let (core, platform, _) = core_with(run, true);
    let m1 = id_of(&core, "m1");
    *platform.net.lock().unwrap() = NetworkKind::Unknown;
    for _ in 0..10 {
        core.enqueue_tick();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(fake.fetches().is_empty(), "nothing in the background on a connection of unknown kind");
    assert!(!is_whole(&core, m1));
    // What the owner opens comes on any connection.
    assert_eq!(block_on(core.open_message(m1)).unwrap(), OpenState::Fetching);
    until(&core, |e| matches!(e, Event::MessageFetched { message } if *message == m1));
}
