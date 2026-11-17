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
    assert_eq!(app.session.driver.round.queue.iter().copied().collect::<Vec<_>>(), vec![(account, true)]);
    // Said again while it waits: still one.
    tx.send(IdleNote::Arrived).unwrap();
    app.idle_tick();
    assert_eq!(app.session.driver.round.queue.len(), 1);
}

#[test]
fn more_mail_turns_a_waiting_push_only_run_into_a_whole_one() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    // The owner flagged a message: the account waits in line to send that change only.
    app.session.driver.round.queue.push_back((account, true));
    app.session.driver.round.push_only_next.insert(account);
    let tx = host.lock().unwrap().say[0].0.clone();
    tx.send(IdleNote::Arrived).unwrap();
    app.idle_tick();
    assert_eq!(app.session.driver.round.queue.len(), 1);
    assert!(!app.session.driver.round.push_only_next.contains(&account));
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
    app.execute("account.remove", json!({"id": account})).unwrap();
    assert!(app.session.mailbox.account(account).is_none());
    app.idle_tick();
    assert!(stop.load(Ordering::Relaxed) && !app.idle.watching(account));
}

#[test]
fn the_drivers_switch_is_the_one_that_counts() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    app.passwords.insert(account, "hunter2".into());
    app.session.driver.set_auto_sync(false);
    app.idle_tick();
    assert!(host.lock().unwrap().asked.is_empty());
    app.session.driver.set_auto_sync(true);
    app.idle_tick();
    assert_eq!(host.lock().unwrap().asked.len(), 1);
}

#[test]
fn editing_the_account_or_losing_its_secret_ends_the_watch_and_a_new_one_has_the_new_server() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    let first = host.lock().unwrap().say[0].1.clone();
    // The server is changed: the old watch stops, and the next one goes to the new host.
    app.execute("account.edit", json!({"id": account, "incoming": {"host": "imap2.example.org", "port": 993, "security": "tls"}})).unwrap();
    app.idle_tick();
    assert!(first.load(Ordering::Relaxed) && !app.idle.watching(account));
    // (The edit forgets the password; entered again, it is a new watch, at the new server.)
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    let asked = host.lock().unwrap().asked.clone();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].account.incoming.as_ref().map(|s| s.host.as_str()), Some("imap2.example.org"));
    // The secret leaves memory (signed out): the watch stops and none is started.
    let second = host.lock().unwrap().say[1].1.clone();
    app.passwords.remove(&account);
    app.idle_tick();
    assert!(second.load(Ordering::Relaxed) && !app.idle.watching(account));
    assert_eq!(host.lock().unwrap().asked.len(), 2);
    // A changed password alone is a new watch too, and so is a changed server with the secret kept.
    app.passwords.insert(account, "hunter2".into());
    app.idle_tick();
    let third = host.lock().unwrap().say[2].1.clone();
    app.session.mailbox.account_mut(account).unwrap().incoming.as_mut().unwrap().host = "imap3.example.org".into();
    app.idle_tick();
    assert!(third.load(Ordering::Relaxed));
    assert_eq!(host.lock().unwrap().asked.len(), 4);
    // A new password is a new watch.
    app.passwords.insert(account, "other".into());
    app.idle_tick();
    assert_eq!(host.lock().unwrap().asked.len(), 5);
}

#[test]
fn news_while_a_round_is_under_way_queues_the_account_without_wiping_the_rows() {
    let (mut app, account, host) = window();
    app.session.prefs.sync_minutes = 5;
    app.passwords.insert(account, "hunter2".into());
    app.session.sync_progress.begin(&[account, 999]);
    assert!(app.session.sync_progress.running());
    let before = app.session.sync_progress.accounts.len();
    app.idle_tick();
    host.lock().unwrap().say[0].0.send(IdleNote::Arrived).unwrap();
    app.idle_tick();
    assert_eq!(app.session.driver.round.queue.len(), 1);
    assert_eq!(app.session.sync_progress.accounts.len(), before);
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
