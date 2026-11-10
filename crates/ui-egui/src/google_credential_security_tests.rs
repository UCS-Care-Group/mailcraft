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
    app.services.secrets = Some(Arc::new(store.clone()));
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

fn pending_sign_in(a: &mut MailApp, id: Id, refresh: String) -> (mpsc::Sender<Result<Tokens, String>>, Arc<AtomicBool>) {
    let (tx, rx) = mpsc::channel();
    let rx = Mutex::new(Some(rx));
    a.services.sign_in = Some(Box::new(move |_| rx.lock().unwrap().take().unwrap()));
    let account = a.session.mailbox.account(id).unwrap().clone();
    a.begin_sign_in(id, account.oauth.unwrap(), &account.email, refresh, false, false).unwrap();
    (tx, a.signing_in.as_ref().unwrap().cancel.clone())
}

fn record_sync(a: &mut MailApp) -> Arc<Mutex<Vec<SyncRequest>>> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    a.services.sync = Some(Box::new(move |request| {
        recorded.lock().unwrap().push(request);
        mpsc::channel().1
    }));
    requests
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
    a.remember("access", a.account_at("one@example.org").map_or(0, |x| x.id), "do-not-persist-an-access-token");
    a.remember("bearer", a.account_at("one@example.org").map_or(0, |x| x.id), "also-not-allowed");
    assert!(a.remembered("access", a.account_at("one@example.org").map_or(0, |x| x.id)).is_none());
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
    assert!(a.session.sync_status.is_empty());
    assert!(flag.lock().unwrap().as_ref().unwrap().load(Ordering::Relaxed));
    assert!(sender.lock().unwrap().take().unwrap().send(Ok(tokens("one@example.org", "late"))).is_err());
    a.poll_sign_in();
    assert!(!a.tokens.contains_key(&first));
    assert!(store.get(&first_key).is_none());
    assert_eq!(store.get(&second_key).as_deref(), Some("audit-refresh-old-two-unique"));
}

#[test]
fn google_security_login_and_security_edits_cancel_without_erasing_kept_credentials() {
    for (name, mut change) in [
        ("login", json!({"username":"alias@example.org"})),
        ("incoming security", json!({"incoming":{"host":"imap.gmail.com","port":993,"security":"starttls"}})),
        ("outgoing security", json!({"outgoing":{"host":"smtp.gmail.com","port":465,"security":"starttls"}})),
    ] {
        let store = Store::default();
        let mut a = app(&store);
        let id = add_google(&mut a, "one@example.org");
        let kept = tokens("one@example.org", "kept-during-edit");
        a.adopt_tokens(id, kept.clone());
        let before = a.session.mailbox.account(id).unwrap().clone();
        let key = MailApp::secret_key("refresh", &before);
        let requests = record_sync(&mut a);
        let (tx, cancel) = pending_sign_in(&mut a, id, String::new());
        assert!(!a.session.sync_status.is_empty());

        change["id"] = json!(id);
        a.execute("account.edit", change).unwrap();

        // Keep host and port intact: these edits must not exercise the existing key cleanup.
        let after = a.session.mailbox.account(id).unwrap();
        assert_eq!(MailApp::secret_key("refresh", after), key, "{name}");
        assert_eq!(after.oauth, before.oauth, "{name}");
        assert!(!a.signing_in(), "{name}");
        assert!(cancel.load(Ordering::Relaxed), "{name}");
        assert!(a.session.sync_status.is_empty(), "{name}");
        assert!(tx.send(Ok(tokens("one@example.org", "late-edit"))).is_err(), "{name}");
        a.poll_sign_in();

        let current = a.tokens.get(&id).unwrap();
        assert_eq!(current.access, kept.access, "{name}");
        assert_eq!(current.refresh, kept.refresh, "{name}");
        assert_eq!(store.get(&key).as_deref(), Some(kept.refresh.as_str()), "{name}");
        assert_eq!(a.secret_cache.get(&key), Some(&kept.refresh), "{name}");
        assert!(requests.lock().unwrap().is_empty(), "{name}");
    }
}

#[test]
fn google_security_removal_cancels_pending_sign_in_and_clears_waiting_status() {
    let store = Store::default();
    let mut a = app(&store);
    let first = add_google(&mut a, "one@example.org");
    let second = add_google(&mut a, "two@example.org");
    let kept = tokens("one@example.org", "before-removal");
    let other = tokens("two@example.org", "unrelated-removal");
    a.adopt_tokens(first, kept.clone());
    a.adopt_tokens(second, other.clone());
    let first_key = MailApp::secret_key("refresh", a.session.mailbox.account(first).unwrap());
    let second_key = MailApp::secret_key("refresh", a.session.mailbox.account(second).unwrap());
    let requests = record_sync(&mut a);
    let (tx, cancel) = pending_sign_in(&mut a, first, kept.refresh);
    assert!(!a.session.sync_status.is_empty());
    // An answer already queued must disappear with the account too.
    tx.send(Ok(tokens("", "queued-removal"))).unwrap();

    a.execute("account.remove", json!({"id":first})).unwrap();

    assert!(!a.signing_in());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(a.session.sync_status.is_empty());
    assert!(tx.send(Ok(tokens("", "late-removal"))).is_err());
    a.poll_sign_in();
    assert!(!a.tokens.contains_key(&first));
    assert!(!a.graph_tokens.contains_key(&first));
    assert!(store.get(&first_key).is_none());
    assert_eq!(store.get(&second_key).as_deref(), Some(other.refresh.as_str()));
    assert_eq!(a.tokens.get(&second).map(|t| t.access.as_str()), Some(other.access.as_str()));
    assert!(requests.lock().unwrap().is_empty());
}

#[test]
fn google_security_other_account_edit_and_removal_leave_pending_sign_in_untouched() {
    let store = Store::default();
    let mut a = app(&store);
    let first = add_google(&mut a, "one@example.org");
    let second = add_google(&mut a, "two@example.org");
    let kept = tokens("one@example.org", "other-account-kept");
    a.adopt_tokens(first, kept.clone());
    a.adopt_tokens(second, tokens("two@example.org", "other-account-removed"));
    let key = MailApp::secret_key("refresh", a.session.mailbox.account(first).unwrap());
    let requests = record_sync(&mut a);
    let (tx, cancel) = pending_sign_in(&mut a, first, String::new());
    let waiting = a.session.sync_status.clone();
    assert!(!waiting.is_empty());

    for (command, params) in [("account.edit", json!({"id":second,"username":"alias@example.org"})), ("account.remove", json!({"id":second}))] {
        a.execute(command, params).unwrap();
        assert!(a.signing_in(), "{command}");
        assert!(!cancel.load(Ordering::Relaxed), "{command}");
        assert_eq!(a.session.sync_status, waiting, "{command}");
        assert_eq!(a.tokens.get(&first).map(|t| t.access.as_str()), Some(kept.access.as_str()), "{command}");
        assert_eq!(store.get(&key).as_deref(), Some(kept.refresh.as_str()), "{command}");
    }

    let given = tokens("one@example.org", "unaffected-completion");
    tx.send(Ok(given.clone())).unwrap();
    a.poll_sign_in();
    assert_eq!(a.tokens.get(&first).map(|t| t.access.as_str()), Some(given.access.as_str()));
    assert_eq!(store.get(&key).as_deref(), Some(given.refresh.as_str()));
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].account.id, first);
    assert_eq!(requests[0].access_token, given.access);
}

#[test]
fn google_security_unrelated_account_edits_accept_pending_sign_in_and_sync() {
    for direct in [false, true] {
        let store = Store::default();
        let mut a = app(&store);
        a.ui.remember_sign_in = true;
        let id = add_google(&mut a, "one@example.org");
        let requests = record_sync(&mut a);
        let (tx, cancel) = pending_sign_in(&mut a, id, String::new());
        let waiting = a.session.sync_status.clone();
        assert!(!waiting.is_empty());
        let changes = json!({
            "id":id,"name":"Personal mail","displayName":"Avery",
            "downloadInFull":"everything","keepOffline":"1m","askBeforeDeletingConversation":false
        });
        if direct {
            a.session.run("account.edit", &changes).unwrap();
        } else {
            a.execute("account.edit", changes).unwrap();
        }
        // Signatures are also preferences; a controller may edit them while the host waits.
        let account = a.session.mailbox.account_mut(id).unwrap();
        account.signature = "Personal signature".into();
        account.reply_signature = Some("Reply signature".into());
        let edited = account.clone();
        assert!(a.signing_in());
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(a.session.sync_status, waiting);

        let given = tokens("one@example.org", "after-preferences");
        tx.send(Ok(given.clone())).unwrap();
        a.poll_sign_in();

        assert!(!a.signing_in());
        assert_eq!(a.tokens.get(&id).map(|t| t.access.as_str()), Some(given.access.as_str()), "direct engine edit: {direct}");
        assert_eq!(store.get(&MailApp::secret_key("refresh", &edited)).as_deref(), Some(given.refresh.as_str()));
        assert_eq!(a.session.mailbox.account(id), Some(&edited));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "the accepted answer should resume Send/Receive");
        assert_eq!(requests[0].account, edited, "Send/Receive must use the updated preferences");
        assert_eq!(requests[0].access_token, given.access);
        assert!(requests[0].password.is_empty());
    }
}

#[test]
fn google_security_credential_edits_reject_identityless_pending_refresh() {
    use mailcraft_model::{Account, AccountKind, OAuthProvider, Security};

    let edits: &[(&str, fn(&mut Account))] = &[
        ("address", |a| a.email = "other@example.org".into()),
        ("login", |a| a.username = "alias@example.org".into()),
        ("auth method", |a| a.oauth = None),
        ("provider", |a| a.oauth.as_mut().unwrap().provider = OAuthProvider::Microsoft),
        ("client", |a| a.oauth.as_mut().unwrap().client_id = "other-client".into()),
        ("tenant", |a| a.oauth.as_mut().unwrap().tenant = "other-tenant".into()),
        ("incoming host", |a| a.incoming.as_mut().unwrap().host = "imap.changed.example".into()),
        ("outgoing host", |a| a.outgoing.as_mut().unwrap().host = "smtp.changed.example".into()),
        ("incoming port", |a| a.incoming.as_mut().unwrap().port = 1993),
        ("outgoing port", |a| a.outgoing.as_mut().unwrap().port = 1465),
        ("incoming security", |a| a.incoming.as_mut().unwrap().security = Security::None),
        ("outgoing security", |a| a.outgoing.as_mut().unwrap().security = Security::StartTls),
        ("account kind", |a| a.kind = AccountKind::Local),
        ("shared owner", |a| a.shared_of = Some(1)),
        ("account id", |a| a.id = 999_999),
    ];
    for (name, edit) in edits {
        let store = Store::default();
        let mut a = app(&store);
        let id = add_google(&mut a, "one@example.org");
        let requests = record_sync(&mut a);
        let (tx, _) = pending_sign_in(&mut a, id, "audit-refresh-before-edit".into());
        // Bypass the UI cancellation hook, as another controller or mailbox edit can do.
        edit(a.session.mailbox.account_mut(id).unwrap());
        // Renewal may omit the email: rejection must come from the saved credential identity,
        // not signed_in_as_other's independent check of a provider-supplied email.
        tx.send(Ok(tokens("", "stale-identity"))).unwrap();
        a.poll_sign_in();
        assert!(!a.signing_in(), "{name}");
        assert!(!a.tokens.contains_key(&id), "{name}");
        assert!(!a.graph_tokens.contains_key(&id), "{name}");
        assert!(!store.0.lock().unwrap().values().any(|value| value.contains("stale-identity")), "{name}");
        assert!(requests.lock().unwrap().is_empty(), "{name}");
    }
}

#[test]
fn google_security_removed_or_reassigned_accounts_reject_pending_refresh() {
    for reuse_id in [false, true] {
        let store = Store::default();
        let mut a = app(&store);
        let id = add_google(&mut a, "one@example.org");
        let requests = record_sync(&mut a);
        let (tx, _) = pending_sign_in(&mut a, id, "audit-refresh-before-removal".into());
        let mut replacement = a.session.mailbox.account(id).unwrap().clone();
        a.session.run("account.remove", &json!({"id":id})).unwrap();
        if reuse_id {
            replacement.email = "replacement@example.org".into();
            replacement.username = replacement.email.clone();
            a.session.mailbox.accounts.push(replacement);
        }
        tx.send(Ok(tokens("", "late-removed"))).unwrap();
        a.poll_sign_in();
        assert!(!a.signing_in());
        assert!(!a.tokens.contains_key(&id), "reassigned id: {reuse_id}");
        assert!(!a.graph_tokens.contains_key(&id));
        assert!(!store.0.lock().unwrap().values().any(|value| value.contains("late-removed")));
        assert!(requests.lock().unwrap().is_empty());
    }
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
    a.services.secrets = Some(Arc::new(RejectStore));
    let given = tokens("one@example.org", "rejected-secret");
    a.adopt_tokens(id, given.clone());
    // Copy Diagnostics replaces this status, so check the store failure before that command.
    if let Some((status, _)) = &a.status_msg {
        assert!(!status.contains(&given.access));
        assert!(!status.contains(&given.refresh));
    }
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

/// Both Microsoft account forms may ask for Graph's calendar and contacts.
fn add_microsoft(a: &mut MailApp, graph_mail: bool) -> Id {
    let mut fields = json!({"email":"microsoft@example.org","auth":"oauth2","provider":"microsoft"});
    if graph_mail {
        fields["kind"] = json!("microsoft");
    } else {
        fields["incoming"] = json!({"host":"outlook.office365.com","port":993,"security":"tls"});
    }
    a.execute("account.add", fields).unwrap()["id"].as_u64().unwrap()
}

/// The host would open the browser here; these tests only record what it was asked.
fn record_sign_in(a: &mut MailApp) -> Arc<Mutex<Vec<SignInRequest>>> {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    a.services.sign_in = Some(Box::new(move |request| {
        recorded.lock().unwrap().push(request);
        mpsc::channel().1
    }));
    requests
}

struct RecordingStore {
    store: Store,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl SecretStore for RecordingStore {
    fn get(&self, key: &str) -> Option<String> {
        self.calls.lock().unwrap().push("get");
        self.store.get(key)
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push("set");
        self.store.set(key, value)
    }
    fn delete(&self, key: &str) {
        self.calls.lock().unwrap().push("delete");
        self.store.delete(key);
    }
    fn take_trouble(&self) -> Option<StoreTrouble> {
        self.calls.lock().unwrap().push("take_trouble");
        self.store.take_trouble()
    }
}

/// Count even reads, so rejection is proved to precede any credential-store access.
fn record_store(a: &mut MailApp, store: &Store) -> Arc<Mutex<Vec<&'static str>>> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    a.services.secrets = Some(Arc::new(RecordingStore { store: store.clone(), calls: calls.clone() }));
    calls
}

/// A Google account before Microsoft must not take Microsoft's calendar/contacts command.
#[test]
fn google_security_microsoft_command_skips_google_by_default() {
    let store = Store::default();
    let mut a = app(&store);
    let google = add_google(&mut a, "google@example.org");
    let microsoft = add_microsoft(&mut a, true);
    assert_eq!(a.session.mailbox.accounts.iter().find(|a| a.oauth.is_some()).map(|a| a.id), Some(google));
    let requests = record_sign_in(&mut a);

    assert_eq!(a.execute("ui.connectMicrosoft", json!({})).unwrap(), json!({"pending":"signIn","account":microsoft}));

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].account, microsoft);
    assert_eq!(requests[0].oauth.provider, mailcraft_model::OAuthProvider::Microsoft);
    assert!(requests[0].graph);
    assert!(requests[0].google_client_secret.is_empty());
}

/// Reject Google's explicit id without asking the host or store, even while Google is signing in.
#[test]
fn google_security_microsoft_command_rejects_explicit_google_before_side_effects() {
    for already_waiting in [false, true] {
        let store = Store::default();
        let mut a = app(&store);
        let google = add_google(&mut a, "google@example.org");
        add_microsoft(&mut a, true);
        let kept = tokens("google@example.org", "microsoft-command-kept");
        a.adopt_tokens(google, kept.clone());
        let owner = a.session.mailbox.account(google).unwrap().clone();
        let stored = store.0.lock().unwrap().clone();
        let cache = a.secret_cache.clone();
        let pending = already_waiting.then(|| pending_sign_in(&mut a, google, String::new()));
        let waiting = a.session.sync_status.clone();
        let requests = record_sign_in(&mut a);
        let store_calls = record_store(&mut a, &store);
        let sync = record_sync(&mut a);

        assert_eq!(a.execute("ui.connectMicrosoft", json!({"account":google})), Err("This account doesn't sign in with Microsoft".to_string()));

        assert!(requests.lock().unwrap().is_empty(), "the host must not open a provider page");
        assert!(store_calls.lock().unwrap().is_empty(), "rejection must precede credential-store access");
        assert!(sync.lock().unwrap().is_empty());
        assert_eq!(a.session.mailbox.account(google), Some(&owner));
        assert_eq!(store.0.lock().unwrap().clone(), stored);
        assert_eq!(a.secret_cache, cache);
        assert_eq!(a.tokens.get(&google).map(|t| t.access.as_str()), Some(kept.access.as_str()));
        assert_eq!(a.tokens.get(&google).map(|t| t.refresh.as_str()), Some(kept.refresh.as_str()));
        assert!(!a.graph_tokens.contains_key(&google));
        assert_eq!(a.signing_in(), already_waiting);
        assert_eq!(a.session.sync_status, waiting);
        if let Some((_, cancel)) = pending {
            assert!(!cancel.load(Ordering::Relaxed));
            assert_eq!(a.signing_in.as_ref().unwrap().account, google);
        }
    }
}

/// With only Google available, the default command cannot start any sign-in.
#[test]
fn google_security_microsoft_command_rejects_google_only_default() {
    let store = Store::default();
    let mut a = app(&store);
    add_google(&mut a, "google@example.org");
    let requests = record_sign_in(&mut a);
    let store_calls = record_store(&mut a, &store);
    let waiting = a.session.sync_status.clone();

    assert_eq!(a.execute("ui.connectMicrosoft", json!({})), Err("No account signs in with Microsoft".to_string()));

    assert!(requests.lock().unwrap().is_empty());
    assert!(store_calls.lock().unwrap().is_empty());
    assert!(!a.signing_in());
    assert_eq!(a.session.sync_status, waiting);
}

/// Microsoft OAuth still works for Graph mail and for an IMAP account asking for calendar/contacts.
#[test]
fn google_security_microsoft_command_accepts_explicit_microsoft_accounts() {
    for graph_mail in [false, true] {
        let store = Store::default();
        let mut a = app(&store);
        add_google(&mut a, "google@example.org");
        let microsoft = add_microsoft(&mut a, graph_mail);
        let requests = record_sign_in(&mut a);

        assert_eq!(a.execute("ui.connectMicrosoft", json!({"account":microsoft})).unwrap(), json!({"pending":"signIn","account":microsoft}));

        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].account, microsoft);
        assert_eq!(requests[0].oauth.provider, mailcraft_model::OAuthProvider::Microsoft);
        assert_eq!(requests[0].email, "microsoft@example.org");
        assert!(requests[0].graph);
        assert_eq!(requests[0].mail_write, graph_mail);
        assert!(!requests[0].quiet);
        assert!(requests[0].refresh.is_empty());
        assert!(requests[0].google_client_secret.is_empty());
        assert!(a.signing_in.as_ref().unwrap().browser);
    }
}
