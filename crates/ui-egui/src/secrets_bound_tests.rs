// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Remembered passwords and sign-ins are filed under the servers they go to (C1 PR-b).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult};
use serde_json::json;

use super::*;
use crate::{SecretStore, Services};

const NOW: i64 = 1_791_993_600;

#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<String, String>>>);

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

/// What each Send/Receive was asked to do: (the IMAP host, the password it carried).
type Seen = Arc<Mutex<Vec<(String, String)>>>;

fn app(store: &Store, seen: &Seen) -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let seen = seen.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let host = req.account.incoming.as_ref().map(|s| s.host.clone()).unwrap_or_default();
        seen.lock().unwrap().push((host, req.password.clone()));
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    a.services.secrets = Some(Box::new(store.clone()));
    a
}

const OLD: &str = "password:avery@mail.example|imap=imap.mail.example:993|smtp=smtp.mail.example:465";
const NEW: &str = "password:avery@mail.example|imap=imap.attacker.example:993|smtp=smtp.mail.example:465";

fn add(a: &mut MailApp) -> u64 {
    let r = a.execute(
        "account.add",
        json!({"email": "Avery@Mail.example", "incoming": {"host": "imap.mail.example", "port": 993}, "outgoing": {"host": "smtp.mail.example", "port": 465}}),
    );
    r.unwrap()["id"].as_u64().unwrap()
}

#[test]
fn the_key_names_the_address_and_the_servers() {
    let (store, seen) = (Store::default(), Seen::default());
    let mut a = app(&store, &seen);
    let id = add(&mut a);
    let account = a.session.mailbox.account(id).unwrap();
    assert_eq!(MailApp::secret_key("password", account), OLD);
    let microsoft = mailcraft_model::Account {
        email: "Me@Example.org".into(),
        kind: mailcraft_model::AccountKind::Microsoft,
        incoming: Some(mailcraft_model::Server { host: "ignored".into(), port: 1, security: Default::default() }),
        ..Default::default()
    };
    assert_eq!(MailApp::secret_key("refresh", &microsoft), "refresh:me@example.org|graph");
}

#[test]
fn a_secret_is_not_found_after_a_server_changes_and_the_old_one_goes() {
    let (store, seen) = (Store::default(), Seen::default());
    let mut a = app(&store, &seen);
    let id = add(&mut a);
    store.set(OLD, "pw").unwrap();
    a.passwords.insert(id, "pw".into());
    assert_eq!(a.remembered("password", "avery@mail.example").as_deref(), Some("pw"));

    a.execute("account.edit", json!({"id": id, "incoming": {"host": "imap.attacker.example", "port": 993}})).unwrap();

    assert_eq!(a.remembered("password", "avery@mail.example"), None);
    assert!(store.get(OLD).is_none() && store.get(NEW).is_none());
    assert!(!a.passwords.contains_key(&id));
}

#[test]
fn an_edit_that_leaves_the_servers_keeps_the_secret() {
    let (store, seen) = (Store::default(), Seen::default());
    let mut a = app(&store, &seen);
    let id = add(&mut a);
    store.set(OLD, "pw").unwrap();
    a.execute("account.edit", json!({"id": id, "displayName": "Avery"})).unwrap();
    assert_eq!(store.get(OLD).as_deref(), Some("pw"));
}

#[test]
fn a_password_never_goes_to_another_host() {
    let (store, seen) = (Store::default(), Seen::default());
    let mut a = app(&store, &seen);
    let id = add(&mut a);
    store.set(OLD, "pw").unwrap();
    // Before the change the remembered password is used, quietly, for the host it was given for.
    assert!(a.can_sync_quietly(id));
    a.execute("ui.sync", json!({"account": id})).unwrap();
    a.poll_sync();
    assert_eq!(seen.lock().unwrap().last().cloned(), Some(("imap.mail.example".to_string(), "pw".to_string())));
    seen.lock().unwrap().clear();

    a.execute("account.edit", json!({"id": id, "incoming": {"host": "imap.attacker.example", "port": 993}})).unwrap();
    assert!(!a.can_sync_quietly(id));
    let asked = a.execute("ui.sync", json!({"account": id})).unwrap();
    assert_eq!(asked["pending"], "password");
    a.poll_sync();
    assert!(seen.lock().unwrap().iter().all(|(_, password)| password.is_empty()), "{:?}", seen.lock().unwrap());
}

#[test]
fn secrets_kept_the_old_way_move_once_to_the_servers_of_the_account() {
    let (store, seen) = (Store::default(), Seen::default());
    let mut a = app(&store, &seen);
    add(&mut a);
    store.set("password:avery@mail.example", "pw").unwrap();
    store.set("refresh:avery@mail.example", "ref").unwrap();
    store.set("password:other@mail.example", "not ours").unwrap();

    a.migrate_secrets();

    assert_eq!(store.get(OLD).as_deref(), Some("pw"));
    assert_eq!(store.get("refresh:avery@mail.example|imap=imap.mail.example:993|smtp=smtp.mail.example:465").as_deref(), Some("ref"));
    assert!(store.get("password:avery@mail.example").is_none() && store.get("refresh:avery@mail.example").is_none());
    assert_eq!(store.get("password:other@mail.example").as_deref(), Some("not ours"));
    assert!(a.ui.sign_ins_bound);
    // Once: an entry that turns up under the old name afterwards is left alone.
    store.set("password:avery@mail.example", "later").unwrap();
    a.migrate_secrets();
    assert_eq!(store.get("password:avery@mail.example").as_deref(), Some("later"));
}

#[test]
fn removing_an_account_forgets_both_names() {
    let (store, seen) = (Store::default(), Seen::default());
    let mut a = app(&store, &seen);
    let id = add(&mut a);
    store.set(OLD, "pw").unwrap();
    store.set("password:avery@mail.example", "old").unwrap();
    a.execute("account.remove", json!({"id": id})).unwrap();
    assert!(store.get(OLD).is_none() && store.get("password:avery@mail.example").is_none());
}
