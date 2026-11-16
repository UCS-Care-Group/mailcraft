// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The passwords of an account's calendar and contacts servers (#209): held per origin, and what
//! was kept for a server that changed or went is not given to the next.

use mailcraft_engine::Session;
use mailcraft_model::Id;
use serde_json::json;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::secret_read::SecretRead;
use crate::{MailApp, SecretStore, Services};
use mailcraft_host::SecretError;

/// A store that keeps what it is given, and, once stuck (a locked keychain), keeps it whatever it
/// is told to delete.
#[derive(Default)]
struct Store {
    held: Mutex<HashMap<String, String>>,
    stuck: std::sync::atomic::AtomicBool,
}

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.held.lock().ok()?.get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.held.lock().map_err(|_| SecretError::Refused)?.insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        if !self.stuck.load(std::sync::atomic::Ordering::Relaxed)
            && let Ok(mut held) = self.held.lock()
        {
            held.remove(key);
        }
    }
}

const NOW: i64 = 1_791_993_600;
const A: &str = "https://dav.example.org";
const B: &str = "https://cards.example.org";

fn app() -> (MailApp, Id) {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    a.session.dav_enabled = true;
    let id = a.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    a.execute("account.davSet", json!({"id": id, "calendarHomes": [format!("{A}/cal/")], "contactHomes": [format!("{B}/c/")]})).unwrap();
    a.dav_passwords.insert((id, A.into()), "one".into());
    a.dav_passwords.insert((id, B.into()), "two".into());
    (a, id)
}

#[test]
fn a_password_is_held_for_its_origin_and_never_named_like_the_mail_password() {
    let (a, id) = app();
    let account = a.session.mailbox.account(id).unwrap();
    let (cal, card) = (MailApp::dav_secret_key(account, A).unwrap(), MailApp::dav_secret_key(account, B).unwrap());
    assert_ne!(cal, card);
    assert_ne!(cal, MailApp::secret_key("password", account));
    assert_eq!(MailApp::dav_secret_keys(account).len(), 2);
}

#[test]
fn a_home_that_goes_takes_only_its_origins_password() {
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "contactHomes": []})).unwrap();
    assert_eq!(a.dav_passwords.get(&(id, A.to_string())).map(String::as_str), Some("one"));
    assert!(!a.dav_passwords.contains_key(&(id, B.to_string())));
}

#[test]
fn another_user_name_or_certificate_gives_the_new_server_none_of_the_old_ones() {
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    assert!(a.dav_passwords.is_empty());
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "trusted": "ab".repeat(32)})).unwrap();
    assert!(a.dav_passwords.is_empty());
    // The password flag is no other server.
    let (mut a, id) = app();
    a.execute("account.davSet", json!({"id": id, "separatePassword": true})).unwrap();
    assert_eq!(a.dav_passwords.len(), 2);
}

#[test]
fn putting_the_server_away_or_removing_the_account_forgets_them() {
    let (mut a, id) = app();
    a.execute("account.davOff", json!({"id": id})).unwrap();
    assert!(a.dav_passwords.is_empty());
    let (mut a, id) = app();
    a.dav_passwords.insert((id + 1000, A.into()), "other account".into());
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert_eq!(a.dav_passwords.len(), 1, "only that account's go");
}

#[test]
fn another_mailbox_drops_them_all() {
    let (mut a, _) = app();
    a.execute("file.new", json!({})).unwrap();
    assert!(a.dav_passwords.is_empty());
}

fn with_store(stuck: bool) -> (MailApp, Id, Arc<Store>) {
    let (mut a, id) = app();
    let store = Arc::new(Store::default());
    a.services.secrets = Some(store.clone());
    let key = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    store.set(&key, "one").unwrap();
    a.secret_cache.insert(key.clone(), "one".into());
    a.index_secret_key(key);
    store.stuck.store(stuck, std::sync::atomic::Ordering::Relaxed);
    (a, id, store)
}

#[test]
fn a_dropped_key_leaves_the_store_and_the_note_when_the_store_answers() {
    let (mut a, id, store) = with_store(false);
    let key = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    assert!(a.ui.remembered_keys.contains(&key), "noted until the store has said it is gone");
    settle(&mut a);
    assert!(store.get(&key).is_none() && !a.secret_cache.contains_key(&key) && !a.ui.remembered_keys.contains(&key));
}

#[test]
fn a_locked_store_leaves_the_note_for_a_later_try_but_not_the_copy_in_memory() {
    let (mut a, id, store) = with_store(true);
    let key = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    assert!(!a.secret_cache.contains_key(&key), "the copy in memory goes at once");
    settle(&mut a);
    assert!(store.get(&key).is_some(), "the store kept it");
    assert!(a.ui.remembered_keys.contains(&key), "so the note stays, for Remove all data and the next try");
}

#[test]
fn a_read_that_was_out_when_the_key_was_dropped_does_not_bring_it_back() {
    let (mut a, id, _) = with_store(false);
    let old = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    let new = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let answers = vec![(old.clone(), Some("one".to_string())), (new.clone(), Some("fresh".to_string()))];
    send.send(SecretRead { answers, moved: Vec::new(), migrated: false, trouble: None }).unwrap();
    a.secret_read = Some((a.session.openings(), receive));
    a.poll_secret_read();
    assert!(!a.secret_cache.contains_key(&old), "a name the settings no longer give is let go");
    assert_eq!(a.secret_cache.get(&new).map(String::as_str), Some("fresh"));
}

/// Waits for the threads forgetting entries, and takes in what they found.
fn settle(a: &mut MailApp) {
    for _ in 0..500 {
        a.poll_dav_forgetting();
        if a.dav_forgetting.is_empty() && a.dav_forget_queue.is_empty() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn the_window_thread_never_reads_the_store_to_forget() {
    // A store whose `get` would wait for its owner: forgetting reads through `read` only, on a
    // thread of its own, and the window's call returns at once.
    struct Waits(Store, std::thread::ThreadId);
    impl SecretStore for Waits {
        fn get(&self, key: &str) -> Option<String> {
            assert_ne!(std::thread::current().id(), self.1, "the window's own thread never calls get");
            self.0.get(key)
        }
        fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
            self.0.set(key, secret)
        }
        fn delete(&self, key: &str) {
            self.0.delete(key);
        }
    }
    let (mut a, id) = app();
    let store = Arc::new(Waits(Store::default(), std::thread::current().id()));
    a.services.secrets = Some(store.clone());
    let key = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    store.set(&key, "one").unwrap();
    a.index_secret_key(key.clone());
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    settle(&mut a);
    assert!(store.0.get(&key).is_none() && !a.ui.remembered_keys.contains(&key));
}

#[test]
fn undoing_a_server_change_forgets_the_new_servers_passwords_and_keeps_the_mail_ones() {
    let (mut a, id, store) = with_store(false);
    a.passwords.insert(id, "mail".into());
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    settle(&mut a);
    let grace = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    store.set(&grace, "two").unwrap();
    a.dav_passwords.insert((id, A.into()), "two".into());
    a.index_secret_key(grace.clone());
    a.execute("edit.undo", json!({})).unwrap();
    settle(&mut a);
    assert_eq!(a.session.mailbox.account(id).unwrap().dav.as_ref().unwrap().username, "");
    assert_eq!(a.passwords.get(&id).map(String::as_str), Some("mail"), "no reason to ask for the mail password again");
    assert!(a.dav_passwords.is_empty() && store.get(&grace).is_none(), "the password for the server undone is forgotten");
}

#[test]
fn a_change_of_mail_server_leaves_the_calendar_and_contacts_passwords() {
    let (mut a, id, store) = with_store(false);
    let key = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    a.execute("account.edit", json!({"id": id, "incoming": {"host": "imap2.example.org"}})).unwrap();
    settle(&mut a);
    assert!(store.get(&key).is_some() && a.dav_passwords.len() == 2);
    // Removing the account takes them.
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert!(store.get(&key).is_none());
}

#[test]
fn a_name_the_settings_give_again_stays_noted_when_the_store_confirms_it_gone() {
    let (mut a, id, _) = with_store(false);
    let key = MailApp::dav_secret_key(a.session.mailbox.account(id).unwrap(), A).unwrap();
    // The server is changed, so the old name goes to the store's thread; before that answers the
    // settings are put back and a password is held under the name again.
    a.execute("account.davSet", json!({"id": id, "username": "grace"})).unwrap();
    a.execute("account.davSet", json!({"id": id, "username": ""})).unwrap();
    a.index_secret_key(key.clone());
    settle(&mut a);
    assert!(a.ui.remembered_keys.contains(&key), "in use again: still noted");
}

#[test]
fn names_past_the_limit_wait_in_a_queue_and_none_is_lost() {
    let (mut a, _, _) = with_store(false);
    let keys: Vec<String> = (0..900).map(|n| format!("dav-password:x{n}@example.org|dav=https://h|user=u|pin=")).collect();
    for key in &keys {
        a.index_secret_key(key.clone());
    }
    for chunk in keys.chunks(50) {
        a.forget_dav_entries(chunk.to_vec());
    }
    assert!(a.dav_forgetting.len() <= 16, "no more threads than the limit");
    assert!(keys.iter().all(|k| a.ui.remembered_keys.contains(k)), "noted until the store says so");
    settle(&mut a);
    assert!(a.dav_forget_queue.is_empty() && a.dav_forgetting.is_empty());
    assert!(keys.iter().all(|k| !a.ui.remembered_keys.contains(k)), "each was asked after, and confirmed");
}
