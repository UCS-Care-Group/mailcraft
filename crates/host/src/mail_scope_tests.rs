// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A session token that doesn't cover mail is never handed to a mail job (MB3.17): its
//! account's mail is renewed first, with the refresh token it brought.
//! Nothing here leaves this machine.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::Mutex;

use crate::fetch::Access;
use crate::secrets::{SecretError, SecretStore};
use crate::sync::{Adopted, Context, SendReceive};
use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::ClientIds;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{AccountKind, Mailbox, OAuth, OAuthProvider};

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
fn a_held_token_that_doesnt_cover_mail_is_renewed_not_handed_to_a_mail_job() {
    let (s, account) = session();
    let store = Store::default();
    let mut sr = SendReceive::new().graph_only();
    let calendar_only =
        Tokens { access: "calendar-access".into(), refresh: "calendar-refresh".into(), expires_at: 1_000 + 3_600, ..Default::default() };
    assert_eq!(sr.adopt_tokens_covering(&s, &ctx(&store, 1_000), account, calendar_only, false), Adopted::Kept);
    match sr.mail_access(&s, &ctx(&store, 1_000), account) {
        Some(Access::Renew { refresh, .. }) => assert_eq!(refresh, "calendar-refresh"),
        other => panic!("expected a renewal, got {other:?}"),
    }
    // A mail token then replaces it and is held; a later calendar-only one takes that back.
    let mail = Tokens { access: "mail-access".into(), refresh: "mail-refresh".into(), expires_at: 1_000 + 3_600, ..Default::default() };
    assert_eq!(sr.adopt_tokens(&s, &ctx(&store, 1_000), account, mail), Adopted::Kept);
    assert!(matches!(sr.mail_access(&s, &ctx(&store, 1_000), account), Some(Access::Held(t)) if t == "mail-access"));
    let later = Tokens { access: "later-access".into(), refresh: "later-refresh".into(), expires_at: 1_000 + 3_600, ..Default::default() };
    assert_eq!(sr.adopt_tokens_covering(&s, &ctx(&store, 1_000), account, later, false), Adopted::Kept);
    assert!(matches!(sr.mail_access(&s, &ctx(&store, 1_000), account), Some(Access::Renew { .. })));
}
