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

/// Restoring a Microsoft or Google account's default OAuth settings takes away its own client id:
/// a refresh token held for the other client is let go of with it, so the next renewal doesn't
/// send it for the wrong client (#1844 review).
#[test]
fn restoring_the_default_oauth_settings_lets_go_of_the_sign_in_for_the_other_client() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let v = json!({"email": "ada@gmail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"},
        "auth": "oauth2", "provider": "google", "clientId": "my-own-client.apps.example"});
    let id = a.execute("account.add", v).unwrap()["id"].as_u64().unwrap();
    let held = mailcraft_model::sync::Tokens { access: "access".into(), refresh: "refresh".into(), expires_at: NOW + 3_600, ..Default::default() };
    a.tokens.insert(id, held);
    a.execute("account.oauthRestoreDefaults", json!({"id": id})).unwrap();
    assert!(a.tokens.get(&id).is_none_or(|t| t.refresh.is_empty()), "the refresh token for the other client is still held");
}

/// A client secret typed while setting up an account goes to it only when it was typed for the
/// settings the account has: one typed for a provider given up on since goes nowhere (#1844
/// review).
#[test]
fn a_setup_secret_typed_for_other_settings_is_not_adopted() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let id = own_account(&mut a);
    let mut other = a.session.mailbox.account(id).unwrap().oauth.clone().unwrap();
    other.token = "https://elsewhere.example.org/token".into();
    a.set_setup_client_secret(&other, SECRET);
    let tokens = mailcraft_model::sync::Tokens { access: "access".into(), refresh: "refresh".into(), expires_at: NOW + 3_600, ..Default::default() };
    a.adopt_tokens(id, tokens.clone());
    assert_eq!(a.client_secret_of(id), "");
    assert!(!a.client_secrets.contains_key(&0), "the setup secret is let go of either way");
    assert!(store.get(&MailApp::secret_key(OAUTH_CLIENT, a.session.mailbox.account(id).unwrap())).is_none());
    // Typed for the settings it has: it is the account's.
    let own = a.session.mailbox.account(id).unwrap().oauth.clone().unwrap();
    a.set_setup_client_secret(&own, SECRET);
    a.adopt_tokens(id, tokens);
    assert_eq!(a.client_secret_of(id), SECRET);
}

/// A renewal after a refused token that the owner calls off lets go of its mark, so that the next
/// run may renew again: the answer that would have cleared it is never read (#1844 review).
#[test]
fn a_cancelled_renewal_after_a_refused_token_may_be_tried_again() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let id = own_account(&mut a);
    let (_kept, rx) = std::sync::mpsc::channel();
    a.signing_in = Some(crate::SigningIn {
        graph: false,
        favorites: false,
        quiet: true,
        account: id,
        snapshot: a.session.mailbox.account(id).cloned(),
        shared: None,
        sync: false,
        browser: true,
        opening: a.session.openings(),
        rx,
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    a.token_retried.insert(id);
    a.execute("ui.cancelSignIn", json!({})).unwrap();
    assert!(!a.token_retried.contains(&id));
}

/// A renewal after a refused token that can't start (another account's sign-in is waiting) marks
/// nothing and ends nothing: the other account keeps its one retry, never a second (#1844 review).
#[test]
fn a_renewal_that_cant_start_leaves_another_accounts_retry_alone() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let first = own_account(&mut a);
    let v = json!({"email": "bo@example.org", "incoming": {"host": "mail.example.org"}, "outgoing": {"host": "mail.example.org"},
        "auth": "oauth2", "provider": "custom", "clientId": "mailcraft", "issuer": ISSUER,
        "authorizeUrl": format!("{ISSUER}/auth"), "tokenUrl": format!("{ISSUER}/token")});
    let second = a.execute("account.add", v).unwrap()["id"].as_u64().unwrap();
    let (_kept, rx) = std::sync::mpsc::channel();
    a.signing_in = Some(crate::SigningIn {
        graph: false,
        favorites: false,
        quiet: true,
        account: first,
        snapshot: a.session.mailbox.account(first).cloned(),
        shared: None,
        sync: false,
        browser: false,
        opening: a.session.openings(),
        rx,
        cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    a.token_retried.insert(first);
    a.tokens.insert(second, mailcraft_model::sync::Tokens { refresh: "not-a-real-refresh-token".into(), ..Default::default() });
    a.retry_refused_token(second, true);
    assert!(a.token_retried.contains(&first), "the first account's retry was ended");
    assert!(!a.token_retried.contains(&second), "a renewal that never started was marked");
    // Nor when this build can't sign in at all.
    a.signing_in = None;
    a.retry_refused_token(second, true);
    assert_eq!(a.token_retried.iter().copied().collect::<Vec<_>>(), [first]);
}

/// One run of `account` handed over to the window as it ends: refused (a sign-in error) or not,
/// called off to go on at once or not.
fn end_run(a: &mut MailApp, account: Id, refused: bool, called_off: bool) {
    let (tx, rx) = std::sync::mpsc::channel();
    let email = a.session.mailbox.account(account).unwrap().email.clone();
    let errors = if refused { vec!["The server refused the sign-in".to_string()] } else { Vec::new() };
    tx.send(mailcraft_model::sync::SyncResult { account, email, errors, ..Default::default() }).unwrap();
    a.sync_run = Some((account, None, called_off, true));
    a.sync_rx = Some(crate::MailRun::Imap { account, opening: a.session.openings(), rx });
    a.poll_sync();
    assert!(a.sync_rx.is_none(), "the run didn't end");
}

/// A run after a renewal that is called off to go on at once is the same Send/Receive: it keeps
/// the account's one retry, so a refusal in the run that goes on renews no second time. The run
/// that goes on ends it, and a Send/Receive called off before it went on leaves none behind
/// (#1844 review).
#[test]
fn a_run_called_off_to_go_on_keeps_its_one_retry() {
    let store = Arc::new(Store::default());
    let mut a = app(&store, Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0));
    let id = own_account(&mut a);
    a.tokens.insert(id, mailcraft_model::sync::Tokens { refresh: "not-a-real-refresh-token".into(), ..Default::default() });
    a.token_retried.insert(id);
    end_run(&mut a, id, true, true);
    assert!(a.token_retried.contains(&id), "the retry was let go of while the run goes on");
    assert_eq!(a.session.driver.round.queue.front().copied(), Some((id, true)), "the run doesn't go on");
    assert!(a.signing_in.is_none(), "renewed a second time in the same Send/Receive");
    // The run that goes on is refused again: no renewal, and the mark ends with it.
    a.session.driver.round.queue.clear();
    end_run(&mut a, id, true, false);
    assert!(!a.token_retried.contains(&id));
    assert!(a.signing_in.is_none(), "renewed a second time in the same Send/Receive");
    // Called off to go on, then the Send/Receive is stopped while it waits its turn behind
    // another account's run: the next one may renew again.
    a.token_retried.insert(id);
    end_run(&mut a, id, false, true);
    assert!(a.token_retried.contains(&id));
    let other = a.session.mailbox.accounts.iter().map(|x| x.id).find(|x| *x != id).unwrap();
    let (_tx, rx) = std::sync::mpsc::channel();
    a.sync_run = Some((other, None, false, true));
    a.sync_rx = Some(crate::MailRun::Imap { account: other, opening: a.session.openings(), rx });
    a.execute("ui.cancelSync", json!({})).unwrap();
    assert!(!a.token_retried.contains(&id), "a run that never went on kept its mark");
}
