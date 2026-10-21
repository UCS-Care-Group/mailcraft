// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar and contacts are off the mail path (#276): a mailbox's mail no longer waits for
//! another's calendar download, the download is a job of its own after the round, and no two
//! jobs of one account run at once. Hosts are fakes; every address is invented.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::remote::{ContactsDownload, EventsDownload, RemoteContact};
use mailcraft_model::sync::Tokens;
use serde_json::json;

use crate::{GraphRequest, GraphResult, MailApp, Services};

const OWNER: &str = "noor@example.org";
const SHARED: &str = "support@example.org";

/// What each Graph job was asked: the account, whether it was the mail, whether it was the wide
/// stretch of the calendar with the contacts.
type Jobs = Arc<Mutex<Vec<(Id, bool, bool)>>>;
/// The answer of the calendar job that the host holds on to, until the test lets it go.
type Held = Arc<Mutex<Option<(mpsc::Sender<GraphResult>, GraphRequest)>>>;

fn window() -> (MailApp, Id, Id, Jobs, Held) {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let tokens = Tokens { access: "own-access".into(), refresh: "ref-1".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    app.tokens.insert(owner, tokens.clone());
    app.graph_tokens.insert(owner, tokens);
    let shared = app.execute("account.addSharedMailbox", json!({"address": SHARED})).unwrap()["id"].as_u64().unwrap();
    let (jobs, held): (Jobs, Held) = Default::default();
    let (log, keep) = (jobs.clone(), held.clone());
    app.services.graph = Some(Box::new(move |req: GraphRequest| {
        log.lock().unwrap().push((req.account.id, req.mail.is_some(), req.directory));
        let (tx, rx) = mpsc::channel();
        if req.mail.is_some() {
            // Mail: nothing to bring, answered at once.
            if let Some(to) = &req.mail_pieces {
                let folder = mailcraft_model::sync::GraphFolder {
                    id: "in".into(),
                    name: "Inbox".into(),
                    kind: mailcraft_model::FolderKind::Inbox,
                    ..Default::default()
                };
                let listing = mailcraft_model::sync::FolderListing { folders: vec![folder], complete: true };
                to.send(mailcraft_model::sync::GraphMailResult {
                    account: req.account.id,
                    email: req.account.email.clone(),
                    listing: Some(listing),
                    ..Default::default()
                })
                .unwrap();
            }
            tx.send(GraphResult { account: req.account.id, ..Default::default() }).unwrap();
        } else {
            // The calendar and contacts: a long download, held on to.
            *keep.lock().unwrap() = Some((tx, req));
        }
        rx
    }));
    app.services.sign_in = Some(Box::new(|_| mpsc::channel().1));
    app.services.sync = Some(Box::new(|_| {
        let (_, rx) = mpsc::channel();
        rx
    }));
    (app, owner, shared, jobs, held)
}

/// A few frames of the window: the round goes on, and the calendar is looked at.
fn frames(app: &mut MailApp, calendar_at: f64) -> bool {
    let mut calendar_started = false;
    for _ in 0..12 {
        app.auto_sync();
        app.poll_graph();
        calendar_started |= app.calendar_tick_at(true, calendar_at);
        app.poll_calendar();
    }
    calendar_started
}

#[test]
fn a_second_mailboxes_mail_starts_before_the_first_ones_calendar_download_ends() {
    let (mut app, owner, shared, jobs, held) = window();
    // (No new round of its own while the test looks.)
    app.last_auto_sync = crate::now_ms();
    app.execute("ui.sync", json!({})).unwrap();
    // The whole round, with the calendar's looks in between: the calendar waits for the round.
    let started_in_round = frames(&mut app, mailcraft_time::now() as f64 * 1_000.0);
    let mail = |job: &(Id, bool, bool)| (job.0, job.1, job.2);
    let log = jobs.lock().unwrap().clone();
    assert_eq!(log.iter().map(mail).collect::<Vec<_>>()[..2], [(owner, true, false), (shared, true, false)], "both mailboxes' mail, no wide stretch");
    // The owner's mail job asked for no calendar and no contacts.
    assert!(log.iter().filter(|job| job.1).all(|job| !job.2));
    // Send/Receive Progress still has each account, done.
    assert_eq!(app.session.sync_progress.accounts.iter().map(|r| r.account).collect::<Vec<_>>(), [owner, shared]);
    let states: Vec<_> = app.session.sync_progress.accounts.iter().map(|r| r.state).collect();
    assert!(states.iter().all(|s| *s == mailcraft_engine::cmd::sync_progress::RunState::Done), "{states:?}");
    // The round is over: the download is a job of its own now (it may have started in the frames
    // above, once the round ended).
    let calendar_job = |log: &[(Id, bool, bool)]| log.iter().position(|job| !job.1);
    assert!(started_in_round || calendar_job(&jobs.lock().unwrap()).is_some() || app.calendar_tick_at(true, 0.0));
    let log = jobs.lock().unwrap().clone();
    let at = calendar_job(&log).expect("the calendar job started");
    assert!(at >= 2, "after both mailboxes' mail, never before");
    assert_eq!((log[at].0, log[at].2), (owner, true), "the owner's wide stretch and contacts");
    assert!(app.calendar_syncing() && held.lock().unwrap().is_some(), "still downloading");
    // Never two jobs of one account at once, nor of a shared mailbox and the sign-in it uses.
    assert!(app.execute("ui.sync", json!({"account": owner})).is_err());
    assert!(app.execute("ui.sync", json!({"account": shared})).is_err());
    assert_eq!(jobs.lock().unwrap().len(), at + 1);
    // The download ends: the contacts are in, and the next run is free to start.
    let (answer, req) = held.lock().unwrap().take().unwrap();
    let events = EventsDownload { from: 0, to: 1, complete: true, ..Default::default() }.for_account(&req.account);
    let contacts = ContactsDownload {
        complete: true,
        contacts: vec![RemoteContact { id: "c-1".into(), display_name: "Remote Person".into(), ..Default::default() }],
        ..Default::default()
    }
    .for_account(&req.account);
    answer.send(GraphResult { account: owner, events: Some(events), contacts: Some(contacts), ..Default::default() }).unwrap();
    app.poll_calendar();
    assert!(!app.calendar_syncing());
    assert!(app.session.mailbox.contacts.values().any(|c| c.name() == "Remote Person"));
    assert!(app.graph_checked.contains_key(&owner), "a quarter of an hour until the next");
    assert!(app.execute("ui.sync", json!({"account": owner})).is_ok());
}

#[test]
fn the_calendar_download_waits_for_the_round_and_a_round_waits_for_a_calendar_job_of_its_account() {
    let (mut app, owner, _, jobs, held) = window();
    // Nothing is under way: the calendar job starts, with the wide stretch.
    assert!(app.calendar_tick_at(true, 1_000.0));
    assert_eq!(jobs.lock().unwrap()[0], (owner, false, true));
    // A round the window starts by itself waits its turn for that account, and goes on after.
    app.sync_queue.push_back((owner, true));
    app.inbox_queue.push_back((owner, true));
    app.auto_sync();
    assert_eq!((app.inbox_queue.len(), app.sync_queue.len()), (1, 1), "the Inbox pass and the run wait");
    assert_eq!(jobs.lock().unwrap().len(), 1);
    let (answer, req) = held.lock().unwrap().take().unwrap();
    answer
        .send(GraphResult {
            account: owner,
            events: Some(EventsDownload { complete: true, ..Default::default() }.for_account(&req.account)),
            ..Default::default()
        })
        .unwrap();
    app.poll_calendar();
    app.auto_sync();
    assert!(jobs.lock().unwrap().iter().any(|job| job.0 == owner && job.1), "the mail went on after");
}
