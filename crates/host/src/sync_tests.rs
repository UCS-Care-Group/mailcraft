// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A whole Send/Receive round of a host with no window, against a runner that asks no server.
//! Nothing here leaves this machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::ClientIds;
use mailcraft_model::sync::{GraphMailResult, Tokens};
use mailcraft_model::{AccountKind, Mailbox, OAuth, OAuthProvider};

use crate::graph::{GraphRequest, GraphResult};
use crate::secrets::{SecretError, SecretStore, secret_key};
use crate::sync::{Context, Left, SendReceive, Step, Taken};
use crate::{Host, Platform, init};

struct Quiet;
impl Platform for Quiet {}

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

fn host() -> Host {
    init(Arc::new(Quiet)).unwrap()
}

fn session(kind: AccountKind) -> (Session, u64) {
    let mut mb = Mailbox::with_local_account("Me", "me@example.org");
    mb.accounts[0].kind = kind;
    mb.accounts[0].oauth = Some(OAuth { provider: OAuthProvider::Microsoft, ..Default::default() });
    let id = mb.accounts[0].id;
    (Session::new(mb), id)
}

fn ctx<'a>(store: &'a Store, now_ms: f64) -> Context<'a> {
    Context {
        secrets: Some(store),
        remember: true,
        ids: ClientIds { microsoft_built_in: "app-id", ..Default::default() },
        now_ms,
        now_s: 1_000,
        minutes: 5.0,
        persist: None,
    }
}

/// Take in until the run is over (a second at most).
fn until_over(sr: &mut SendReceive, s: &mut Session, c: &Context) -> Taken {
    for _ in 0..500 {
        match sr.take_result(s, c) {
            Taken::Piece | Taken::Pending => std::thread::sleep(Duration::from_millis(2)),
            other => return other,
        }
    }
    panic!("the run never ended");
}

#[test]
fn a_round_that_is_due_runs_the_account_with_the_stored_sign_in_and_keeps_the_newest_one() {
    let (mut s, id) = session(AccountKind::Microsoft);
    let store = Store::default();
    let key = secret_key("refresh", s.mailbox.account(id).unwrap());
    store.set(&key, "old-refresh").unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let mut sr = SendReceive::with_runner(move |req: GraphRequest| {
        seen2.lock().unwrap().push((req.refresh.clone(), req.oauth.client_id.clone(), req.access.is_empty()));
        let pieces = req.mail_pieces.clone().unwrap();
        let _ = pieces.send(GraphMailResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() });
        GraphResult {
            account: req.account.id,
            tokens: Some(Tokens { refresh: "new-refresh".into(), expires_at: 9_999, ..Default::default() }),
            ..Default::default()
        }
    });
    let h = host();
    let c = ctx(&store, 10_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { account } if account == id));
    assert!(sr.busy());
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Waiting), "one run at a time");
    assert!(matches!(until_over(&mut sr, &mut s, &c), Taken::Done { account, .. } if account == id));
    assert!(!sr.busy());
    assert_eq!(*seen.lock().unwrap(), [("old-refresh".to_string(), "app-id".to_string(), true)]);
    assert_eq!(store.get(&key).as_deref(), Some("new-refresh"), "the newest refresh token is the account's");
    // The round is over and the next is not due for the owner's minutes.
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Idle));
}

#[test]
fn a_run_nobody_asked_for_has_no_page_put_in_front_of_the_owner_and_one_asked_for_has() {
    let (mut s, id) = session(AccountKind::Microsoft);
    let store = Store::default();
    let mut sr = SendReceive::with_runner(|req| GraphResult { account: req.account.id, ..Default::default() });
    let h = host();
    let c = ctx(&store, 10_000.0);
    // (Nothing to run with, so a round by itself doesn't even start it.)
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::LeftOut { why: Left::NoSignIn, .. }));
    sr.start_round(&mut s, &[id]);
    match sr.what_to_run(&h, &mut s, &c) {
        Step::SignIn { account, oauth, address } => {
            assert_eq!((account, address.as_str(), oauth.client_id.as_str()), (id, "me@example.org", "app-id"));
        }
        other => panic!("{other:?}"),
    }
    assert!(!sr.busy());
}

#[test]
fn another_persons_sign_in_ends_the_run_and_is_never_kept() {
    let (mut s, id) = session(AccountKind::Microsoft);
    let store = Store::default();
    let key = secret_key("refresh", s.mailbox.account(id).unwrap());
    store.set(&key, "mine").unwrap();
    let mut sr = SendReceive::with_runner(|req| GraphResult {
        account: req.account.id,
        tokens: Some(Tokens { refresh: "theirs".into(), email: "you@example.org".into(), ..Default::default() }),
        ..Default::default()
    });
    let h = host();
    let c = ctx(&store, 10_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { .. }));
    assert_eq!(until_over(&mut sr, &mut s, &c), Taken::WrongPerson { account: id });
    assert_eq!(store.get(&key).as_deref(), Some("mine"));
}

#[test]
fn a_refused_sign_in_asks_the_owner_again_and_an_account_of_another_kind_is_the_front_ends() {
    let (mut s, id) = session(AccountKind::Microsoft);
    let store = Store::default();
    store.set(&secret_key("refresh", s.mailbox.account(id).unwrap()), "r").unwrap();
    let mut sr = SendReceive::with_runner(|req| GraphResult { account: req.account.id, sign_in_refused: true, ..Default::default() });
    let h = host();
    let c = ctx(&store, 10_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { .. }));
    assert_eq!(until_over(&mut sr, &mut s, &c), Taken::SignInAgain { account: id, consent: false });

    let (mut imap, _) = session(AccountKind::Imap);
    let mut sr = SendReceive::new();
    assert!(matches!(sr.what_to_run(&h, &mut imap, &c), Step::Unsupported { kind: AccountKind::Imap, .. }));
}

#[test]
fn without_auto_sync_nothing_starts_by_itself_and_a_new_mailbox_forgets_the_old_sign_ins() {
    let (mut s, _) = session(AccountKind::Microsoft);
    let store = Store::default();
    s.driver.set_auto_sync(false);
    let mut sr = SendReceive::new();
    let h = host();
    assert!(matches!(sr.what_to_run(&h, &mut s, &ctx(&store, 10_000.0)), Step::Idle));
    assert_eq!(format!("{sr:?}"), "SendReceive { running: None, .. }");
}
