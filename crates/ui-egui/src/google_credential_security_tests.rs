// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Google's OAuth credentials never enter MailCraft files or diagnostics. All credentials and
//! messages in this audit are synthetic, on fake accounts with .example/example.org addresses.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, Tokens};
use serde_json::json;

use super::*;

#[derive(Clone, Default)]
struct Store(Arc<Mutex<HashMap<String, String>>>);

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.to_string(), value.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

fn app(store: &Store) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(1_791_993_600, 0), 1_791_993_600, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    app.services.secrets = Some(Box::new(store.clone()));
    app
}

fn add_google(app: &mut MailApp, email: &str) -> Id {
    app.execute(
        "account.add",
        json!({
            "email":email,"auth":"oauth2","provider":"google",
            "incoming":{"host":"imap.gmail.com","port":993,"security":"tls"},
            "outgoing":{"host":"smtp.gmail.com","port":465,"security":"tls"}
        }),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap()
}

fn tokens(email: &str, name: &str) -> Tokens {
    Tokens {
        email: email.into(),
        access: format!("audit-access-{name}-unique"),
        refresh: format!("audit-refresh-{name}-unique"),
        expires_at: mailcraft_time::now() + 3600,
        ..Default::default()
    }
}

#[test]
fn google_security_tokens_never_enter_mailbox_ui_files_diagnostics_or_debug() {
    let store = Store::default();
    let mut a = app(&store);
    let id = add_google(&mut a, "one@example.org");
    let owner = a.session.mailbox.account(id).unwrap().clone();
    let given = tokens("one@example.org", "only");
    a.adopt_tokens(id, given.clone());
    let request =
        SyncRequest { account: owner, password: "audit-password-never-written".into(), access_token: given.access.clone(), ..Default::default() };
    let diagnostics = a.execute("ui.copyDiagnostics", json!({})).unwrap();
    let outputs = [
        serde_json::to_string(&a.session.mailbox).unwrap(),
        serde_json::to_string(&a.ui).unwrap(),
        serde_json::to_string(&request).unwrap(),
        format!("{given:?}"),
        format!("{request:?}"),
        diagnostics["text"].as_str().unwrap().to_string(),
    ];
    for text in &outputs {
        for private in [&given.access, &given.refresh, &request.password] {
            assert!(!text.contains(private), "a secret appeared in an output");
        }
    }
    let key = MailApp::secret_key("refresh", a.session.mailbox.account(id).unwrap());
    assert_eq!(store.get(&key).as_deref(), Some(given.refresh.as_str()));

    // Save a synthetic sample mailbox to a test-only file and inspect the actual persisted bytes.
    let folder = std::env::temp_dir().join(format!("mailcraft-google-oauth-security-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("synthetic.mailcraft");
    mailcraft_engine::store::save(&path, &a.session.mailbox).unwrap();
    let on_disk = std::fs::read(&path).unwrap();
    for private in [&given.access, &given.refresh, &request.password] {
        assert!(!on_disk.windows(private.len()).any(|part| part == private.as_bytes()), "mailbox file contained an owner credential");
    }
    std::fs::remove_dir_all(&folder).unwrap();
}

#[test]
fn google_security_only_passwords_and_refresh_tokens_may_reach_the_store() {
    let store = Store::default();
    let mut a = app(&store);
    let id = add_google(&mut a, "one@example.org");
    a.adopt_tokens(id, tokens("one@example.org", "remembered"));
    let access = a.tokens.get(&id).unwrap().access.clone();
    assert!(store.0.lock().unwrap().values().all(|v| v != &access));

    // Even if a future caller passes the wrong kind, the storage boundary fails closed.
    a.remember("access", "one@example.org", "do-not-persist-an-access-token");
    a.remember("bearer", "one@example.org", "also-not-allowed");
    assert!(a.remembered("access", "one@example.org").is_none());
    assert!(store.0.lock().unwrap().keys().all(|key| key.starts_with("refresh:")));
    assert!(!store.0.lock().unwrap().values().any(|value| value.contains("do-not-persist")));

    a.execute("ui.rememberSignIn", json!({"value": false})).unwrap();
    assert!(store.0.lock().unwrap().is_empty());
    a.adopt_tokens(id, tokens("one@example.org", "memory-only"));
    assert!(store.0.lock().unwrap().is_empty());
    assert_eq!(a.tokens.get(&id).map(|t| t.refresh.as_str()), Some("audit-refresh-memory-only-unique"));
    a.execute("ui.rememberSignIn", json!({"value": true})).unwrap();
    assert!(store.0.lock().unwrap().is_empty(), "re-enabling Remember cannot write old in-memory credentials");
    a.adopt_tokens(id, tokens("one@example.org", "new"));
    assert_eq!(store.0.lock().unwrap().len(), 1);
    assert!(store.0.lock().unwrap().values().all(|v| v == "audit-refresh-new-unique"));
}

#[test]
fn google_security_orphan_adoption_and_missing_google_identity_never_create_credentials() {
    let store = Store::default();
    let mut a = app(&store);
    let first = add_google(&mut a, "one@example.org");
    let second = add_google(&mut a, "two@example.org");
    a.adopt_tokens(second, tokens("two@example.org", "unrelated"));
    let second_key = MailApp::secret_key("refresh", a.session.mailbox.account(second).unwrap());

    // A Google authorization which does not name the signed-in account is not accepted
    // during account setup; renewal grants are handled separately by poll_sign_in.
    a.adopt_tokens(first, Tokens { email: String::new(), ..tokens("one@example.org", "unknown") });
    assert!(!a.tokens.contains_key(&first));
    a.execute("account.remove", json!({"id":first})).unwrap();
    a.adopt_tokens(first, tokens("one@example.org", "orphan"));
    assert!(!a.tokens.contains_key(&first));
    assert!(!store.0.lock().unwrap().values().any(|v| v.contains("orphan") || v.contains("unknown")));
    assert_eq!(store.get(&second_key).as_deref(), Some("audit-refresh-unrelated-unique"));
}

#[test]
fn google_security_an_account_edit_cancels_a_pending_refresh_before_it_can_be_reused() {
    let store = Store::default();
    let mut a = app(&store);
    let first = add_google(&mut a, "one@example.org");
    let second = add_google(&mut a, "two@example.org");
    a.adopt_tokens(first, tokens("one@example.org", "old-one"));
    a.adopt_tokens(second, tokens("two@example.org", "old-two"));
    let first_key = MailApp::secret_key("refresh", a.session.mailbox.account(first).unwrap());
    let second_key = MailApp::secret_key("refresh", a.session.mailbox.account(second).unwrap());
    let flag: Arc<Mutex<Option<Arc<AtomicBool>>>> = Default::default();
    let sender: Arc<Mutex<Option<mpsc::Sender<Result<Tokens, String>>>>> = Default::default();
    let flags = flag.clone();
    let pending = sender.clone();
    a.services.sign_in = Some(Box::new(move |req| {
        *flags.lock().unwrap() = Some(req.cancel.clone());
        let (tx, rx) = mpsc::channel();
        *pending.lock().unwrap() = Some(tx);
        rx
    }));
    let oauth = a.session.mailbox.account(first).unwrap().oauth.clone().unwrap();
    a.begin_sign_in(first, oauth, "one@example.org", "audit-refresh-old-one-unique".into(), false, false).unwrap();
    assert!(a.signing_in());
    a.execute("account.edit", json!({"id":first,"incoming":{"host":"imap.changed.example","port":993,"security":"tls"}})).unwrap();
    assert!(!a.signing_in());
    assert!(flag.lock().unwrap().as_ref().unwrap().load(Ordering::Relaxed));
    assert!(sender.lock().unwrap().take().unwrap().send(Ok(tokens("one@example.org", "late"))).is_err());
    a.poll_sign_in();
    assert!(!a.tokens.contains_key(&first));
    assert!(store.get(&first_key).is_none());
    assert_eq!(store.get(&second_key).as_deref(), Some("audit-refresh-old-two-unique"));
}

#[test]
fn google_security_direct_engine_edit_cannot_accept_stale_async_tokens() {
    let store = Store::default();
    let mut a = app(&store);
    let first = add_google(&mut a, "one@example.org");
    let sender: Arc<Mutex<Option<mpsc::Sender<Result<Tokens, String>>>>> = Default::default();
    let pending = sender.clone();
    a.services.sign_in = Some(Box::new(move |_req| {
        let (tx, rx) = mpsc::channel();
        *pending.lock().unwrap() = Some(tx);
        rx
    }));
    let oauth = a.session.mailbox.account(first).unwrap().oauth.clone().unwrap();
    a.begin_sign_in(first, oauth, "one@example.org", String::new(), false, false).unwrap();
    // This bypasses MailApp::execute, as if a hostile mailbox/controller changed the account
    // while the host was still working. poll_sign_in must check the original account snapshot.
    a.session.run("account.edit", &json!({"id":first,"outgoing":{"host":"smtp.changed.example","port":465,"security":"tls"}})).unwrap();
    sender.lock().unwrap().take().unwrap().send(Ok(tokens("one@example.org", "stale"))).unwrap();
    a.poll_sign_in();
    assert!(!a.tokens.contains_key(&first));
    assert!(!store.0.lock().unwrap().values().any(|v| v.contains("stale")));
}

#[test]
fn google_security_revoked_refresh_deletes_only_its_own_key_not_another_account() {
    let store = Store::default();
    let mut a = app(&store);
    let (first, second) = (add_google(&mut a, "one@example.org"), add_google(&mut a, "two@example.org"));
    a.adopt_tokens(first, tokens("one@example.org", "revoked"));
    a.adopt_tokens(second, tokens("two@example.org", "still-valid"));
    let first_owner = a.session.mailbox.account(first).unwrap().clone();
    let key_one = MailApp::secret_key("refresh", &first_owner);
    let key_two = MailApp::secret_key("refresh", a.session.mailbox.account(second).unwrap());
    store.set(&MailApp::legacy_secret_key("refresh", &first_owner.email), "old-legacy-refresh").unwrap();
    a.services.sign_in = Some(Box::new(|_req| {
        let (tx, rx) = mpsc::channel();
        tx.send(Err(crate::approval::GOOGLE_REAUTH_REQUIRED.into())).unwrap();
        rx
    }));
    let oauth = first_owner.oauth.unwrap();
    a.begin_sign_in(first, oauth, "one@example.org", "audit-refresh-revoked-unique".into(), true, false).unwrap();
    a.poll_sign_in();
    assert!(!a.tokens.contains_key(&first));
    assert!(store.get(&key_one).is_none());
    assert!(store.get("refresh:one@example.org").is_none());
    assert_eq!(store.get(&key_two).as_deref(), Some("audit-refresh-still-valid-unique"));
    assert_eq!(a.tokens.get(&second).map(|t| t.refresh.as_str()), Some("audit-refresh-still-valid-unique"));

    let diagnostic = a.execute("ui.copyDiagnostics", json!({})).unwrap();
    let text = diagnostic["text"].as_str().unwrap();
    for private in ["audit-access-revoked-unique", "audit-refresh-revoked-unique", "audit-refresh-still-valid-unique", "old-legacy-refresh"] {
        assert!(!text.contains(private));
    }
}

#[test]
fn google_security_network_failure_keeps_refresh_grant_but_discards_spent_access() {
    let store = Store::default();
    let mut a = app(&store);
    let first = add_google(&mut a, "one@example.org");
    a.adopt_tokens(first, tokens("one@example.org", "retain"));
    let owner = a.session.mailbox.account(first).unwrap().clone();
    let key = MailApp::secret_key("refresh", &owner);
    a.services.sign_in = Some(Box::new(|_req| {
        let (tx, rx) = mpsc::channel();
        tx.send(Err("Google's server was temporarily unreachable".into())).unwrap();
        rx
    }));
    a.begin_sign_in(first, owner.oauth.unwrap(), "one@example.org", "audit-refresh-retain-unique".into(), true, false).unwrap();
    a.poll_sign_in();
    let kept = a.tokens.get(&first).unwrap();
    assert!(kept.access.is_empty());
    assert_eq!(kept.refresh, "audit-refresh-retain-unique");
    assert_eq!(store.get(&key).as_deref(), Some("audit-refresh-retain-unique"));
}

struct RejectStore;
impl SecretStore for RejectStore {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn set(&self, _: &str, value: &str) -> Result<(), String> {
        // OS keychains can echo an attempted credential in their error message. No code
        // should copy the store's text into a status, log or diagnostics.
        Err(format!("the system rejected the secret: {value}"))
    }
    fn delete(&self, _: &str) {}
}

#[test]
fn google_security_store_failure_keeps_tokens_only_in_memory_without_diagnostic_leak() {
    let store = Store::default();
    let mut a = app(&store);
    let id = add_google(&mut a, "one@example.org");
    a.services.secrets = Some(Box::new(RejectStore));
    let given = tokens("one@example.org", "rejected-secret");
    a.adopt_tokens(id, given.clone());
    assert_eq!(a.tokens.get(&id).map(|t| t.access.as_str()), Some(given.access.as_str()));
    assert_eq!(a.tokens.get(&id).map(|t| t.refresh.as_str()), Some(given.refresh.as_str()));
    let report = a.execute("ui.copyDiagnostics", json!({})).unwrap();
    for output in
        [report["text"].as_str().unwrap().to_string(), serde_json::to_string(&a.ui).unwrap(), serde_json::to_string(&a.session.mailbox).unwrap()]
    {
        assert!(!output.contains(&given.access));
        assert!(!output.contains(&given.refresh));
    }
}
