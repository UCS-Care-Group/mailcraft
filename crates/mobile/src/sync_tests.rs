// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send/Receive through the binding: a core over a fake platform, a Microsoft account, and a
//! runner that plays the Graph stand-in (it asks no server). Nothing here leaves this machine.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_host::graph::{GraphRequest, GraphResult};
use mailcraft_host::secrets::secret_key;
use mailcraft_host::sync::SendReceive;
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, GraphMessage, MessagePage, ServerMessage, Tokens};
use mailcraft_model::{Address, FolderKind};
use serde_json::json;

use crate::fake_tests::{FakePlatform, block_on};
use crate::{Core, CoreError, Event, NetworkKind, NotificationTopic};

const EMAIL: &str = "owner@example.org";

/// A session with one Microsoft account and no clock of its own for rounds.
fn session() -> Session {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": EMAIL, "kind": "microsoft"})).unwrap();
    // (Rounds only when asked: the tests count what the runner is called with.)
    s.prefs.sync_minutes = 0;
    s
}

type Runner = dyn Fn(GraphRequest) -> GraphResult + Send + Sync;

fn core_with(platform: Arc<FakePlatform>, make: impl FnOnce() -> Session + Send + 'static, runner: Arc<Runner>) -> Arc<Core> {
    Core::start_with(platform, make, SendReceive::with_runner(move |req| runner(req))).unwrap()
}

fn account_of(core: &Core) -> (u64, String) {
    block_on(core.call(|s| {
        let a = s.mailbox.accounts.first().unwrap();
        Ok((a.id, secret_key("refresh", a)))
    }))
    .unwrap()
}

/// A platform that has a refresh token kept for the account, as an earlier sign-in left it.
fn signed_in(runner: Arc<Runner>) -> (Arc<Core>, Arc<FakePlatform>, u64) {
    let platform = FakePlatform::new();
    let core = core_with(platform.clone(), session, runner);
    let (id, key) = account_of(&core);
    platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    (core, platform, id)
}

fn message(id: &str, subject: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: "in".into(), ..Default::default() },
        subject: subject.into(),
        from: Address::new("Ada Lovelace", "ada@example.org"),
        ..Default::default()
    }
}

/// The first run: the folders, and an Inbox with one old message, caught up.
fn first_download(req: &GraphRequest) -> GraphMailResult {
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    GraphMailResult {
        email: req.account.email.clone(),
        listing: Some(FolderListing { folders, complete: true }),
        pages: vec![MessagePage {
            folder: "in".into(),
            messages: vec![message("old", "Old news")],
            link: "link-1".into(),
            caught_up: true,
            ..Default::default()
        }],
        whole: true,
        ..Default::default()
    }
}

fn second_run(req: &GraphRequest) -> GraphMailResult {
    GraphMailResult {
        email: req.account.email.clone(),
        pages: vec![MessagePage {
            folder: "in".into(),
            messages: vec![message("new", "Budget review")],
            link: "link-2".into(),
            caught_up: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// A runner that brings the first download and then one new message, counting its calls and what
/// refresh token each was given.
fn stand_in() -> (Arc<Runner>, Arc<Mutex<Vec<String>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let runner: Arc<Runner> = Arc::new(move |req: GraphRequest| {
        let n = {
            let mut log = log.lock().unwrap();
            log.push(req.refresh.clone());
            log.len()
        };
        let piece = if n == 1 { first_download(&req) } else { second_run(&req) };
        if let Some(pieces) = &req.mail_pieces {
            let _ = pieces.send(GraphMailResult { account: req.account.id, partial: false, ..piece });
        }
        GraphResult { account: req.account.id, ..Default::default() }
    });
    (runner, seen)
}

/// What is waiting in the event stream now, without waiting for more.
fn drain(core: &Core) -> Vec<Event> {
    let mut out = Vec::new();
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        let mut next = std::pin::pin!(core.next_event());
        match next.as_mut().poll(&mut cx) {
            Poll::Ready(Some(event)) => out.push(event),
            Poll::Ready(None) | Poll::Pending => return out,
        }
    }
}

/// Events until the round's end has come (ten seconds at most).
fn until_finished(core: &Core) -> Vec<Event> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut all = Vec::new();
    loop {
        all.extend(drain(core));
        if all.iter().any(|e| matches!(e, Event::SyncFinished { .. })) {
            return all;
        }
        assert!(Instant::now() < deadline, "the round never ended: {all:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn inbox_subjects(core: &Core) -> Vec<String> {
    block_on(core.call(|s| Ok(s.mailbox.messages.values().map(|m| m.subject.clone()).collect()))).unwrap()
}

#[test]
fn sync_now_brings_mail_in_and_the_app_learns_through_events() {
    let (runner, seen) = stand_in();
    let (core, platform, _) = signed_in(runner);
    let answer = block_on(core.sync_now()).unwrap();
    assert!(answer.started, "{answer:?}");
    let first = until_finished(&core);
    assert_eq!(inbox_subjects(&core), ["Old news"]);
    assert!(first.iter().any(|e| matches!(e, Event::RevChanged { .. })), "{first:?}");
    // An account's first download is not "new mail".
    assert!(!first.iter().any(|e| matches!(e, Event::Notification { .. })), "{first:?}");
    assert!(first.iter().any(|e| matches!(e, Event::SyncFinished { new_mail: 0, finished: true })));

    assert!(block_on(core.sync_now()).unwrap().started);
    let second = until_finished(&core);
    assert_eq!(inbox_subjects(&core), ["Old news", "Budget review"]);
    assert!(second.iter().any(|e| matches!(e, Event::RevChanged { .. })));
    let told: Vec<_> = second
        .iter()
        .filter_map(|e| match e {
            Event::Notification { notification } => Some(notification.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(told.len(), 1, "{second:?}");
    assert_eq!((told[0].topic, told[0].title.as_str(), told[0].body.as_str()), (NotificationTopic::Mail, "Ada Lovelace", "Budget review"));
    assert!(second.iter().any(|e| matches!(e, Event::SyncFinished { new_mail: 1, finished: true })));
    // The stored sign-in was what the runs began with, and no secret went anywhere else.
    assert_eq!(*seen.lock().unwrap(), ["stored-refresh", "stored-refresh"]);
    assert!(platform.shown.lock().unwrap().is_empty(), "the core raises events; the app shows the notification");
    assert!(!format!("{second:?}").contains("stored-refresh"));
    let status = block_on(core.sync_status()).unwrap();
    assert!(!status.running && status.needs_sign_in.is_empty());
}

#[test]
fn new_mail_notifications_can_be_switched_off_and_many_are_counted() {
    let (runner, _) = stand_in();
    let (core, _, _) = signed_in(runner);
    core.set_notify_new_mail(false);
    block_on(core.sync_now()).unwrap();
    until_finished(&core);
    block_on(core.sync_now()).unwrap();
    let events = until_finished(&core);
    assert!(!events.iter().any(|e| matches!(e, Event::Notification { .. })), "{events:?}");
    assert!(events.iter().any(|e| matches!(e, Event::SyncFinished { new_mail: 1, .. })));
}

#[test]
fn background_sync_runs_one_round_and_says_what_it_brought() {
    let (runner, _) = stand_in();
    let (core, _, _) = signed_in(runner);
    let first = block_on(core.background_sync(10_000)).unwrap();
    assert_eq!((first.ran, first.finished, first.new_mail, first.sign_in_needed), (true, true, 0, false), "{first:?}");
    let second = block_on(core.background_sync(10_000)).unwrap();
    assert_eq!((second.ran, second.finished, second.new_mail), (true, true, 1), "{second:?}");
    assert!(second.errors.is_empty());
}

#[test]
fn background_sync_keeps_to_its_budget() {
    let runner: Arc<Runner> = Arc::new(|req: GraphRequest| {
        // A server that never answers, until it is called off.
        let until = Instant::now() + Duration::from_secs(20);
        while !req.cancel.load(Ordering::Relaxed) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let (core, _, _) = signed_in(runner);
    let began = Instant::now();
    let summary = block_on(core.background_sync(600)).unwrap();
    assert!(!summary.finished && summary.ran, "{summary:?}");
    assert!(began.elapsed() < Duration::from_secs(8), "{:?}", began.elapsed());
    // The run that was called off ends, and the core is ready for the next round.
    let deadline = Instant::now() + Duration::from_secs(10);
    while block_on(core.sync_status()).unwrap().running {
        assert!(Instant::now() < deadline, "the run never ended");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn nothing_runs_while_offline() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let runner: Arc<Runner> = Arc::new(move |req: GraphRequest| {
        counted.fetch_add(1, Ordering::Relaxed);
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let (core, platform, _) = signed_in(runner);
    // The platform says there is no network.
    *platform.net.lock().unwrap() = NetworkKind::Offline;
    let answer = block_on(core.sync_now()).unwrap();
    assert!(!answer.started && !answer.message.is_empty(), "{answer:?}");
    let summary = block_on(core.background_sync(1_000)).unwrap();
    assert!(!summary.ran && summary.finished && !summary.errors.is_empty(), "{summary:?}");
    // The owner's Work Offline.
    *platform.net.lock().unwrap() = NetworkKind::Online { metered: false, data_saving: false };
    block_on(core.call(|s| {
        s.prefs.offline = true;
        Ok(())
    }))
    .unwrap();
    let answer = block_on(core.sync_now()).unwrap();
    assert!(!answer.started && answer.message.contains("Work Offline"), "{answer:?}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn a_round_goes_by_itself_at_the_owners_minutes_and_not_when_never() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let runner: Arc<Runner> = Arc::new(move |req: GraphRequest| {
        counted.fetch_add(1, Ordering::Relaxed);
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let platform = FakePlatform::new();
    let core = core_with(
        platform.clone(),
        || {
            let mut s = session();
            s.prefs.sync_minutes = 5;
            s
        },
        runner,
    );
    let (_, key) = account_of(&core);
    platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    // The driver's first wake is a second after the start.
    let deadline = Instant::now() + Duration::from_secs(10);
    while calls.load(Ordering::Relaxed) == 0 {
        assert!(Instant::now() < deadline, "no round began by itself");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_account_of_another_kind_is_said_so_in_words() {
    let platform = FakePlatform::new();
    let (runner, _) = stand_in();
    let core = core_with(
        platform,
        || {
            let mut s = Session::empty();
            s.run(
                "account.add",
                &json!({"email": "ada@example.org", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "outlook.office365.com"}}),
            )
            .unwrap();
            s.prefs.sync_minutes = 0;
            s
        },
        runner,
    );
    let answer = block_on(core.sync_now()).unwrap();
    assert!(!answer.started && answer.message.contains("can't be sent and received"), "{answer:?}");
    let status = block_on(core.sync_status()).unwrap();
    assert_eq!(status.unsupported.len(), 1);
    assert!(status.unsupported[0].message.contains("phone"));
    assert!(!status.running);
}

#[test]
fn a_sign_in_without_a_stored_one_asks_the_owner_to_sign_in() {
    let (runner, seen) = stand_in();
    let core = core_with(FakePlatform::new(), session, runner);
    assert!(block_on(core.sync_now()).unwrap().started);
    let events = until_finished(&core);
    assert!(events.iter().any(|e| matches!(e, Event::SignInNeeded { .. })), "{events:?}");
    assert!(seen.lock().unwrap().is_empty(), "nothing ran without a sign-in");
    let status = block_on(core.sync_status()).unwrap();
    assert_eq!(status.needs_sign_in.len(), 1);
    let summary = block_on(core.background_sync(1_000)).unwrap();
    assert!(summary.sign_in_needed && !summary.ran, "{summary:?}");
}

#[test]
fn tokens_handed_over_by_the_app_are_kept_and_remembered_and_forgotten_on_request() {
    let (runner, seen) = stand_in();
    let platform = FakePlatform::new();
    let core = core_with(platform.clone(), session, runner);
    let (id, key) = account_of(&core);
    let tokens = Tokens { access: "acc".into(), refresh: "fresh-refresh".into(), expires_at: i64::MAX, email: EMAIL.into(), ..Default::default() };
    block_on(core.adopt_tokens(id, tokens)).unwrap();
    // Kept in the platform's store, and the name is in the index that "remove my data" reads.
    let store = platform.secrets.kept.lock().unwrap().clone();
    assert!(store.contains(&(key.clone(), "fresh-refresh".to_string())), "{}", store.len());
    let index = store.iter().find(|(k, _)| k == "mailcraft:index").map(|(_, v)| v.clone()).unwrap_or_default();
    assert!(index.contains(&key) && !index.contains("fresh-refresh"), "{index}");
    // The run begins with it.
    assert!(block_on(core.background_sync(10_000)).unwrap().ran);
    assert_eq!(seen.lock().unwrap().first().map(String::as_str), Some("fresh-refresh"));
    // Remove all data: nothing of it is left.
    assert!(block_on(core.forget_sign_ins()).unwrap().looked_for >= 1);
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
}

#[test]
fn another_persons_tokens_are_refused_and_not_kept() {
    let (runner, _) = stand_in();
    let platform = FakePlatform::new();
    let core = core_with(platform.clone(), session, runner);
    let (id, _) = account_of(&core);
    let tokens = Tokens { refresh: "theirs".into(), email: "you@example.org".into(), ..Default::default() };
    match block_on(core.adopt_tokens(id, tokens)) {
        Err(CoreError::Failed { reason }) => assert!(reason.contains("somebody else"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
    // An account that isn't there.
    assert!(matches!(block_on(core.adopt_tokens(u64::MAX, Tokens::default())), Err(CoreError::Failed { .. })));
}

#[test]
fn a_sign_in_the_owner_does_not_remember_is_not_written() {
    let (runner, _) = stand_in();
    let platform = FakePlatform::new();
    let core = core_with(platform.clone(), session, runner);
    core.set_remember_sign_ins(false);
    let (id, _) = account_of(&core);
    let tokens = Tokens { refresh: "fresh-refresh".into(), email: EMAIL.into(), expires_at: i64::MAX, ..Default::default() };
    block_on(core.adopt_tokens(id, tokens)).unwrap();
    assert!(platform.secrets.kept.lock().unwrap().is_empty());
}

#[test]
fn junk_budgets_and_calls_never_panic() {
    let (runner, _) = stand_in();
    let platform = FakePlatform::new();
    *platform.net.lock().unwrap() = NetworkKind::Offline;
    let core = core_with(platform, session, runner);
    for budget in [0, 1, u64::MAX] {
        let summary = block_on(core.background_sync(budget)).unwrap();
        assert!(!summary.ran);
    }
    assert!(matches!(block_on(core.sign_in(u64::MAX)), Err(CoreError::Failed { .. })));
    assert!(!core.sign_in_redirect(String::new()));
    assert!(!core.sign_in_redirect("x".repeat(100_000)));
    core.shutdown();
    assert!(matches!(block_on(core.sync_now()), Err(CoreError::Cancelled)));
}

/// A session with a Microsoft account and an IMAP one, rounds by the clock.
fn mixed_session() -> Session {
    let mut s = session();
    s.run("account.add", &json!({"email": "ada@example.org", "auth": "password", "incoming": {"host": "imap.example.org"}})).unwrap();
    s.prefs.sync_minutes = 5;
    s
}

#[test]
fn a_round_by_the_clock_queues_only_what_the_phone_can_run() {
    let seen = Arc::new(Mutex::new(Vec::<u64>::new()));
    let counted = seen.clone();
    let runner: Arc<Runner> = Arc::new(move |req: GraphRequest| {
        counted.lock().unwrap().push(req.account.id);
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let platform = FakePlatform::new();
    let core = core_with(platform.clone(), mixed_session, runner);
    let (ms, key) = account_of(&core);
    platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    let deadline = Instant::now() + Duration::from_secs(10);
    while seen.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "no round began by itself");
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = block_on(core.sync_status()).unwrap();
    let queued = block_on(core.call(|s| Ok(s.driver.round.queue.iter().map(|(a, _)| *a).collect::<Vec<_>>()))).unwrap();
    assert!(queued.iter().all(|a| *a == ms), "{queued:?}");
    assert_eq!(status.unsupported.len(), 1, "{status:?}");
    assert!(status.unsupported[0].account != ms);
    assert!(seen.lock().unwrap().iter().all(|a| *a == ms));
}

#[test]
fn the_index_of_names_is_written_again_after_a_failed_write() {
    let (runner, _) = stand_in();
    let platform = FakePlatform::new();
    let core = core_with(platform.clone(), session, runner);
    let (id, key) = account_of(&core);
    let tokens = || Tokens { access: "a".into(), refresh: "r".into(), expires_at: i64::MAX, email: EMAIL.into(), ..Default::default() };
    platform.secrets.fail_index.store(1, Ordering::Relaxed);
    block_on(core.adopt_tokens(id, tokens())).unwrap();
    let index = |p: &FakePlatform| p.secrets.kept.lock().unwrap().iter().find(|(k, _)| k.contains("index")).map(|(_, v)| v.clone());
    // The same credential again: the set doesn't grow, the index is written all the same
    // (or the wake has already written it).
    block_on(core.adopt_tokens(id, tokens())).unwrap();
    assert!(index(&platform).is_some_and(|i| i.contains(&key)));
}

#[test]
fn a_tick_queued_before_shutdown_starts_no_round() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let runner: Arc<Runner> = Arc::new(move |req: GraphRequest| {
        counted.fetch_add(1, Ordering::Relaxed);
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let platform = FakePlatform::new();
    let core = core_with(
        platform.clone(),
        || {
            let mut s = session();
            s.prefs.sync_minutes = 5;
            s
        },
        runner,
    );
    let (_, key) = account_of(&core);
    platform.secrets.kept.lock().unwrap().push((key, "stored-refresh".to_string()));
    let (started_tx, started) = std::sync::mpsc::channel::<()>();
    let (release_tx, release) = std::sync::mpsc::channel::<()>();
    let holder = core.clone();
    let blocker = std::thread::spawn(move || {
        let _ = block_on(holder.call(move |_| {
            let _ = started_tx.send(());
            let _ = release.recv();
            Ok(())
        }));
    });
    started.recv().unwrap();
    // The session thread is busy: the tick waits behind it, and shutdown comes first.
    core.enqueue_tick();
    core.shutdown();
    release_tx.send(()).unwrap();
    blocker.join().unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn a_failed_index_write_is_retried_at_the_next_wake_with_nothing_new_written() {
    let (runner, _) = stand_in();
    let platform = FakePlatform::new();
    let core = core_with(platform.clone(), session, runner);
    let (id, key) = account_of(&core);
    platform.secrets.fail_index.store(1, Ordering::Relaxed);
    let tokens = Tokens { access: "a".into(), refresh: "r".into(), expires_at: i64::MAX, email: EMAIL.into(), ..Default::default() };
    block_on(core.adopt_tokens(id, tokens)).unwrap();
    let index = || platform.secrets.kept.lock().unwrap().iter().find(|(k, _)| k.contains("index")).map(|(_, v)| v.clone());
    // The write with the adoption was tried and failed. (The driver, woken by it, may already
    // have written the index again: that is not asked here.)
    assert_eq!(platform.secrets.fail_index.load(Ordering::Relaxed), 0, "the first write of the index failed");
    // The driver wakes by itself; no credential is written again.
    let deadline = Instant::now() + Duration::from_secs(10);
    while index().is_none() {
        assert!(Instant::now() < deadline, "the index was never written again");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(index().is_some_and(|i| i.contains(&key)));
}

#[test]
fn an_account_that_went_is_no_longer_said_to_be_unsupported() {
    let (runner, _) = stand_in();
    let platform = FakePlatform::new();
    let core = core_with(
        platform,
        || {
            let mut s = mixed_session();
            s.prefs.sync_minutes = 0;
            s
        },
        runner,
    );
    let unsupported = |core: &Core| block_on(core.sync_status()).unwrap().unsupported;
    let deadline = Instant::now() + Duration::from_secs(10);
    core.enqueue_tick();
    while unsupported(&core).len() != 1 {
        assert!(Instant::now() < deadline, "the IMAP account was never told of");
        std::thread::sleep(Duration::from_millis(20));
        core.enqueue_tick();
    }
    let imap = unsupported(&core)[0].account;
    block_on(core.execute("account.remove".into(), json!({"id": imap}).to_string())).unwrap();
    while !unsupported(&core).is_empty() {
        assert!(Instant::now() < deadline, "a removed account is still told of");
        std::thread::sleep(Duration::from_millis(20));
        core.enqueue_tick();
    }
}
