// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An own identity provider's client secret is remembered in the credential store, so that the
//! refresh after a restart carries it: without it a confidential client is refused
//! (`invalid_client`) although the sign-in it renews worked. Invented example.org values; nothing
//! leaves the test.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_host::SecretError;
use mailcraft_host::secrets::OAUTH_CLIENT;
use mailcraft_model::Id;
use serde_json::json;

use crate::{MailApp, SecretStore, Services, SignInRequest};

const NOW: i64 = 1_791_993_600;
const ISSUER: &str = "https://id.example.org/realms/mail";
const SECRET: &str = "not-a-real-client-secret";

#[derive(Default)]
struct Store {
    held: Mutex<HashMap<String, String>>,
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
        if let Ok(mut held) = self.held.lock() {
            held.remove(key);
        }
    }
}

fn app(store: &Arc<Store>, session: Session) -> MailApp {
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    a.services.secrets = Some(store.clone());
    a
}

fn own_account(a: &mut MailApp) -> Id {
    let v = json!({"email": "alice@example.org", "incoming": {"host": "mail.example.org"}, "outgoing": {"host": "mail.example.org"},
        "auth": "oauth2", "provider": "custom", "clientId": "mailcraft", "issuer": ISSUER,
        "authorizeUrl": format!("{ISSUER}/auth"), "tokenUrl": format!("{ISSUER}/token"), "hasSecret": true});
    a.execute("account.add", v).unwrap()["id"].as_u64().unwrap()
}

/// What the background read of the credential store brings at a start, taken in.
fn read_store(a: &mut MailApp) {
    a.start_secret_read(false);
    let until = Instant::now() + Duration::from_secs(5);
    while a.secret_read.is_some() && Instant::now() < until {
        a.poll_secret_read();
        std::thread::yield_now();
    }
    assert!(a.secret_read.is_none(), "the read of the credential store never ended");
}

#[test]
fn the_client_secret_is_in_the_store_and_back_after_a_restart() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let id = own_account(&mut a);
    a.set_client_secret(id, SECRET);
    let key = MailApp::secret_key(OAUTH_CLIENT, a.session.mailbox.account(id).unwrap());
    assert_eq!(store.get(&key).as_deref(), Some(SECRET), "the client secret wasn't remembered");
    // A restart: the same mailbox, nothing in memory but what the store gives back.
    let mailbox = a.session.mailbox.clone();
    let mut b = app(&store, Session::at(mailbox, NOW, 0));
    assert_eq!(b.client_secret_of(id), "");
    read_store(&mut b);
    assert_eq!(b.client_secret_of(id), SECRET);
}

#[test]
fn nothing_is_remembered_when_sign_ins_are_not() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    a.ui.remember_sign_in = false;
    let id = own_account(&mut a);
    a.set_client_secret(id, SECRET);
    assert!(store.held.lock().unwrap().is_empty());
    // Still there for this session.
    assert_eq!(a.client_secret_of(id), SECRET);
}

/// A host that counts the sign-ins it is handed (and never answers them).
fn counting(a: &mut MailApp) -> Arc<Mutex<usize>> {
    let asked = Arc::new(Mutex::new(0));
    let count = asked.clone();
    a.services.sign_in = Some(Box::new(move |_req: SignInRequest| {
        *count.lock().unwrap() += 1;
        std::sync::mpsc::channel().1
    }));
    asked
}

#[test]
fn a_client_whose_secret_isnt_here_is_said_so_and_the_provider_never_asked() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let asked = counting(&mut a);
    // As a file from a build that never stored the secret has it: it says there is one.
    let id = own_account(&mut a);
    let oauth = a.session.mailbox.account(id).unwrap().oauth.clone().unwrap();
    let error = a.begin_sign_in(id, oauth.clone(), "alice@example.org", "a-refresh-token".into(), false, false).unwrap_err();
    assert!(error.contains("client secret for alice@example.org isn't stored") && error.contains("Account Settings"), "{error}");
    assert_eq!(*asked.lock().unwrap(), 0, "the provider was asked without the secret");
    // While the credential store is still being read, that is what is said.
    a.start_secret_read(false);
    assert!(a.secret_read.is_some());
    let waiting = a.begin_sign_in(id, oauth.clone(), "alice@example.org", "a-refresh-token".into(), true, false).unwrap_err();
    assert!(waiting.contains("credential store"), "{waiting}");
    read_store(&mut a);
    // Entered again: the sign-in goes ahead, with it.
    a.set_client_secret(id, SECRET);
    a.begin_sign_in(id, oauth, "alice@example.org", "a-refresh-token".into(), false, false).unwrap();
    assert_eq!(*asked.lock().unwrap(), 1);
}

#[test]
fn a_public_client_needs_no_secret() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let asked = counting(&mut a);
    let id = own_account(&mut a);
    a.execute("account.edit", json!({"id": id, "hasSecret": false})).unwrap();
    let oauth = a.session.mailbox.account(id).unwrap().oauth.clone().unwrap();
    a.begin_sign_in(id, oauth, "alice@example.org", "a-refresh-token".into(), false, false).unwrap();
    assert_eq!(*asked.lock().unwrap(), 1);
    assert!(a.session.sync_status.contains("browser"), "{}", a.session.sync_status);
}

/// A client secret kept under settings that then change isn't left behind in the credential
/// store, where it would never be found again (#1844 review).
#[test]
fn a_secret_kept_under_settings_that_changed_is_taken_out_of_the_store() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let id = own_account(&mut a);
    a.set_client_secret(id, SECRET);
    let before = a.session.mailbox.account(id).cloned().unwrap();
    let old = MailApp::secret_key(OAUTH_CLIENT, &before);
    assert!(store.get(&old).is_some());
    a.execute("account.edit", json!({"id": id, "tokenUrl": format!("{ISSUER}/other-token")})).unwrap();
    a.drop_stale_client_secret(&before);
    assert!(store.get(&old).is_none(), "left behind under the old settings");
    // Unchanged settings keep it.
    a.set_client_secret(id, SECRET);
    let kept = a.session.mailbox.account(id).cloned().unwrap();
    a.execute("account.edit", json!({"id": id, "displayName": "Alice"})).unwrap();
    a.drop_stale_client_secret(&kept);
    assert!(store.get(&MailApp::secret_key(OAUTH_CLIENT, &kept)).is_some());
}
