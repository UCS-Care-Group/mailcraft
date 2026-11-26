// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A pull to refresh (#1981): the mail of the account on screen, or of every account for a view
//! across them, answered once that mail is in and not when the round's calendar, to-dos and
//! contacts are; and the device's log of how long each part took. A core over a fake platform and
//! runners that play the Graph stand-in: nothing here leaves this machine.
//!
//! What holds:
//! - a pull for one account runs that account alone, and moves nothing of another's: not its
//!   pacing, not its hold, not its mail;
//! - the account left out still goes when its time comes;
//! - a pull while the account runs or waits in line doesn't run it twice, and is still answered;
//!   a pull while another account runs puts this one at the end of the line;
//! - no network, or a run that fails, answers the pull (the first with the words the owner sees);
//! - the account's Outbox goes with its pull;
//! - the answer doesn't wait for the calendar's job, and the round's end (a background run's
//!   answer) still does;
//! - the log says numbers and account numbers, never an address, a folder's name or a subject.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_host::graph::{GraphRequest, GraphResult};
use mailcraft_host::secrets::secret_key;
use mailcraft_host::sync::SendReceive;
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, GraphMessage, MessagePage, ServerMessage, Timings};
use mailcraft_model::{Address, FolderKind, Message};
use serde_json::json;

use crate::fake_tests::{FakePlatform, block_on};
use crate::jobs::Runners;
use crate::sync::run_line;
use crate::sync_tests::{Runner, drain, signed_in, stand_in};
use crate::{Core, CoreError, Event, NetworkKind, SyncAnswer};

/// The accounts the runner was called for, in order.
type Calls = Arc<Mutex<Vec<u64>>>;

/// A runner that brings each account its Inbox with one message, noting whom it ran for. While
/// `hold` names an account, that account's run waits until it is cleared (or is called off).
fn runner(calls: Calls, hold: Arc<Mutex<Option<u64>>>, entered: Arc<AtomicBool>) -> Arc<Runner> {
    Arc::new(move |req: GraphRequest| {
        let account = req.account.id;
        calls.lock().unwrap().push(account);
        if *hold.lock().unwrap() == Some(account) {
            entered.store(true, Ordering::SeqCst);
            let until = Instant::now() + Duration::from_secs(20);
            while *hold.lock().unwrap() == Some(account) && !req.cancel.load(Ordering::Relaxed) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
        let message = GraphMessage {
            server: ServerMessage { id: format!("m-{account}-{}", calls.lock().unwrap().len()), folder: "in".into(), ..Default::default() },
            subject: "Budget review".into(),
            from: Address::new("Ada Lovelace", "ada@example.org"),
            ..Default::default()
        };
        let piece = GraphMailResult {
            account,
            email: req.account.email.clone(),
            listing: Some(FolderListing { folders, complete: true }),
            pages: vec![MessagePage { folder: "in".into(), messages: vec![message], link: "link".into(), caught_up: true, ..Default::default() }],
            whole: true,
            ..Default::default()
        };
        if let Some(pieces) = &req.mail_pieces {
            let _ = pieces.send(piece);
        }
        GraphResult { account, ..Default::default() }
    })
}

struct Two {
    core: Arc<Core>,
    platform: Arc<FakePlatform>,
    a: u64,
    b: u64,
    /// Every account, in the mailbox's order (`a` and `b` are the first two).
    ids: Vec<u64>,
    calls: Calls,
    hold: Arc<Mutex<Option<u64>>>,
    entered: Arc<AtomicBool>,
}

/// Two Microsoft accounts, both signed in as an earlier start left them, rounds only when asked.
fn two() -> Two {
    accounts(&[true, true])
}

/// A Microsoft account for each of `signed`, signed in as an earlier start left it where it says
/// so (else with no sign-in kept), rounds only when asked.
fn accounts(signed: &[bool]) -> Two {
    let (calls, hold, entered) = (Calls::default(), Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
    let run = runner(calls.clone(), hold.clone(), entered.clone());
    let platform = FakePlatform::new();
    let n = signed.len();
    let make = move || {
        let mut s = Session::empty();
        for i in 0..n {
            s.run("account.add", &json!({"email": format!("owner{i}@example.org"), "kind": "microsoft"})).unwrap();
        }
        s.prefs.sync_minutes = 0;
        s
    };
    let core = Core::start_with(platform.clone(), make, SendReceive::with_runner(move |req| run(req))).unwrap();
    let ids: Vec<(u64, String)> =
        block_on(core.call(|s| Ok(s.mailbox.accounts.iter().filter(|a| a.kind.has_server()).map(|a| (a.id, secret_key("refresh", a))).collect())))
            .unwrap();
    for ((_, key), signed) in ids.iter().zip(signed) {
        if *signed {
            platform.secrets.kept.lock().unwrap().push((key.clone(), "stored-refresh".to_string()));
        }
    }
    let all: Vec<u64> = ids.iter().map(|(id, _)| *id).collect();
    Two { core, platform, a: all[0], b: all[1], ids: all, calls, hold, entered }
}

/// A background run on a thread of its own, its summary on the channel.
fn background(core: &Arc<Core>, budget_ms: u64) -> Receiver<crate::SyncSummary> {
    let (tx, rx) = channel();
    let core = core.clone();
    std::thread::spawn(move || {
        let _ = tx.send(block_on(core.background_sync(budget_ms)).unwrap());
    });
    rx
}

/// A pull to refresh as the app makes it: the ask, then the wait for its mail when it started.
fn refresh(core: &Core, account: Option<u64>) -> Result<SyncAnswer, CoreError> {
    let answer = block_on(core.refresh_mail(account))?;
    if answer.started {
        block_on(core.mail_in(account))?;
    }
    Ok(answer)
}

/// [`refresh`] on a thread of its own, its answer on the channel.
fn pull(core: &Arc<Core>, account: Option<u64>) -> Receiver<Result<SyncAnswer, CoreError>> {
    let (tx, rx) = channel();
    let core = core.clone();
    std::thread::spawn(move || {
        let _ = tx.send(refresh(&core, account));
    });
    rx
}

fn answered(rx: &Receiver<Result<SyncAnswer, CoreError>>) -> SyncAnswer {
    rx.recv_timeout(Duration::from_secs(15)).expect("the pull was never answered").unwrap()
}

fn until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Events until the round's end, gathered (ten seconds at most).
fn until_finished(core: &Core, mut all: Vec<Event>) -> Vec<Event> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        all.extend(drain(core));
        if all.iter().any(|e| matches!(e, Event::SyncFinished { .. })) {
            return all;
        }
        assert!(Instant::now() < end, "the round never ended: {all:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn logs(events: &[Event]) -> Vec<String> {
    events.iter().filter_map(|e| if let Event::SyncLog { line } = e { Some(line.clone()) } else { None }).collect()
}

#[test]
fn a_pull_in_one_accounts_folder_brings_that_account_alone_and_moves_nothing_of_the_other() {
    let t = two();
    let b = t.b;
    // The other account is held back (its first download waits for room): it stays so.
    block_on(t.core.hold_background_sync(b, true)).unwrap();
    let before = block_on(t.core.call(|s| Ok(s.driver.round.last_auto))).unwrap();
    let answer = refresh(&t.core, Some(t.a)).unwrap();
    assert!(answer.started, "{answer:?}");
    assert_eq!(*t.calls.lock().unwrap(), vec![t.a]);
    let (last_auto, pushed_b, mail_b) = block_on(t.core.call(move |s| {
        Ok((s.driver.round.last_auto, s.driver.round.push_seen.contains_key(&b), s.mailbox.messages.values().filter(|m| m.account == b).count()))
    }))
    .unwrap();
    let held_b = t.core.sync.lock().sr.held(b);
    let held_a = t.core.sync.lock().sr.held(t.a);
    assert_eq!(last_auto, before, "the clock's rounds keep their pacing");
    assert!(!pushed_b, "the other account isn't noted as having run");
    assert_eq!(mail_b, 0);
    assert!(held_b && !held_a, "only the account asked for is let go of");
}

#[test]
fn the_account_left_out_of_a_pull_still_goes_at_its_time() {
    let t = two();
    assert!(refresh(&t.core, Some(t.a)).unwrap().started);
    let summary = block_on(t.core.background_sync(10_000)).unwrap();
    assert!(summary.ran && summary.finished, "{summary:?}");
    assert!(t.calls.lock().unwrap().contains(&t.b), "{:?}", t.calls.lock().unwrap());
}

#[test]
fn a_pull_across_accounts_brings_every_one() {
    let t = two();
    assert!(refresh(&t.core, None).unwrap().started);
    let calls = t.calls.lock().unwrap().clone();
    assert!(calls.contains(&t.a) && calls.contains(&t.b), "{calls:?}");
}

#[test]
fn a_pull_while_the_account_runs_does_not_run_it_twice_and_is_answered() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.a);
    let first = pull(&t.core, Some(t.a));
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    let second = pull(&t.core, Some(t.a));
    std::thread::sleep(Duration::from_millis(200));
    *t.hold.lock().unwrap() = None;
    assert!(answered(&first).started);
    assert!(answered(&second).started);
    assert_eq!(*t.calls.lock().unwrap(), vec![t.a], "one run of the account");
}

#[test]
fn a_pull_while_another_account_runs_joins_the_line() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.b);
    let other = pull(&t.core, Some(t.b));
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    let mine = pull(&t.core, Some(t.a));
    std::thread::sleep(Duration::from_millis(100));
    *t.hold.lock().unwrap() = None;
    assert!(answered(&mine).started);
    assert!(answered(&other).started);
    assert_eq!(*t.calls.lock().unwrap(), vec![t.b, t.a]);
}

#[test]
fn a_new_accounts_first_round_asked_for_while_a_pull_runs_joins_its_line() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.a);
    let mine = pull(&t.core, Some(t.a));
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    // The app's round for a new account (it keeps the holds): the round under way takes the account.
    assert!(block_on(t.core.sync_keeping_holds()).unwrap().started);
    *t.hold.lock().unwrap() = None;
    assert!(answered(&mine).started);
    until("the new account's first run never came", || t.calls.lock().unwrap().len() == 2);
    assert_eq!(*t.calls.lock().unwrap(), vec![t.a, t.b]);
}

#[test]
fn a_pull_with_no_network_says_so_and_a_failed_run_is_answered_too() {
    let t = two();
    *t.platform.net.lock().unwrap() = NetworkKind::Offline;
    let answer = refresh(&t.core, Some(t.a)).unwrap();
    assert!(!answer.started && answer.message.contains("network"), "{answer:?}");
    assert!(t.calls.lock().unwrap().is_empty());
    // A run that fails (the server answers with an error and no mail).
    let failing: Arc<Runner> =
        Arc::new(|req: GraphRequest| GraphResult { account: req.account.id, errors: vec!["the server didn't answer".into()], ..Default::default() });
    let (core, _, account) = signed_in(failing);
    let answer = pull(&core, Some(account));
    assert!(answered(&answer).started);
}

#[test]
fn the_accounts_outbox_goes_with_its_pull_and_the_others_waits() {
    let seen: Arc<Mutex<Vec<(u64, usize)>>> = Arc::default();
    let log = seen.clone();
    let t = two();
    let (a, b) = (t.a, t.b);
    block_on(t.core.call(move |s| {
        for (account, subject) in [(a, "Going out"), (b, "Also going")] {
            let folder = s.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
            let m = Message {
                account,
                folder,
                subject: subject.into(),
                to: vec![Address::new("", "kit@example.net")],
                message_id: format!("{account}@example.org"),
                ..Default::default()
            };
            s.mailbox.add_message(m).unwrap();
        }
        s.mailbox_changed();
        Ok(())
    }))
    .unwrap();
    let (quick, _) = stand_in();
    let watching: Arc<Runner> = Arc::new(move |req: GraphRequest| {
        log.lock().unwrap().push((req.account.id, req.mail.as_ref().map_or(0, |m| m.sends.len())));
        quick(req)
    });
    // (The same mailbox, a runner that notes what each run would send.)
    let mailbox = block_on(t.core.call(|s| Ok(s.mailbox.clone()))).unwrap();
    let platform = t.platform.clone();
    let core = Core::start_with(
        platform,
        move || {
            let mut s = Session::empty();
            s.mailbox = mailbox;
            s.prefs.sync_minutes = 0;
            s
        },
        SendReceive::with_runner(move |req| watching(req)),
    )
    .unwrap();
    assert!(refresh(&core, Some(a)).unwrap().started);
    assert_eq!(*seen.lock().unwrap(), vec![(a, 1)], "the account's own Outbox went with its run, and the other account wasn't run");
}

#[test]
fn the_pull_is_answered_when_the_mail_is_in_and_the_rounds_end_waits_for_the_calendar() {
    let (runner, _) = stand_in();
    let (core, _, account) = signed_in(runner);
    let (release, entered) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let (gate, mark) = (release.clone(), entered.clone());
    core.sync.set_job_runners(Runners {
        calendar: Arc::new(move |req: GraphRequest| {
            mark.store(true, Ordering::SeqCst);
            let until = Instant::now() + Duration::from_secs(20);
            while !gate.load(Ordering::SeqCst) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(5));
            }
            GraphResult { account: req.account.id, ..Default::default() }
        }),
        tasks: Arc::new(|job| mailcraft_host::tasks::TasksResult { account: job.account.id, ..Default::default() }),
    });
    let answer = pull(&core, Some(account));
    // Answered while the calendar's job hasn't ended (it waits to be let go of).
    assert!(answered(&answer).started);
    assert!(!release.load(Ordering::SeqCst));
    until("the calendar's job never started at the round's end", || entered.load(Ordering::SeqCst));
    let _ = block_on(core.call(|_| Ok(())));
    let events = drain(&core);
    assert!(!events.iter().any(|e| matches!(e, Event::SyncFinished { .. })), "the round waits for its calendar: {events:?}");
    release.store(true, Ordering::SeqCst);
    let events = until_finished(&core, events);
    let lines = logs(&events);
    assert!(
        lines.iter().any(|l| l.starts_with(&format!("calendar and contacts job: account {account}, ")) && l.ends_with("at the end of a round")),
        "{lines:?}"
    );
}

#[test]
fn the_device_log_says_how_long_each_part_took_and_nothing_of_the_mail() {
    let t = two();
    assert!(refresh(&t.core, Some(t.a)).unwrap().started);
    let events = until_finished(&t.core, Vec::new());
    let lines = logs(&events);
    let run = format!("mail run: account {}, ", t.a);
    assert!(lines.iter().any(|l| l.starts_with(&run) && l.contains("done") && l.contains("full sweep yes")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("pull to refresh: mail in after ") && l.ends_with("1 accounts")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("round: ") && l.contains("1 mail runs") && l.ends_with("finished")), "{lines:?}");
    // The second run of the account within the hour reads only the folders that changed.
    assert!(refresh(&t.core, Some(t.a)).unwrap().started);
    let lines = logs(&until_finished(&t.core, Vec::new()));
    assert!(lines.iter().any(|l| l.starts_with(&run) && l.contains("full sweep no")), "{lines:?}");
    for line in logs(&events).iter().chain(&lines) {
        for private in ["@", "example", "Budget", "Inbox", "link", "stored-refresh", "Ada"] {
            assert!(!line.contains(private), "{private} in {line}");
        }
    }
}

#[test]
fn a_pull_whose_mail_came_before_its_wait_is_answered_at_once_and_logged() {
    let t = two();
    assert!(block_on(t.core.refresh_mail(Some(t.a))).unwrap().started);
    let sync = t.core.sync.clone();
    until("the run never ended", || {
        let sync = sync.clone();
        block_on(t.core.call(move |s| Ok(!sync.lock().sr.busy() && s.driver.round.queue.is_empty()))).unwrap()
    });
    let _ = drain(&t.core);
    block_on(t.core.mail_in(Some(t.a))).unwrap();
    let lines = logs(&drain(&t.core));
    assert!(lines.iter().any(|l| l.starts_with("pull to refresh: mail in after ") && l.ends_with("1 accounts")), "{lines:?}");
}

#[test]
fn a_run_line_is_numbers_and_fixed_words() {
    let mut timings = Timings::default();
    timings.add(mailcraft_model::sync::SyncPhase::Inbox, 120, 2);
    timings.add(mailcraft_model::sync::SyncPhase::NewMail, 900, 7);
    timings.skipped = 4;
    assert_eq!(
        run_line(3, 1500, "done", true, &timings),
        "mail run: account 3, 1500 ms, done, full sweep yes, phases [Inbox 120 ms 2 requests; NewMail 900 ms 7 requests], 4 folders unchanged"
    );
}

#[test]
fn a_pull_that_waits_is_let_go_of_when_the_core_closes() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.a);
    let waiting = pull(&t.core, Some(t.a));
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    t.core.shutdown();
    let answer = waiting.recv_timeout(Duration::from_secs(10)).expect("the pull still waits");
    assert!(matches!(answer, Err(CoreError::Cancelled)), "{answer:?}");
    *t.hold.lock().unwrap() = None;
}

#[test]
fn a_pull_for_an_account_that_is_gone_brings_every_account_as_before() {
    let t = two();
    let answer = refresh(&t.core, Some(t.a + t.b + 99)).unwrap();
    assert!(answer.started, "{answer:?}");
    let calls = t.calls.lock().unwrap().clone();
    assert!(calls.contains(&t.a) && calls.contains(&t.b), "{calls:?}");
}

#[test]
fn a_pull_in_a_shared_mailboxs_folder_brings_its_owners_account() {
    let t = two();
    let a = t.a;
    let shared = block_on(t.core.call(move |s| {
        let id = s.mailbox.add_shared_mailbox(a, "team@example.org").map_err(|e| CoreError::Failed { reason: e.to_string() })?;
        s.mailbox_changed();
        Ok(id)
    }))
    .unwrap();
    let answer = refresh(&t.core, Some(shared)).unwrap();
    assert!(answer.started, "never refused: {answer:?}");
    assert_eq!(*t.calls.lock().unwrap(), vec![a]);
}

#[test]
fn a_background_run_that_comes_while_a_pull_runs_brings_every_account() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.a);
    let mine = pull(&t.core, Some(t.a));
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    let worker = background(&t.core, 20_000);
    std::thread::sleep(Duration::from_millis(100));
    *t.hold.lock().unwrap() = None;
    assert!(answered(&mine).started);
    let summary = worker.recv_timeout(Duration::from_secs(15)).expect("the background run never ended");
    assert!(summary.finished, "{summary:?}");
    // The background run's answer came after the other account's mail, as when it starts a round.
    assert_eq!(*t.calls.lock().unwrap(), vec![t.a, t.b]);
}

#[test]
fn a_pull_for_an_account_waiting_quietly_in_line_asks_for_its_sign_in() {
    // The second account has no sign-in kept: quietly, it is left out; asked for, the owner is asked.
    let t = accounts(&[true, false]);
    *t.hold.lock().unwrap() = Some(t.a);
    let first = pull(&t.core, Some(t.a));
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    // A background run joins the round: the second account waits in line quietly.
    let worker = background(&t.core, 20_000);
    let b = t.b;
    until("the background run never joined", || block_on(t.core.call(move |s| Ok(s.driver.round.queue.contains(&(b, true))))).unwrap());
    let mine = pull(&t.core, Some(t.b));
    std::thread::sleep(Duration::from_millis(100));
    *t.hold.lock().unwrap() = None;
    assert!(answered(&first).started);
    assert!(answered(&mine).started);
    let _ = worker.recv_timeout(Duration::from_secs(15));
    let _ = block_on(t.core.call(|_| Ok(())));
    let events = drain(&t.core);
    assert!(events.iter().any(|e| matches!(e, Event::SignInNeeded { account } if *account == b)), "{events:?}");
}

#[test]
fn a_pull_goes_before_the_accounts_waiting_in_line() {
    let t = accounts(&[true, true, true]);
    let (a, b, c) = (t.ids[0], t.ids[1], t.ids[2]);
    *t.hold.lock().unwrap() = Some(a);
    let worker = background(&t.core, 20_000);
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    let mine = pull(&t.core, Some(c));
    std::thread::sleep(Duration::from_millis(100));
    *t.hold.lock().unwrap() = None;
    assert!(answered(&mine).started);
    let _ = worker.recv_timeout(Duration::from_secs(15));
    assert_eq!(*t.calls.lock().unwrap(), vec![a, c, b]);
}

#[test]
fn a_pull_that_joined_a_background_round_outlives_that_rounds_time() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.a);
    // The background's round, with half a second: its first account's run never ends by itself.
    let worker = background(&t.core, 500);
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    let mine = pull(&t.core, Some(t.b));
    let summary = worker.recv_timeout(Duration::from_secs(15)).expect("the budget never ran out");
    assert!(!summary.finished, "{summary:?}");
    // The pull still brings its account's mail.
    assert!(answered(&mine).started);
    assert!(t.calls.lock().unwrap().contains(&t.b), "{:?}", t.calls.lock().unwrap());
    let b = t.b;
    let mail = block_on(t.core.call(move |s| Ok(s.mailbox.messages.values().filter(|m| m.account == b).count()))).unwrap();
    assert!(mail > 0);
}

#[test]
fn past_the_most_pulls_that_wait_one_is_answered_at_once() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.a);
    let mine = pull(&t.core, Some(t.a));
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    let (sync, a) = (t.core.sync.clone(), t.a);
    let replies = block_on(t.core.call(move |s| {
        let replies: Vec<_> = (0..17).map(|_| crate::queue::Queue::<Result<(), CoreError>>::new()).collect();
        for reply in &replies {
            sync.wait_mail(s, Some(a), reply.clone());
        }
        Ok(replies)
    }))
    .unwrap();
    // (The pull on its thread may be waiting too: one of the 16 places is its.)
    let answered_now = replies.iter().filter(|r| r.len() == 1).count();
    assert!((1..=2).contains(&answered_now), "{answered_now}");
    *t.hold.lock().unwrap() = None;
    assert!(answered(&mine).started);
    until("a pull that waited was never answered", || replies.iter().all(|r| r.len() == 1));
}

#[test]
fn a_pull_whose_account_is_removed_while_it_waits_is_answered() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.a);
    let mine = pull(&t.core, None);
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    let b = t.b;
    block_on(t.core.call(move |s| s.run("account.remove", &json!({"id": b})).map(|_| ()).map_err(|e| CoreError::Failed { reason: e.to_string() })))
        .unwrap();
    *t.hold.lock().unwrap() = None;
    assert!(answered(&mine).started);
    assert_eq!(*t.calls.lock().unwrap(), vec![t.a]);
}

#[test]
fn work_offline_turned_on_while_a_pull_waits_answers_it() {
    let t = two();
    *t.hold.lock().unwrap() = Some(t.a);
    let mine = pull(&t.core, Some(t.a));
    until("the run never started", || t.entered.load(Ordering::SeqCst));
    block_on(t.core.call(|s| {
        s.prefs.offline = true;
        Ok(())
    }))
    .unwrap();
    // Answered while the run hasn't ended: nothing more starts.
    assert!(answered(&mine).started);
    *t.hold.lock().unwrap() = None;
}
