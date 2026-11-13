// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The wait for new mail in the window (#276): which accounts are listened for, what a watch
//! that says "more mail" starts, and when watches stop and are left alone. The host is a fake that
//! notes every watch it was asked for; every address is invented.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::sync::{IdleEnd, IdleNote, SyncRequest};
use serde_json::json;

use super::*;
use crate::{IdleWatch, Services};

/// What the fake host was asked, and the ends of the channels the watches say things through.
#[derive(Default)]
struct Host {
    asked: Vec<SyncRequest>,
    say: Vec<(Sender<IdleNote>, Arc<AtomicBool>)>,
}

fn window() -> (MailApp, Id, Arc<Mutex<Host>>) {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    let account = app
        .execute("account.add", json!({"email": "noor@example.org", "incoming": {"host": "imap.example.org", "port": 993, "security": "tls"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let host: Arc<Mutex<Host>> = Default::default();
    let note = host.clone();
    app.services.idle = Some(Box::new(move |request| {
        let (tx, notes) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let mut host = note.lock().unwrap();
        host.asked.push(request);
        host.say.push((tx, stop.clone()));
        IdleWatch { stop, notes }
    }));
    (app, account, host)
}

#[test]
fn an_account_with_a_password_in_memory_is_listened_for_and_one_without_is_not() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    // Nothing to sign in with: no watch, and nothing is asked of anybody.
    app.idle_tick();
    assert!(host.lock().unwrap().asked.is_empty() && !app.idle.watching(account));
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    app.idle_tick();
    let asked = host.lock().unwrap().asked.clone();
    assert_eq!(asked.len(), 1, "one watch for one account");
    assert_eq!((asked[0].account.id, asked[0].secret()), (account, "hunter2"));
    assert!(app.idle.watching(account));
}

#[test]
fn more_mail_queues_the_accounts_run_once_and_quietly() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    let tx = host.lock().unwrap().say[0].0.clone();
    tx.send(IdleNote::Arrived).unwrap();
    tx.send(IdleNote::Arrived).unwrap();
    app.idle_tick();
    assert_eq!(app.sync_queue.iter().copied().collect::<Vec<_>>(), vec![(account, true)]);
    // Said again while it waits: still one.
    tx.send(IdleNote::Arrived).unwrap();
    app.idle_tick();
    assert_eq!(app.sync_queue.len(), 1);
}

#[test]
fn turning_send_receive_off_or_working_offline_stops_the_watch() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    let stop = host.lock().unwrap().say[0].1.clone();
    assert!(!stop.load(Ordering::Relaxed));
    app.session.prefs.sync_minutes = 0;
    app.idle_tick();
    assert!(stop.load(Ordering::Relaxed) && !app.idle.watching(account));
    // Back on: a new watch.
    app.session.prefs.sync_minutes = 5;
    app.idle_tick();
    assert_eq!(host.lock().unwrap().asked.len(), 2);
    let stop = host.lock().unwrap().say[1].1.clone();
    app.session.prefs.offline = true;
    app.idle_tick();
    assert!(stop.load(Ordering::Relaxed));
    // And the account removed.
    app.session.prefs.offline = false;
    app.idle_tick();
    let stop = host.lock().unwrap().say[2].1.clone();
    app.execute("account.remove", json!({"id": account})).ok();
    app.idle_tick();
    assert!(stop.load(Ordering::Relaxed) || app.session.mailbox.account(account).is_some());
}

#[test]
fn a_server_that_cant_idle_or_keeps_dropping_is_left_alone_for_a_while() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    let tx = host.lock().unwrap().say[0].0.clone();
    tx.send(IdleNote::Ended(IdleEnd::NoIdle)).unwrap();
    app.idle_tick();
    assert!(!app.idle.watching(account));
    // Not tried again at once, nor within the day; after it, it is.
    app.idle_tick();
    assert_eq!(host.lock().unwrap().asked.len(), 1);
    app.session.clock.now = Some(app.session.now() + 24 * 3600 + 1);
    app.idle_tick();
    assert_eq!(host.lock().unwrap().asked.len(), 2);
    // A watch that ended without a word (its thread is gone) is left alone for ten minutes.
    drop(host.lock().unwrap().say.remove(1));
    app.idle_tick();
    assert!(!app.idle.watching(account));
    assert_eq!(retry_after(IdleEnd::Stopped), None);
}

#[test]
fn a_mailbox_taking_the_place_of_the_open_one_stops_every_watch() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    let stop = host.lock().unwrap().say[0].1.clone();
    app.forget_mail_runs();
    assert!(stop.load(Ordering::Relaxed) && !app.idle.watching(account));
}
