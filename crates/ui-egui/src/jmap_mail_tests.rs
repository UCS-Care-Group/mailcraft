// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The window's side of a JMAP account's Send/Receive (M6), with a host that keeps every job for
//! the test to answer: the secret asked for or the host's own, Cancel, a round by itself, a run
//! whose task ended early, a secret turned down, a mailbox from a file held back, and messages
//! being downloaded whole when another mailbox takes the place of theirs. (The whole way, through
//! `mailcraft_jmap` and its server in the process: the desktop app's `jmap_mail_flow`.)

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::sync_progress::RunState;
use mailcraft_model::sync::{FolderListing, GraphFetchResult, GraphFetched, GraphFolder, JmapMailResult, ServerMessage};
use mailcraft_model::{FolderKind, Id, Mailbox, Message};
use serde_json::json;

use super::*;
use crate::{SecretStore, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SESSION: &str = "https://jmap.example.org/jmap/session";

type Jobs = Arc<Mutex<Vec<(JmapJob, mpsc::Sender<JmapMailResult>)>>>;

/// A window with JMAP accounts at these addresses, and a host that keeps every job. No round
/// starts by itself unless a test says so.
fn window(addresses: &[&str]) -> (MailApp, Vec<Id>, Jobs) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let jobs: Jobs = Default::default();
    let kept = jobs.clone();
    app.services.jmap = Some(Box::new(move |job: JmapJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    let accounts = addresses
        .iter()
        .map(|address| {
            let added = app.execute("account.add", json!({"email": address, "kind": "jmap", "jmap": {"session": SESSION}})).unwrap();
            added["id"].as_u64().unwrap()
        })
        .collect();
    (app, accounts, jobs)
}

/// The system's credential store, as the list of what it was asked to keep.
struct Recording(Arc<Mutex<Vec<String>>>);

impl SecretStore for Recording {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn set(&self, key: &str, _: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(key.to_string());
        Ok(())
    }
    fn delete(&self, _: &str) {}
}

/// The jobs the host was handed so far, taken.
fn taken(jobs: &Jobs) -> Vec<(JmapJob, mpsc::Sender<JmapMailResult>)> {
    std::mem::take(&mut *jobs.lock().unwrap())
}

type Fetches = Arc<Mutex<Vec<(JmapFetchJob, mpsc::Sender<GraphFetchResult>)>>>;

/// A host that downloads messages whole by keeping every job for the test to answer.
fn downloads_kept(app: &mut MailApp) -> Fetches {
    let fetches: Fetches = Default::default();
    let kept = fetches.clone();
    app.services.jmap_fetch = Some(Box::new(move |job: JmapFetchJob| {
        let (tx, rx) = mpsc::channel();
        kept.lock().unwrap().push((job, tx));
        rx
    }));
    fetches
}

/// A message of `account` in its Inbox whose headers alone are here, at the server id `id`.
fn not_here_whole(app: &mut MailApp, account: Id, id: &str) -> Id {
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let server = ServerMessage { id: id.into(), folder: "M1".into(), ..Default::default() };
    let m = Message { account, folder: inbox, subject: "Kickoff".into(), received: NOW, unfetched: true, server: Some(server), ..Default::default() };
    app.session.mailbox.add_message(m).unwrap()
}

/// What the host brings back for a download: each message it asked for, whole.
fn whole(job: &JmapFetchJob) -> GraphFetchResult {
    let fetched =
        job.request.ids.iter().map(|id| GraphFetched { id: id.clone(), mime: b"Subject: Kickoff\r\n\r\nFrom before.\r\n".to_vec() }).collect();
    GraphFetchResult {
        account: job.request.account,
        email: job.request.email.clone(),
        session: job.request.session.clone(),
        fetched,
        ..Default::default()
    }
}

/// The last piece of a run, as the host sends it.
fn last(job: &JmapJob, how: JmapMailResult) -> JmapMailResult {
    JmapMailResult { account: job.request.account, email: job.request.email.clone(), ..how }
}

#[test]
fn the_first_run_asks_for_the_password_and_the_run_goes_with_it() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let kept: Arc<Mutex<Vec<String>>> = Default::default();
    app.services.secrets = Some(Box::new(Recording(kept.clone())));
    assert!(app.enabled("ui.sync"));
    assert_eq!(app.server_accounts(), [ada]);
    assert_eq!(app.execute("ui.sync", json!({"account": ada})).unwrap()["pending"], "password");
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("password"));
    assert!(taken(&jobs).is_empty());
    app.dialog = None;
    // What the dialog runs once it is typed.
    assert_eq!(app.execute("ui.sync", json!({"account": ada, "password": "typed here"})).unwrap()["jmap"], true);
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    assert_eq!(
        (job.secret.as_str(), job.request.session.as_str(), job.request.bearer, job.request.background),
        ("typed here", SESSION, false, false)
    );
    assert!(Arc::ptr_eq(&job.cancel, &app.services.sync_cancel));
    // Under way: one at a time, and Cancel is there.
    assert!(app.syncing() && !app.enabled("ui.sync") && app.enabled("ui.cancelSync"));
    assert_eq!(app.execute("ui.sync", json!({"account": ada})).unwrap_err(), "Send/Receive is already running");
    tx.send(last(&job, JmapMailResult { whole: true, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(!app.syncing());
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);
    // Kept for the session, and never with the system's credential store.
    assert!(app.can_sync_quietly(ada));
    assert!(kept.lock().unwrap().is_empty(), "{:?}", kept.lock().unwrap());
}

#[test]
fn a_password_turned_down_is_forgotten_and_a_round_by_itself_leaves_the_account_out() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    app.execute("ui.sync", json!({"account": ada, "password": "not the one"})).unwrap();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    tx.send(last(&job, JmapMailResult { auth_refused: true, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(app.session.sync_status.starts_with("The server turned down the sign-in for ada@example.org"), "{}", app.session.sync_status);
    assert!(app.has_sync_problems());
    assert!(!app.passwords.contains_key(&ada) && !app.can_sync_quietly(ada));
    // A round by itself: left out, nothing asked.
    (app.auto_sync, app.session.prefs.sync_minutes) = (true, 5);
    app.auto_sync();
    app.auto_sync();
    assert!(taken(&jobs).is_empty() && app.dialog.is_none());
    // The owner's own Send/Receive asks again.
    assert_eq!(app.execute("ui.sync", json!({"account": ada})).unwrap()["pending"], "password");
}

#[test]
fn a_host_that_holds_an_account_s_credential_runs_it_by_itself_with_no_secret() {
    let (mut app, accounts, jobs) = window(&["ada@example.org", "bo@example.org"]);
    let [ada, bo] = [accounts[0], accounts[1]];
    // The host holds Ada's; Bo's, at a server the host has nothing to do with, is the owner's.
    app.services.jmap_host_credentials = Some(Box::new(|a: &mailcraft_model::Account| a.email == "ada@example.org"));
    assert!(app.can_sync_quietly(ada) && !app.can_sync_quietly(bo));
    (app.auto_sync, app.session.prefs.sync_minutes) = (true, 5);
    // The first frame lines the round up, the next starts it.
    app.auto_sync();
    app.auto_sync();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    assert!(job.request.account == ada && job.secret.is_empty() && job.request.background);
    assert!(app.dialog.is_none());
    tx.send(last(&job, JmapMailResult { whole: true, ..Default::default() })).unwrap();
    app.poll_sync();
    // Bo is left out of the round, its row with it: it would have to ask.
    app.auto_sync();
    assert!(taken(&jobs).is_empty() && app.dialog.is_none());
    let state = |account: Id| app.session.sync_progress.accounts.iter().find(|r| r.account == account).map(|r| r.state);
    assert_eq!((state(ada), state(bo)), (Some(RunState::Done), None));
    assert_eq!(app.execute("ui.sync", json!({"account": bo})).unwrap()["pending"], "password");
}

#[test]
fn a_host_credential_turned_down_keeps_rounds_by_themselves_away_until_the_owner_asks() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    app.services.jmap_host_credentials = Some(Box::new(|_: &mailcraft_model::Account| true));
    app.execute("ui.sync", json!({"account": ada})).unwrap();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    tx.send(last(&job, JmapMailResult { auth_refused: true, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(!app.can_sync_quietly(ada));
    (app.auto_sync, app.session.prefs.sync_minutes) = (true, 5);
    app.auto_sync();
    app.auto_sync();
    assert!(taken(&jobs).is_empty() && app.dialog.is_none());
    // The owner asks: it goes again, with the host's own, and rounds by themselves may follow.
    app.execute("ui.sync", json!({"account": ada})).unwrap();
    let [(job, _tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    assert!(job.secret.is_empty());
    assert!(app.can_sync_quietly(ada));
}

#[test]
fn cancel_stops_the_run_and_the_round_with_it() {
    let (mut app, accounts, jobs) = window(&["ada@example.org", "bo@example.org"]);
    let [ada, bo] = [accounts[0], accounts[1]];
    for account in [ada, bo] {
        app.passwords.insert(account, "typed here".into());
    }
    app.execute("ui.sync", json!({})).unwrap();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    assert_eq!(job.request.account, ada);
    app.execute("ui.cancelSync", json!({})).unwrap();
    // The host's run sees it before its next request, and ends there.
    assert!(job.cancel.load(Ordering::Relaxed));
    tx.send(last(&job, JmapMailResult { stopped: true, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(app.session.sync_status.starts_with("Stopped"), "{}", app.session.sync_status);
    app.auto_sync();
    assert!(taken(&jobs).is_empty(), "the rest of the round is called off too");
    let state = |account: Id| app.session.sync_progress.accounts.iter().find(|r| r.account == account).map(|r| r.state);
    assert_eq!((state(ada), state(bo)), (Some(RunState::Stopped), Some(RunState::Stopped)));
}

#[test]
fn a_run_whose_task_ended_early_says_so_and_the_round_goes_on() {
    let (mut app, accounts, jobs) = window(&["ada@example.org", "bo@example.org"]);
    let [ada, bo] = [accounts[0], accounts[1]];
    for account in [ada, bo] {
        app.passwords.insert(account, "typed here".into());
    }
    app.execute("ui.sync", json!({})).unwrap();
    // The task is gone without its last piece.
    drop(taken(&jobs));
    app.poll_sync();
    assert!(!app.syncing());
    assert_eq!(app.session.sync_status, "Send/Receive stopped unexpectedly");
    app.auto_sync();
    let [(job, _tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    assert_eq!(job.request.account, bo);
}

#[test]
fn a_run_under_way_when_another_mailbox_is_opened_brings_nothing_into_it() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    app.execute("ui.sync", json!({"account": ada, "password": "typed here"})).unwrap();
    let [(job, tx)]: [_; 1] = taken(&jobs).try_into().ok().unwrap();
    // Another mailbox takes its place, with an account at the same address on the same server.
    app.execute("file.new", json!({})).unwrap();
    let other = json!({"email": "ada@example.org", "kind": "jmap", "jmap": {"session": SESSION}});
    let other = app.execute("account.add", other).unwrap()["id"].as_u64().unwrap();
    assert!(app.passwords.is_empty(), "what was typed for the mailbox before is gone with it");
    // What the run still brings was for the mailbox before: none of it is taken in, the run is
    // called off and ends.
    let inbox = GraphFolder { id: "M1".into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() };
    let listing = Some(FolderListing { folders: vec![inbox], complete: true });
    let session = job.request.session.clone();
    tx.send(JmapMailResult { partial: true, listing, session: session.clone(), ..last(&job, JmapMailResult::default()) }).unwrap();
    tx.send(last(&job, JmapMailResult { whole: true, session, ..Default::default() })).unwrap();
    app.poll_sync();
    assert!(job.cancel.load(Ordering::Relaxed));
    assert!(!app.syncing());
    assert!(!app.session.mailbox.folders.iter().any(|f| f.server.is_some()), "nothing of the old run is here");
    // The new mailbox's account goes its own way: its Send/Receive asks for its own password.
    assert_eq!(app.execute("ui.sync", json!({"account": other})).unwrap()["pending"], "password");
}

#[test]
fn a_download_under_way_when_another_mailbox_is_opened_brings_nothing_into_it() {
    let (mut app, accounts, _jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let fetches = downloads_kept(&mut app);
    app.passwords.insert(ada, "typed here".into());
    let kickoff = not_here_whole(&mut app, ada, "E1");
    // Opened: it is asked for at once.
    app.session.sel.messages = vec![kickoff];
    app.drive_fetch();
    let asked = std::mem::take(&mut *fetches.lock().unwrap());
    let (job, tx) = asked.into_iter().find(|(job, _)| !job.request.background).unwrap();
    assert_eq!(job.request.ids, ["E1"]);
    // Another mailbox takes its place, with an account at the same address on the same server and
    // a message at the same server id, opened too.
    app.execute("file.new", json!({})).unwrap();
    let other = json!({"email": "ada@example.org", "kind": "jmap", "jmap": {"session": SESSION}});
    let other = app.execute("account.add", other).unwrap()["id"].as_u64().unwrap();
    let theirs = not_here_whole(&mut app, other, "E1");
    // The download is called off, and what it still brings, for the mailbox before, is listened
    // for no longer.
    assert!(job.cancel.load(Ordering::Relaxed));
    assert!(tx.send(whole(&job)).is_err(), "the window stopped listening");
    // The new mailbox's message, opened, is asked for at once with its own password: the old
    // download neither holds it up nor stands for it.
    app.passwords.insert(other, "typed for the new one".into());
    app.session.sel.messages = vec![theirs];
    app.drive_fetch();
    let asked = std::mem::take(&mut *fetches.lock().unwrap());
    let (new, _tx) = asked.into_iter().find(|(job, _)| !job.request.background).unwrap();
    assert!(new.request.account == other && new.request.ids == ["E1"] && !new.cancel.load(Ordering::Relaxed));
    assert!(app.fetching.fetching(theirs) && app.session.mailbox.message(theirs).unwrap().unfetched);
}

#[test]
fn a_download_that_comes_back_after_the_mailbox_was_replaced_is_thrown_away() {
    let (mut app, accounts, _jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let fetches = downloads_kept(&mut app);
    app.passwords.insert(ada, "typed here".into());
    let kickoff = not_here_whole(&mut app, ada, "E1");
    not_here_whole(&mut app, ada, "E2");
    let before = app.session.mailbox.clone();
    // The one opened and the background's batch, both under way.
    app.session.sel.messages = vec![kickoff];
    app.drive_fetch();
    let asked = std::mem::take(&mut *fetches.lock().unwrap());
    assert_eq!(asked.iter().map(|(job, _)| job.request.background).collect::<Vec<_>>(), [false, true]);
    // The mailbox is replaced under the window by one with the same account and messages, and the
    // answers come before the window has looked.
    app.session.replace_mailbox(before);
    for (job, tx) in &asked {
        tx.send(whole(job)).unwrap();
    }
    app.drive_fetch();
    assert!(app.session.mailbox.messages.values().all(|m| m.unfetched), "nothing of the old downloads is here");
}

#[test]
fn a_message_of_an_account_whose_sign_in_was_turned_down_says_why_it_is_not_here() {
    let (mut app, accounts, _jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    let fetches = downloads_kept(&mut app);
    app.passwords.insert(ada, "typed here".into());
    let kickoff = not_here_whole(&mut app, ada, "E1");
    app.jmap_refused.insert(ada);
    let why = "The server turned down the sign-in for this account. Use Send/Receive to sign in again.";
    assert_eq!(app.run("mail.reply", json!({"id": kickoff})).unwrap_err(), why);
    app.session.sel.messages = vec![kickoff];
    app.drive_fetch();
    assert!(fetches.lock().unwrap().is_empty());
    let m = app.session.mailbox.message(kickoff).unwrap().clone();
    assert_eq!(crate::reading::unfetched_said(&app, &m), why);
}

#[test]
fn a_mailbox_from_a_file_reaches_no_jmap_server_until_its_servers_are_confirmed() {
    let (mut app, accounts, jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    app.passwords.insert(ada, "typed here".into());
    app.session.path = Some(std::path::PathBuf::from("from-a-file.mailcraft"));
    app.judge_mailbox();
    assert!(app.services.jmap.is_none() && app.server_accounts().is_empty());
    assert_eq!(app.execute("ui.sync", json!({"account": ada})).unwrap()["pending"], "confirmServers");
    assert!(taken(&jobs).is_empty());
    app.dialog = None;
    app.execute("ui.confirmServers", json!({})).unwrap();
    app.execute("ui.sync", json!({"account": ada})).unwrap();
    assert_eq!(taken(&jobs).len(), 1);
}

#[test]
fn a_secret_goes_only_where_and_as_it_was_typed_for() {
    let (mut app, accounts, _jobs) = window(&["ada@example.org"]);
    let ada = accounts[0];
    app.passwords.insert(ada, "typed here".into());
    app.execute("account.edit", json!({"id": ada, "name": "Ada at work"})).unwrap();
    assert!(app.passwords.contains_key(&ada));
    app.execute("account.edit", json!({"id": ada, "jmap": {"session": "https://jmap.example.org/other/session"}})).unwrap();
    assert!(!app.passwords.contains_key(&ada));
    // Typed for the new one, and Undo puts the old one back: it goes to neither.
    app.passwords.insert(ada, "typed for the other".into());
    app.execute("edit.undo", json!({})).unwrap();
    assert_eq!(app.session.mailbox.account(ada).and_then(|a| a.jmap.as_ref()).map(|j| j.session.as_str()), Some(SESSION));
    assert!(!app.passwords.contains_key(&ada));
    // Nor as a bearer token what was typed as a password.
    app.passwords.insert(ada, "typed as a password".into());
    app.execute("account.edit", json!({"id": ada, "jmap": {"bearer": true}})).unwrap();
    assert!(!app.passwords.contains_key(&ada));
}
