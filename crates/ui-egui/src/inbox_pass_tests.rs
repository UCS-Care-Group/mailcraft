// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Inbox pass in the window (#276): which accounts have one, that it comes before the other
//! phases of the round, what Stop and Skip do to it, and that the run after it starts from the
//! Inbox's link the pass moved on. The host is a fake that notes every job; every address is
//! invented.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::graph_mail::apply_graph_mail;
use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, GraphMessage, MessagePage, ServerMessage, SyncPhase, Timings, Tokens};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::{GraphRequest, GraphResult, MailApp, Services};

const OWNER: &str = "noor@example.org";
const SHARED: &str = "support@example.org";
const FIRST_LINK: &str = "https://graph.microsoft.com/v1.0/me/mailFolders/in/messages/delta?$deltatoken=one";
const NEXT_LINK: &str = "https://graph.microsoft.com/v1.0/me/mailFolders/in/messages/delta?$deltatoken=two";

/// What a Graph job was asked: for whom, whether it was an Inbox pass, and the link it was given
/// for the Inbox.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    account: Id,
    pass: bool,
    inbox_link: Option<String>,
}

/// The fake host: notes the jobs, answers a pass with one new message and a link moved on, and a
/// run with its end. With `hold`, it keeps the first job's pieces and answer until told.
#[derive(Default)]
struct Host {
    jobs: Vec<Job>,
    hold: bool,
    held: Option<(mpsc::SyncSender<GraphMailResult>, mpsc::Sender<GraphResult>)>,
    /// The refresh token each job went with.
    refreshes: Vec<String>,
}

type Shared = Arc<Mutex<Host>>;

fn tokens() -> Tokens {
    Tokens { access: "own-access".into(), refresh: "ref-1".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

fn message(id: &str) -> GraphMessage {
    GraphMessage {
        server: ServerMessage { id: id.into(), folder: "in".into(), ..Default::default() },
        subject: "Hello".into(),
        received: 1_790_000_000,
        sent: 1_790_000_000,
        ..Default::default()
    }
}

/// Give `account` an Inbox known to the server (`in`), with a delta link when `link`.
fn seed_inbox(app: &mut MailApp, account: Id, email: &str, link: bool) {
    let folders = vec![GraphFolder { id: "in".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }];
    let pages =
        if link { vec![MessagePage { folder: "in".into(), link: FIRST_LINK.into(), caught_up: true, ..Default::default() }] } else { Vec::new() };
    let seed =
        GraphMailResult { account, email: email.into(), listing: Some(FolderListing { folders, complete: true }), pages, ..Default::default() };
    apply_graph_mail(&mut app.session, seed).unwrap();
}

/// A window with the owner's account, signed in this session, and a shared mailbox under it;
/// each Inbox has a delta link when `links` says so (owner, shared).
fn window(links: (bool, bool)) -> (MailApp, Id, Id, Shared) {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    // (No round of its own: the tests start them.)
    app.auto_sync = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.tokens.insert(owner, tokens());
    let shared = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    seed_inbox(&mut app, owner, OWNER, links.0);
    seed_inbox(&mut app, shared, SHARED, links.1);
    let host: Shared = Default::default();
    let note = host.clone();
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        let mut host = note.lock().unwrap();
        let mail = req.mail.clone().unwrap_or_default();
        let (account, email) = (req.account.id, req.account.email.clone());
        let pass = mail.inbox_pass.is_some();
        host.refreshes.push(req.refresh.clone());
        host.jobs.push(Job { account, pass, inbox_link: mail.links.iter().find(|l| l.folder == "in").map(|l| l.link.clone()) });
        let (tx, rx) = mpsc::channel();
        let pieces = req.mail_pieces.clone().unwrap();
        if host.hold && host.held.is_none() {
            host.held = Some((pieces, tx));
            return rx;
        }
        let mut timings = Timings::default();
        timings.add(if pass { SyncPhase::Inbox } else { SyncPhase::NewMail }, 120, 1);
        let piece = if pass {
            let page = MessagePage {
                folder: "in".into(),
                messages: vec![message(&format!("m-{account}"))],
                link: NEXT_LINK.into(),
                caught_up: true,
                ..Default::default()
            };
            GraphMailResult { account, email, pages: vec![page], partial: true, inbox_pass: true, timings, ..Default::default() }
        } else {
            GraphMailResult { account, email, timings, ..Default::default() }
        };
        pieces.send(piece).unwrap();
        drop(pieces);
        tx.send(GraphResult { account, ..Default::default() }).unwrap();
        rx
    }));
    // The desktop host can open mail connections; nothing here is kept in step by them.
    app.services.sync = Some(Box::new(|_req: mailcraft_model::sync::SyncRequest| mpsc::channel().1));
    (app, owner, shared, host)
}

/// Poll and start what is next, until the round is over (or `most` frames have gone).
fn drive(app: &mut MailApp, most: usize) {
    for _ in 0..most {
        app.poll_graph();
        app.auto_sync();
        let over = app.graph_rx.is_none() && app.sync_queue.is_empty() && app.inbox_queue.is_empty();
        if over && !app.session.sync_progress.running() {
            break;
        }
    }
}

fn row(app: &MailApp, account: Id) -> serde_json::Value {
    let all = app.session.sync_progress.json(&app.session.mailbox);
    all["accounts"].as_array().unwrap().iter().find(|r| r["id"] == json!(account)).cloned().unwrap()
}

fn jobs(host: &Shared) -> Vec<Job> {
    host.lock().unwrap().jobs.clone()
}

fn count_hello(app: &MailApp, account: Id) -> usize {
    app.session.mailbox.messages.values().filter(|m| m.account == account && m.subject == "Hello").count()
}

#[test]
fn every_graph_accounts_inbox_pass_comes_before_the_other_phases_of_the_round() {
    let (mut app, owner, shared, host) = window((true, true));
    app.execute("ui.sync", json!({})).unwrap();
    drive(&mut app, 200);
    let jobs = jobs(&host);
    let pass = |account| Job { account, pass: true, inbox_link: Some(FIRST_LINK.into()) };
    // The owner's and the shared mailbox's passes first, then each account's own run, which goes
    // on from the link the pass moved on: the Inbox is not read again.
    let run = |account| Job { account, pass: false, inbox_link: Some(NEXT_LINK.into()) };
    assert_eq!(jobs, [pass(owner), pass(shared), run(owner), run(shared)]);
    // What the passes brought is in, once each.
    assert_eq!((count_hello(&app, owner), count_hello(&app, shared)), (1, 1));
    // The round is over, every account done, and the Inbox's time is its own phase in each row's
    // timings, first (the run's own time is New mail's).
    assert!(!app.session.sync_progress.running());
    for account in [owner, shared] {
        let r = row(&app, account);
        assert_eq!(r["state"], json!("done"), "{r}");
        assert_eq!(r["timings"]["phases"][0]["phase"], json!("inbox"), "{r}");
        assert_eq!(r["timings"]["phases"][0]["requests"], json!(1), "{r}");
        assert_eq!(r["new"], json!(1), "what the pass brought is counted");
    }
}

#[test]
fn an_account_with_no_stored_inbox_link_has_a_pass_from_the_start_of_its_inbox() {
    // The owner's Inbox was never read; the shared mailbox's was.
    let (mut app, owner, shared, host) = window((false, true));
    app.execute("ui.sync", json!({})).unwrap();
    drive(&mut app, 200);
    let jobs = jobs(&host);
    assert_eq!(jobs.iter().filter(|j| j.pass).map(|j| j.account).collect::<Vec<_>>(), [owner, shared]);
    assert_eq!(jobs.iter().filter(|j| !j.pass).map(|j| j.account).collect::<Vec<_>>(), [owner, shared]);
    assert_eq!(jobs[0], Job { account: owner, pass: true, inbox_link: None });
    assert_eq!(jobs[1], Job { account: shared, pass: true, inbox_link: Some(FIRST_LINK.into()) });
    // Nor a pass for a round under way, whose Inbox link is the next page of it.
    let owner_inbox = app.session.mailbox.folder_of_kind(owner, FolderKind::Inbox).unwrap();
    let server = app.session.mailbox.folder_mut(owner_inbox).unwrap().server.as_mut().unwrap();
    (server.delta, server.caught_up) = (FIRST_LINK.into(), false);
    assert!(mailcraft_engine::cmd::graph_mail::graph_inbox_request(&app.session, owner).is_none(), "not caught up");
}

#[test]
fn stop_during_the_pass_ends_the_run() {
    let (mut app, owner, shared, host) = window((true, true));
    host.lock().unwrap().hold = true;
    app.execute("ui.sync", json!({})).unwrap();
    // The owner's pass is under way, held by the host; the others wait their turn.
    assert_eq!(jobs(&host).len(), 1);
    assert!(jobs(&host)[0].pass);
    assert_eq!(row(&app, owner)["phase"], json!("inbox"));
    app.execute("ui.cancelSync", json!({})).unwrap();
    assert!(app.sync_queue.is_empty() && app.inbox_queue.is_empty(), "nothing is left in line");
    // The host stops the pass after its page in hand, and answers.
    let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
    drop(pieces);
    answer.send(GraphResult { account: owner, ..Default::default() }).unwrap();
    drive(&mut app, 50);
    // Not another job: no second pass, no run.
    assert_eq!(jobs(&host).len(), 1, "{:?}", jobs(&host));
    assert!(!app.session.sync_progress.running());
    assert_eq!(row(&app, owner)["state"], json!("stopped"));
    assert_eq!(row(&app, shared)["state"], json!("stopped"));
}

#[test]
fn skip_leaves_out_an_accounts_other_phases_but_never_its_pass() {
    let (mut app, owner, shared, host) = window((true, true));
    host.lock().unwrap().hold = true;
    app.execute("ui.sync", json!({})).unwrap();
    assert_eq!(row(&app, shared)["state"], json!("waiting"));
    // The shared mailbox is skipped while the owner's pass is under way.
    assert_eq!(app.execute("ui.skipSync", json!({"account": shared})).unwrap(), json!({"skipped": shared}));
    let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
    pieces
        .send(GraphMailResult {
            account: owner,
            email: OWNER.into(),
            partial: true,
            inbox_pass: true,
            pages: vec![MessagePage { folder: "in".into(), link: NEXT_LINK.into(), caught_up: true, ..Default::default() }],
            ..Default::default()
        })
        .unwrap();
    drop(pieces);
    answer.send(GraphResult { account: owner, ..Default::default() }).unwrap();
    host.lock().unwrap().hold = false;
    drive(&mut app, 200);
    let jobs = jobs(&host);
    // Its pass ran; its run did not.
    assert!(jobs.iter().any(|j| j.account == shared && j.pass), "{jobs:?}");
    assert!(!jobs.iter().any(|j| j.account == shared && !j.pass), "{jobs:?}");
    assert!(jobs.iter().any(|j| j.account == owner && !j.pass));
    assert_eq!(count_hello(&app, shared), 1, "what its pass brought is in");
    let r = row(&app, shared);
    assert_eq!(r["state"], json!("skipped"), "{r}");
    assert_eq!(r["timings"]["phases"][0]["phase"], json!("inbox"), "{r}");
}

#[test]
fn a_round_that_started_by_itself_asks_nobody_for_a_pass() {
    // The owner's sign-in is not at hand (nothing kept, nothing remembered): no pass for either,
    // and nothing is asked.
    let (mut app, owner, shared, host) = window((true, true));
    app.tokens.remove(&owner);
    app.queue_inbox_passes(&[owner, shared], true);
    assert!(app.start_next_inbox_pass().is_none());
    assert!(host.lock().unwrap().jobs.is_empty() && app.signing_in.is_none() && app.inbox_queue.is_empty());
}

// The Inbox checked by itself every minute, between rounds.

const T0: i64 = 1_800_000_000;

/// A window whose Inbox is checked by itself: Send/Receive runs by itself, a sign-in could be
/// asked for (it never is), and the session's clock is at `T0`. The first tick only starts the
/// minute.
fn checked_window() -> (MailApp, Id, Id, Shared) {
    let (mut app, owner, shared, host) = window((true, true));
    app.auto_sync = true;
    app.services.sign_in = Some(Box::new(|_| mpsc::channel().1));
    at(&mut app, T0);
    assert_eq!(app.inbox_check_tick(), None, "nothing at once: the window's first round reads the Inbox");
    (app, owner, shared, host)
}

fn at(app: &mut MailApp, now: i64) {
    app.session.clock.now = Some(now);
}

/// Take in what the check under way brings, until it is over.
fn settle(app: &mut MailApp) {
    for _ in 0..20 {
        if app.graph_rx.is_none() {
            return;
        }
        app.poll_graph();
    }
}

/// Tick at `now` until nothing more starts; the accounts whose Inbox was checked.
fn tick(app: &mut MailApp, now: i64) -> Vec<Id> {
    at(app, now);
    let mut checked = Vec::new();
    while let Some(account) = app.inbox_check_tick() {
        checked.push(account);
        settle(app);
    }
    checked
}

#[test]
fn the_inbox_is_checked_by_itself_every_minute_quietly() {
    let (mut app, owner, shared, host) = checked_window();
    app.session.sync_status = "Up to date — 9:30".into();
    let progress = app.session.sync_progress.clone();
    assert!(tick(&mut app, T0 + 30).is_empty(), "not before the minute is up");
    assert_eq!(tick(&mut app, T0 + 60), [owner, shared], "each account kept in step through Graph, one after the other");
    assert!(tick(&mut app, T0 + 90).is_empty());
    assert_eq!(tick(&mut app, T0 + 125), [owner, shared]);
    let jobs = jobs(&host);
    assert_eq!(jobs.len(), 4);
    assert!(jobs.iter().all(|j| j.pass), "an Inbox pass and nothing else: {jobs:?}");
    // Each goes on from the link the one before moved on.
    assert_eq!(jobs[0].inbox_link.as_deref(), Some(FIRST_LINK));
    assert_eq!(jobs[2].inbox_link.as_deref(), Some(NEXT_LINK));
    // What it brought is in; Send/Receive Progress and the status line never heard of it.
    assert_eq!((count_hello(&app, owner), count_hello(&app, shared)), (1, 1));
    assert!(app.session.sync_progress == progress && !app.session.sync_progress.running());
    assert_eq!(app.session.sync_status, "Up to date — 9:30");
    assert!(app.dialog.is_none() && app.signing_in.is_none() && app.sync_panel.is_none());
}

#[test]
fn a_check_is_quiet_while_it_runs_and_ticks_never_stack() {
    let (mut app, owner, _, host) = checked_window();
    host.lock().unwrap().hold = true;
    let (progress, status) = (app.session.sync_progress.clone(), app.session.sync_status.clone());
    at(&mut app, T0 + 60);
    assert_eq!(app.inbox_check_tick(), Some(owner));
    // Not a Send/Receive the window shows: the button is there to press, nothing says it runs.
    assert!(!app.syncing() && app.enabled("ui.sync"));
    assert!(app.session.sync_progress == progress && app.session.sync_status == status);
    // Minutes go by with the check held: no other starts.
    for later in [T0 + 120, T0 + 180, T0 + 600] {
        at(&mut app, later);
        assert_eq!(app.inbox_check_tick(), None);
    }
    assert_eq!(jobs(&host).len(), 1);
}

#[test]
fn no_check_while_a_round_is_under_way_and_the_next_is_a_minute_after_it() {
    let (mut app, owner, shared, host) = checked_window();
    // (The next round by itself is minutes away.)
    app.last_auto_sync = crate::now_ms();
    host.lock().unwrap().hold = true;
    app.execute("ui.sync", json!({})).unwrap();
    assert_eq!(jobs(&host).len(), 1, "the round's first pass");
    assert!(tick(&mut app, T0 + 60).is_empty(), "the round is under way");
    assert!(tick(&mut app, T0 + 200).is_empty(), "still");
    let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
    drop(pieces);
    answer.send(GraphResult { account: owner, ..Default::default() }).unwrap();
    host.lock().unwrap().hold = false;
    // The round goes to its end, its frames ticking as the window's do.
    for _ in 0..200 {
        app.poll_graph();
        app.auto_sync();
        assert_eq!(app.inbox_check_tick(), None, "no check during the round");
        if app.graph_rx.is_none() && app.sync_queue.is_empty() && app.inbox_queue.is_empty() && !app.session.sync_progress.running() {
            break;
        }
    }
    assert_eq!(jobs(&host).len(), 4, "{:?}", jobs(&host));
    // The round read the Inbox: the next check is a minute after it.
    assert!(tick(&mut app, T0 + 230).is_empty());
    assert_eq!(tick(&mut app, T0 + 260), [owner, shared]);
}

#[test]
fn a_send_receive_asked_for_during_a_check_goes_ahead() {
    let (mut app, owner, shared, host) = checked_window();
    host.lock().unwrap().hold = true;
    at(&mut app, T0 + 60);
    assert_eq!(app.inbox_check_tick(), Some(owner));
    let started = app.execute("ui.sync", json!({})).unwrap();
    assert_eq!(started["inbox"], json!(true), "{started}");
    assert!(started.get("check").is_none(), "the round's own pass: {started}");
    assert_eq!(jobs(&host).len(), 2);
    assert!(app.syncing() && app.session.sync_progress.running());
    // The check gave way: nothing of it is left waiting.
    assert!(!app.inbox_check.under_way());
    host.lock().unwrap().hold = false;
    drive(&mut app, 200);
    assert_eq!((count_hello(&app, owner), count_hello(&app, shared)), (1, 1));
}

#[test]
fn a_check_that_fails_is_tried_again_next_minute_without_a_word() {
    let (mut app, owner, _, host) = checked_window();
    host.lock().unwrap().hold = true;
    let (progress, status) = (app.session.sync_progress.clone(), app.session.sync_status.clone());
    at(&mut app, T0 + 60);
    assert_eq!(app.inbox_check_tick(), Some(owner));
    let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
    drop(pieces);
    let failed = GraphResult { account: owner, errors: vec!["Couldn't reach Microsoft".into()], ..Default::default() };
    answer.send(failed).unwrap();
    settle(&mut app);
    host.lock().unwrap().hold = false;
    assert!(app.dialog.is_none() && app.sync_panel.is_none() && app.signing_in.is_none());
    assert!(app.session.sync_progress == progress && app.session.sync_status == status);
    // (The shared mailbox's check of this tick goes on.)
    while app.inbox_check_tick().is_some() {
        settle(&mut app);
    }
    assert_eq!(tick(&mut app, T0 + 120).first(), Some(&owner), "tried again");
}

#[test]
fn no_check_for_an_account_without_graph_or_when_send_receive_by_itself_is_off() {
    let (mut app, owner, shared, host) = checked_window();
    let imap = app.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    app.passwords.insert(imap, "invented".into());
    let asked = Arc::new(Mutex::new(0));
    let count = asked.clone();
    app.services.sync = Some(Box::new(move |_req: mailcraft_model::sync::SyncRequest| {
        *count.lock().unwrap() += 1;
        mpsc::channel().1
    }));
    assert_eq!(tick(&mut app, T0 + 60), [owner, shared], "not the mail server's account");
    assert_eq!(*asked.lock().unwrap(), 0, "no mail server is asked");
    // Send/Receive by itself turned off: no check either.
    app.session.prefs.sync_minutes = 0;
    assert!(tick(&mut app, T0 + 200).is_empty());
    app.session.prefs.sync_minutes = 5;
    app.auto_sync = false;
    assert!(tick(&mut app, T0 + 400).is_empty());
    // No account kept in step through Graph: nothing ever.
    app.auto_sync = true;
    app.execute("account.remove", json!({"id": shared})).unwrap();
    app.execute("account.remove", json!({"id": owner})).unwrap();
    let before = jobs(&host).len();
    assert!(tick(&mut app, T0 + 600).is_empty() && tick(&mut app, T0 + 700).is_empty());
    assert_eq!(jobs(&host).len(), before);
    assert_eq!(*asked.lock().unwrap(), 0);
}

#[test]
fn no_check_for_an_account_with_no_inbox_link_or_no_sign_in() {
    let (mut app, owner, shared, host) = window((false, true));
    app.auto_sync = true;
    app.services.sign_in = Some(Box::new(|_| mpsc::channel().1));
    at(&mut app, T0);
    assert_eq!(app.inbox_check_tick(), None);
    assert_eq!(tick(&mut app, T0 + 60), [shared], "the owner's Inbox was never read to its end");
    // Nothing kept or remembered to sign in with: no check, nothing asked.
    app.tokens.remove(&owner);
    app.graph_tokens.clear();
    assert!(tick(&mut app, T0 + 120).is_empty());
    assert!(app.signing_in.is_none() && app.dialog.is_none());
    assert_eq!(jobs(&host).len(), 1);
}

/// What a check renewed with Microsoft: a new access token and a new refresh token.
fn renewed(refresh: &str) -> Tokens {
    Tokens { access: "renewed-access".into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

/// The credential store of the system, as a map.
#[derive(Clone, Default)]
struct Store(Arc<Mutex<std::collections::HashMap<String, String>>>);

impl crate::SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

#[test]
fn a_check_that_gives_way_keeps_the_sign_in_it_renewed_and_the_round_goes_with_it() {
    let (mut app, owner, _, host) = checked_window();
    let store = Store::default();
    app.services.secrets = Some(Box::new(store.clone()));
    app.ui.remember_sign_in = true;
    host.lock().unwrap().hold = true;
    at(&mut app, T0 + 60);
    assert_eq!(app.inbox_check_tick(), Some(owner));
    // Microsoft renewed the sign-in and the check's answer is in, not yet taken in, when the
    // owner presses Send/Receive.
    let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
    drop(pieces);
    answer.send(GraphResult { account: owner, tokens: Some(renewed("ref-2")), ..Default::default() }).unwrap();
    host.lock().unwrap().hold = false;
    app.execute("ui.sync", json!({})).unwrap();
    assert_eq!(host.lock().unwrap().refreshes, ["ref-1", "ref-2"], "the round goes with the renewed refresh token");
    assert_eq!(app.tokens[&owner].refresh, "ref-2");
    assert!(store.0.lock().unwrap().values().any(|s| s == "ref-2"), "remembered as a finished check's would be");
}

#[test]
fn a_check_that_gave_way_keeps_the_sign_in_its_answer_brings_later() {
    let (mut app, owner, _, host) = checked_window();
    host.lock().unwrap().hold = true;
    at(&mut app, T0 + 60);
    assert_eq!(app.inbox_check_tick(), Some(owner));
    let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
    host.lock().unwrap().hold = false;
    app.execute("ui.sync", json!({})).unwrap();
    drive(&mut app, 200);
    // The check's job comes to its end after the round: only its sign-in is taken in.
    drop(pieces);
    answer.send(GraphResult { account: owner, tokens: Some(renewed("ref-3")), ..Default::default() }).unwrap();
    let hello = count_hello(&app, owner);
    at(&mut app, T0 + 61);
    app.inbox_check_tick();
    assert_eq!(app.tokens[&owner].refresh, "ref-3");
    assert_eq!(app.graph_tokens[&owner].access, "renewed-access");
    assert_eq!(count_hello(&app, owner), hello);
}

#[test]
fn a_check_that_keeps_failing_says_so_once_until_one_gets_through() {
    let (mut app, owner, _, host) = checked_window();
    let logged = crate::graph_switch::tests::logged();
    let problem = "Couldn't reach Microsoft (inbox check, once)";
    let said = || logged.lock().unwrap().iter().filter(|l| l.contains(problem)).count();
    let fail = |app: &mut MailApp, host: &Shared, now: i64| {
        host.lock().unwrap().hold = true;
        at(app, now);
        assert_eq!(app.inbox_check_tick(), Some(owner));
        let (pieces, answer) = host.lock().unwrap().held.take().unwrap();
        drop(pieces);
        answer.send(GraphResult { account: owner, errors: vec![problem.into()], ..Default::default() }).unwrap();
        settle(app);
        host.lock().unwrap().hold = false;
        while app.inbox_check_tick().is_some() {
            settle(app);
        }
    };
    fail(&mut app, &host, T0 + 60);
    fail(&mut app, &host, T0 + 120);
    fail(&mut app, &host, T0 + 180);
    assert_eq!(said(), 1, "the network down is said once, not every minute");
    // One gets through; the next that fails says so again.
    assert_eq!(tick(&mut app, T0 + 240).first(), Some(&owner));
    fail(&mut app, &host, T0 + 300);
    assert_eq!(said(), 2);
}
