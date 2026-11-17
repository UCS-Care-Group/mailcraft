// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! How a job of an account's mail gets its Graph token (MB3.17): the session's while it is good,
//! else a renewal from the refresh token, and nothing when there is nothing to sign in with.
//! Nothing here leaves this machine.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::Mutex;

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::ClientIds;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{AccountKind, Mailbox, OAuth, OAuthProvider};
use mailcraft_net::NetError;
use mailcraft_net::oauth::GraphTokens;

use crate::fetch::{Access, with_access_renewing};
use crate::secrets::{SecretError, SecretStore, secret_key};
use crate::sync::{Adopted, Context, SendReceive};

#[derive(Default)]
struct Store(Mutex<HashMap<String, String>>);
impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

fn session() -> (Session, u64) {
    let mut mb = Mailbox::with_local_account("Me", "me@example.org");
    mb.accounts[0].kind = AccountKind::Microsoft;
    mb.accounts[0].oauth = Some(OAuth { provider: OAuthProvider::Microsoft, ..Default::default() });
    let id = mb.accounts[0].id;
    (Session::new(mb), id)
}

fn ctx(store: &Store, now_s: i64) -> Context<'_> {
    Context {
        secrets: Some(store),
        remember: true,
        ids: ClientIds { microsoft_built_in: "app-id", ..Default::default() },
        now_ms: 0.0,
        now_s,
        minutes: 0.0,
        persist: None,
        unconfirmed: false,
        data_removed: false,
        job_busy: None,
    }
}

#[test]
fn a_job_uses_the_sessions_token_while_it_is_good_and_else_renews_from_the_refresh_token() {
    let (s, account) = session();
    let store = Store::default();
    let mut sr = SendReceive::new().graph_only();
    // Nothing to sign in with: no job.
    assert!(sr.mail_access(&s, &ctx(&store, 1_000), account).is_none());

    // A refresh token kept by an earlier sign-in: renewed, with the build's application id.
    store.set(&secret_key("refresh", s.mailbox.account(account).unwrap()), "stored-refresh").unwrap();
    match sr.mail_access(&s, &ctx(&store, 1_000), account) {
        Some(Access::Renew { oauth, refresh }) => {
            assert_eq!(refresh, "stored-refresh");
            assert_eq!(oauth.client_id, "app-id");
        }
        other => panic!("expected a renewal, got {other:?}"),
    }

    // A token of the session that is still good is used as it is, and not once it has run out.
    let tokens = Tokens { access: "session-access".into(), refresh: "newer-refresh".into(), expires_at: 1_000 + 3_600, ..Default::default() };
    assert_eq!(sr.adopt_tokens(&s, &ctx(&store, 1_000), account, tokens), Adopted::Kept);
    assert!(matches!(sr.mail_access(&s, &ctx(&store, 1_000), account), Some(Access::Held(t)) if t == "session-access"));
    match sr.mail_access(&s, &ctx(&store, 1_000 + 3_600), account) {
        Some(Access::Renew { refresh, .. }) => assert_eq!(refresh, "newer-refresh", "the session's newest refresh token"),
        other => panic!("expected a renewal, got {other:?}"),
    }
}

#[test]
fn no_job_gets_a_token_offline_unconfirmed_or_for_an_account_the_phone_doesnt_run() {
    let (mut s, account) = session();
    let store = Store::default();
    store.set(&secret_key("refresh", s.mailbox.account(account).unwrap()), "stored-refresh").unwrap();
    let mut sr = SendReceive::new().graph_only();
    s.prefs.offline = true;
    assert!(sr.mail_access(&s, &ctx(&store, 1_000), account).is_none(), "Work Offline");
    s.prefs.offline = false;
    let held = Context { unconfirmed: true, ..ctx(&store, 1_000) };
    assert!(sr.mail_access(&s, &held, account).is_none(), "an unconfirmed mailbox");
    assert!(sr.mail_access(&s, &ctx(&store, 1_000), account + 99).is_none(), "no such account");
    s.mailbox.accounts[0].kind = AccountKind::Imap;
    assert!(sr.mail_access(&s, &ctx(&store, 1_000), account).is_none(), "not a Microsoft account");
}

#[test]
fn a_renewal_hands_back_its_tokens_and_says_why_none_came() {
    let oauth = OAuth { provider: OAuthProvider::Microsoft, ..Default::default() };
    let renew_ok = |_: &OAuth, refresh: &str| -> Result<GraphTokens, NetError> {
        assert_eq!(refresh, "r1");
        Ok(GraphTokens {
            tokens: Tokens { access: "a2".into(), refresh: "r2".into(), expires_at: 9, ..Default::default() },
            extras_missing: false,
            covers_favorites: false,
        })
    };
    let (answer, renewal) = with_access_renewing(Access::Renew { oauth: oauth.clone(), refresh: "r1".into() }, &renew_ok, |t| t.to_string());
    assert_eq!(answer.as_deref(), Some("a2"));
    assert_eq!(renewal.tokens.as_ref().map(|t| t.refresh.as_str()), Some("r2"));
    // Nothing of a token in the `Debug` output.
    let shown = format!("{renewal:?} {:?}", Access::Held("secret-token".into()));
    assert!(!shown.contains("a2") && !shown.contains("r2") && !shown.contains("secret-token"), "{shown}");

    let consent = |_: &OAuth, _: &str| -> Result<GraphTokens, NetError> { Err(NetError::ConsentNeeded("no".into())) };
    let (answer, renewal) = with_access_renewing(Access::Renew { oauth: oauth.clone(), refresh: "r1".into() }, &consent, |_| ());
    assert!(answer.is_none() && renewal.consent_needed && renewal.tokens.is_none());

    let failing = |_: &OAuth, _: &str| -> Result<GraphTokens, NetError> { Err(NetError::Config("nope".into())) };
    let (answer, renewal) = with_access_renewing(Access::Renew { oauth, refresh: "r1".into() }, &failing, |_| ());
    assert!(answer.is_none() && renewal.error.is_some() && !renewal.sign_in_refused);

    // A token held is used without asking anybody.
    let never = |_: &OAuth, _: &str| -> Result<GraphTokens, NetError> { panic!("not asked") };
    let (answer, renewal) = with_access_renewing(Access::Held("held".into()), &never, |t| t.len());
    assert_eq!(answer, Some(4));
    assert!(renewal.tokens.is_none());
    // An empty token is no token.
    let (answer, _) = with_access_renewing(Access::Held(String::new()), &never, |t| t.len());
    assert!(answer.is_none());
}
