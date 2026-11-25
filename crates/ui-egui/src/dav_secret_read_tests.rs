// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A remembered password of their own for an account's calendar servers (#209), read where it is
//! used: by the DAV job, off the window's thread, each name once in a session.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_host::SecretError;
use mailcraft_model::{DavDownload, Id};
use serde_json::json;

use crate::{DavDone, DavJob, MailApp, SecretStore, Services};

const NOW: i64 = 1_791_993_600;
const HOME: &str = "https://dav.example.org";

/// A store that keeps what it is given and counts how often it is asked.
#[derive(Default)]
struct Store {
    held: Mutex<HashMap<String, String>>,
    asked: AtomicUsize,
}

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.held.lock().ok()?.get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.held.lock().map_err(|_| SecretError::Refused)?.insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        if let Ok(mut held) = self.held.lock() {
            held.remove(key);
        }
    }
}

type Sent = Arc<Mutex<Vec<HashMap<String, String>>>>;

/// A window with one account whose calendar server has a password of its own (or shares the
/// mail one), a store, and a host that notes the passwords each job was given.
fn app(store: &Arc<Store>, separate: bool) -> (MailApp, Id, Sent) {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let services = Services { secrets: Some(store.clone()), ..Services::default() };
    let mut a = MailApp::new(session, services);
    a.autosave = false;
    a.session.dav_enabled = true;
    let id = a.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    a.execute("account.davSet", json!({"id": id, "calendarHomes": [format!("{HOME}/cal/ada/")], "separatePassword": separate})).unwrap();
    let sent: Sent = Arc::default();
    let seen = sent.clone();
    a.services.dav = Some(Box::new(move |job: DavJob| {
        seen.lock().unwrap().push(job.passwords.clone());
        let (send, receive) = std::sync::mpsc::channel();
        let download = DavDownload { account: job.ask.account, email: job.ask.email.clone(), server: job.ask.server.clone(), ..Default::default() };
        send.send(DavDone { download }).unwrap();
        receive
    }));
    (a, id, sent)
}

fn key(a: &MailApp, id: Id) -> String {
    MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), HOME).unwrap()
}

/// Tick until a job has started and its answer is taken in (or give up: none was to start).
fn until_a_job(a: &mut MailApp, sent: &Sent) {
    for _ in 0..2_000 {
        a.dav_tick();
        if !sent.lock().unwrap().is_empty() {
            a.dav_tick();
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_remembered_password_of_their_own_is_read_and_given_to_the_job() {
    let store = Arc::new(Store::default());
    let (mut a, id, sent) = app(&store, true);
    store.set(&key(&a, id), "own").unwrap();
    until_a_job(&mut a, &sent);
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].get(HOME).map(String::as_str), Some("own"));
    assert!(a.ui.remembered_keys.contains(&key(&a, id)), "noted for Remove all data");
}

#[test]
fn a_name_is_asked_once_a_session_and_a_miss_stays_a_miss() {
    let store = Arc::new(Store::default());
    let (mut a, _, sent) = app(&store, true);
    until_a_job(&mut a, &sent);
    assert_eq!(store.asked.load(Ordering::SeqCst), 1);
    assert_eq!(sent.lock().unwrap()[0].len(), 0, "nothing to give");
    // The next jobs don't ask again (a locked store would prompt at every one).
    a.dav.next = 0.0;
    a.dav.trouble.clear();
    a.dav_tick();
    a.dav_tick();
    assert_eq!(store.asked.load(Ordering::SeqCst), 1);
}

#[test]
fn another_user_name_asks_for_its_own_name_and_never_gets_the_old_one() {
    let store = Arc::new(Store::default());
    let (mut a, id, sent) = app(&store, true);
    let old = key(&a, id);
    store.set(&old, "old").unwrap();
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    let new = key(&a, id);
    assert_ne!(old, new);
    until_a_job(&mut a, &sent);
    assert_eq!(sent.lock().unwrap()[0].get(HOME), None);
    assert!(!a.secret_cache.contains_key(&old));
}

#[test]
fn nothing_is_asked_while_remembering_is_off_or_for_a_mailbox_nobody_confirmed() {
    for off in [true, false] {
        let store = Arc::new(Store::default());
        let (mut a, id, sent) = app(&store, true);
        store.set(&key(&a, id), "own").unwrap();
        if off {
            a.ui.remember_sign_in = false;
        } else {
            a.unconfirmed = true;
        }
        until_a_job(&mut a, &sent);
        assert_eq!(store.asked.load(Ordering::SeqCst), 0);
        assert!(sent.lock().unwrap().iter().all(|passwords| passwords.is_empty()));
    }
}

#[test]
fn a_name_with_a_forget_pending_is_not_asked() {
    let store = Arc::new(Store::default());
    let (mut a, id, sent) = app(&store, true);
    let k = key(&a, id);
    store.set(&k, "own").unwrap();
    a.dav_forget_queue.push(k.clone());
    until_a_job(&mut a, &sent);
    assert_eq!(store.asked.load(Ordering::SeqCst), 0);
    assert!(!a.secret_cache.contains_key(&k));
}

#[test]
fn servers_that_share_the_mail_password_need_nothing_read() {
    let store = Arc::new(Store::default());
    let (mut a, id, sent) = app(&store, false);
    a.passwords.insert(id, "mail".into());
    until_a_job(&mut a, &sent);
    assert_eq!(store.asked.load(Ordering::SeqCst), 0);
    assert_eq!(sent.lock().unwrap()[0].get(HOME).map(String::as_str), Some("mail"));
}

/// A window whose host signs in to the home with whatever it is given (`took`), or refuses it.
fn signing_in(store: &Arc<Store>, separate: bool, took: bool) -> (MailApp, Id, Sent) {
    let (mut a, id, sent) = app(store, separate);
    let seen = sent.clone();
    a.services.dav = Some(Box::new(move |job: DavJob| {
        seen.lock().unwrap().push(job.passwords.clone());
        let (send, receive) = std::sync::mpsc::channel();
        let (signed_in, problem) =
            if took { (vec![HOME.to_string()], mailcraft_model::DavProblem::None) } else { (vec![], mailcraft_model::DavProblem::WrongPassword) };
        let download = DavDownload {
            account: job.ask.account,
            email: job.ask.email.clone(),
            server: job.ask.server.clone(),
            signed_in,
            problem,
            ..Default::default()
        };
        send.send(DavDone { download }).unwrap();
        receive
    }));
    (a, id, sent)
}

#[test]
fn a_password_the_server_took_is_remembered_when_the_owner_lets_it_be() {
    let store = Arc::new(Store::default());
    let (mut a, id, sent) = signing_in(&store, true, true);
    a.dav_passwords.insert((id, HOME.into()), "typed".into());
    until_a_job(&mut a, &sent);
    let k = key(&a, id);
    assert_eq!(store.held.lock().unwrap().get(&k).map(String::as_str), Some("typed"));
    assert!(a.ui.remembered_keys.contains(&k), "noted for Remove all data");
    // A new window finds it there: the next session reads it back.
    let (mut b, id, sent) = app(&store, true);
    until_a_job(&mut b, &sent);
    assert_eq!(sent.lock().unwrap()[0].get(HOME).map(String::as_str), Some("typed"));
    assert_eq!(key(&b, id), k);
}

#[test]
fn nothing_is_written_while_remembering_is_off_or_for_a_refused_password() {
    for (remember, took) in [(false, true), (true, false)] {
        let store = Arc::new(Store::default());
        let (mut a, id, sent) = signing_in(&store, true, took);
        a.ui.remember_sign_in = remember;
        a.dav_passwords.insert((id, HOME.into()), "typed".into());
        until_a_job(&mut a, &sent);
        assert!(store.held.lock().unwrap().is_empty(), "remember {remember}, took {took}");
    }
}

#[test]
fn the_mail_password_is_never_written_under_a_calendar_server_s_name() {
    let store = Arc::new(Store::default());
    let (mut a, id, sent) = signing_in(&store, false, true);
    a.passwords.insert(id, "mail".into());
    until_a_job(&mut a, &sent);
    assert_eq!(sent.lock().unwrap()[0].get(HOME).map(String::as_str), Some("mail"));
    assert!(store.held.lock().unwrap().is_empty());
}

#[test]
fn a_password_is_not_written_while_a_forget_is_pending_for_its_name() {
    let store = Arc::new(Store::default());
    let (mut a, id, _) = signing_in(&store, true, true);
    let k = key(&a, id);
    a.dav_forget_queue.push(k.clone());
    a.remember_dav_password(id, HOME, "typed");
    assert!(store.held.lock().unwrap().is_empty());
    a.dav_forget_queue.clear();
    a.remember_dav_password(id, HOME, "typed");
    assert_eq!(store.held.lock().unwrap().get(&k).map(String::as_str), Some("typed"));
}

#[test]
fn a_remembered_password_the_server_refused_is_asked_for_again() {
    let store = Arc::new(Store::default());
    let (mut a, id, sent) = signing_in(&store, true, false);
    store.set(&key(&a, id), "old").unwrap();
    until_a_job(&mut a, &sent);
    assert_eq!(sent.lock().unwrap()[0].get(HOME).map(String::as_str), Some("old"), "it was tried");
    assert_eq!(a.dav_password(id, HOME), None, "not offered again");
    assert_eq!(a.dav_origins_wanting_password(id), vec![HOME.to_string()], "File › Info asks for another");
    // A password typed after it is not touched by the refusal of the remembered one.
    assert!(a.hold_dav_password(id, HOME, "new"));
    assert_eq!(a.dav_password(id, HOME).as_deref(), Some("new"));
}

#[test]
fn a_read_that_was_under_way_when_its_name_was_forgotten_brings_nothing_back() {
    let store = Arc::new(Store::default());
    let (mut a, id, _) = app(&store, true);
    let k = key(&a, id);
    store.set(&k, "old").unwrap();
    assert!(a.dav_secrets_first(), "the read starts");
    // Meanwhile the servers change and change back: the old name is forgotten and given again.
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    a.execute("account.davSet", json!({"id": id, "username": ""})).unwrap();
    assert_eq!(key(&a, id), k);
    for _ in 0..2_000 {
        a.poll_dav_forgetting();
        if !a.dav_forget_pending(&k) && !a.dav.secrets.is_reading() {
            break;
        }
        let _ = a.dav_secrets_first();
        std::thread::sleep(Duration::from_millis(1));
    }
    let _ = a.dav_secrets_first();
    assert_eq!(a.dav_password(id, HOME), None, "what the read brought from before the forget isn't taken in");
}
