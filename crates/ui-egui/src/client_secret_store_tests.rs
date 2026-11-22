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

use crate::{MailApp, SecretStore, Services};

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
