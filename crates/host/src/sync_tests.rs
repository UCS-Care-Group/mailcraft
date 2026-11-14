// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A whole Send/Receive round of a host with no window, against a runner that asks no server.
//! Nothing here leaves this machine.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_engine::sync_driver::ClientIds;
use mailcraft_model::sync::{GraphMailResult, Tokens};
use mailcraft_model::{AccountKind, Mailbox, OAuth, OAuthProvider};

use crate::graph::{GraphRequest, GraphResult};
use crate::secrets::{SecretError, SecretStore, secret_key};
use crate::sync::{Adopted, Context, Left, SendReceive, Step, Taken};
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
        unconfirmed: false,
        data_removed: false,
        job_busy: None,
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

/// Waits (a bounded time) for the run's first piece; fails at once if the run ends first.
fn first_piece(sr: &mut SendReceive, s: &mut Session, c: &Context<'_>) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match sr.take_result(s, c) {
            Taken::Piece => return,
            Taken::Pending => {}
            other => panic!("the run ended before its first piece: done={}", matches!(other, Taken::Done { .. })),
        }
        assert!(std::time::Instant::now() < deadline, "no piece came within the time");
        std::thread::sleep(Duration::from_millis(2));
    }
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
    assert!(sr.start_round(&mut s, &c, &[id]));
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

fn one_with_a_sign_in() -> (Session, u64, Store) {
    let (s, id) = session(AccountKind::Microsoft);
    let store = Store::default();
    store.set(&secret_key("refresh", s.mailbox.account(id).unwrap()), "r").unwrap();
    (s, id, store)
}

#[test]
fn working_offline_starts_nothing_and_refuses_a_round() {
    let (mut s, id, store) = one_with_a_sign_in();
    s.prefs.offline = true;
    let ran = Arc::new(Mutex::new(0));
    let ran2 = ran.clone();
    let mut sr = SendReceive::with_runner(move |req| {
        *ran2.lock().unwrap() += 1;
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let h = host();
    let c = ctx(&store, 10_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Idle), "a round is due, and nothing starts");
    assert!(!sr.start_round(&mut s, &c, &[id]));
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Idle));
    assert_eq!(*ran.lock().unwrap(), 0);
    s.prefs.offline = false;
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { .. }));
}

#[test]
fn an_unconfirmed_mailbox_starts_nothing_reads_and_keeps_no_secret_and_refuses_a_round() {
    let (mut s, id, store) = one_with_a_sign_in();
    let mut sr = SendReceive::with_runner(|req| GraphResult { account: req.account.id, ..Default::default() });
    let h = host();
    let mut c = ctx(&store, 10_000.0);
    c.unconfirmed = true;
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Idle));
    assert!(!sr.start_round(&mut s, &c, &[id]));
    assert!(sr.refresh_for(&s, &c, id).is_none(), "no remembered secret is read");
    let tokens = Tokens { refresh: "new".into(), ..Default::default() };
    assert_eq!(sr.adopt_tokens(&s, &c, id, tokens.clone()), Adopted::Refused);
    // Data removed: nothing is kept again either, though a sign-in may be read.
    c.unconfirmed = false;
    c.data_removed = true;
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Idle));
    assert_eq!(sr.adopt_tokens(&s, &c, id, tokens), Adopted::Refused);
    let key = secret_key("refresh", s.mailbox.account(id).unwrap());
    assert_eq!(store.get(&key).as_deref(), Some("r"));
    assert!(sr.take_written_keys().is_empty());
}

#[test]
fn a_sign_in_handed_in_is_kept_reported_and_refused_for_another_person() {
    let (mut s, id) = session(AccountKind::Microsoft);
    let store = Store::default();
    let mut sr = SendReceive::new();
    let c = ctx(&store, 10_000.0);
    let theirs = Tokens { refresh: "theirs".into(), email: "you@example.org".into(), ..Default::default() };
    assert_eq!(sr.adopt_tokens(&s, &c, id, theirs), Adopted::WrongPerson);
    let key = secret_key("refresh", s.mailbox.account(id).unwrap());
    assert!(store.get(&key).is_none());
    let mine = Tokens { refresh: "mine".into(), email: "me@example.org".into(), ..Default::default() };
    assert_eq!(sr.adopt_tokens(&s, &c, id, mine), Adopted::Kept);
    assert_eq!(store.get(&key).as_deref(), Some("mine"));
    assert_eq!(sr.take_written_keys(), [key]);
    assert!(sr.take_written_keys().is_empty(), "reported once");
    // And a run now starts with it, without the store.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let mut sr2 = SendReceive::with_runner(move |req| {
        seen2.lock().unwrap().push(req.refresh.clone());
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let mut c2 = ctx(&store, 10_000.0);
    c2.secrets = None;
    let _ = sr2.adopt_tokens(&s, &c2, id, Tokens { refresh: "kept".into(), email: "me@example.org".into(), ..Default::default() });
    let step = sr2.what_to_run(&host(), &mut s, &c2);
    assert!(matches!(step, Step::Started { .. }), "{step:?}");
    assert_eq!(until_over(&mut sr2, &mut s, &c2), Taken::Done { account: id, errors: Vec::new() });
    assert_eq!(*seen.lock().unwrap(), ["kept"]);
}

#[test]
fn a_run_let_go_of_is_called_off_and_never_calls_off_or_is_called_off_by_the_next() {
    let (mut s, id, store) = one_with_a_sign_in();
    let flags: Arc<Mutex<Vec<Arc<AtomicBool>>>> = Arc::default();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let flags2 = flags.clone();
    let mut sr = SendReceive::with_runner(move |req: GraphRequest| {
        let first = {
            let mut all = flags2.lock().unwrap();
            all.push(req.cancel.clone());
            all.len() == 1
        };
        if first {
            let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
            // The orphan's failed hand-over calls its own run off.
            req.cancel.store(true, Ordering::Relaxed);
        }
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let h = host();
    let c = ctx(&store, 10_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { .. }));
    while flags.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(2));
    }
    sr.forget();
    assert!(flags.lock().unwrap()[0].load(Ordering::Relaxed), "the run let go of is called off");
    assert!(sr.start_round(&mut s, &c, &[id]));
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { .. }));
    let _ = release.send(());
    while flags.lock().unwrap().len() < 2 {
        std::thread::sleep(Duration::from_millis(2));
    }
    {
        let all = flags.lock().unwrap();
        assert!(all[0].load(Ordering::Relaxed));
        assert!(!all[1].load(Ordering::Relaxed), "the next run starts with a flag of its own");
    }
    assert!(matches!(until_over(&mut sr, &mut s, &c), Taken::Done { .. }));
    assert!(!flags.lock().unwrap()[1].load(Ordering::Relaxed), "and the orphan's flag never reached it");
}

#[test]
fn errors_of_the_pieces_reach_the_end_and_the_answer_sets_the_calendar_consent() {
    let (mut s, id, store) = one_with_a_sign_in();
    let mut sr = SendReceive::with_runner(move |req: GraphRequest| {
        let pieces = req.mail_pieces.clone().unwrap();
        let _ = pieces.send(GraphMailResult {
            account: req.account.id,
            email: req.account.email.clone(),
            errors: vec!["a folder was refused".into()],
            ..Default::default()
        });
        GraphResult { account: req.account.id, consent_needed: true, mail: Some(GraphMailResult::default()), ..Default::default() }
    });
    let h = host();
    let c = ctx(&store, 10_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { .. }));
    // Taken a piece at a time, as a front end does: the error of an earlier call is not lost.
    let mut last = Taken::Nothing;
    for _ in 0..500 {
        last = sr.take_result(&mut s, &c);
        match last {
            Taken::Piece | Taken::Pending => std::thread::sleep(Duration::from_millis(2)),
            _ => break,
        }
    }
    match last {
        Taken::Done { errors, .. } => assert!(errors.iter().any(|e| e.contains("refused")), "{errors:?}"),
        other => panic!("{other:?}"),
    }
    assert!(s.driver.calendar.consent_needed.contains(&id));
}

#[test]
fn a_busy_calendar_job_makes_the_account_wait_and_another_kind_is_the_front_ends_to_run() {
    let (mut s, id, store) = one_with_a_sign_in();
    let mut sr = SendReceive::with_runner(|req| GraphResult { account: req.account.id, ..Default::default() });
    let h = host();
    let busy = AtomicBool::new(true);
    let job = |_: u64| busy.load(Ordering::Relaxed);
    let mut c = ctx(&store, 10_000.0);
    c.job_busy = Some(&job);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Waiting));
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Waiting), "still in line");
    busy.store(false, Ordering::Relaxed);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { account } if account == id));

    // An account of another kind is handed back and not left out: the front end runs it.
    let (mut imap, other) = session(AccountKind::Imap);
    let mut sr = SendReceive::new();
    let c = ctx(&store, 10_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut imap, &c), Step::Unsupported { account, .. } if account == other));
    assert!(imap.sync_progress.accounts.iter().any(|r| r.account == other), "its row is the front end's to settle");
}

#[test]
fn something_made_here_during_a_long_run_calls_it_off_and_it_goes_first_again() {
    use mailcraft_model::{Address, FolderKind, Message};
    let (mut s, id, store) = one_with_a_sign_in();
    let flag: Arc<Mutex<Option<Arc<AtomicBool>>>> = Arc::default();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let flag2 = flag.clone();
    let mut sr = SendReceive::with_runner(move |req: GraphRequest| {
        let pieces = req.mail_pieces.clone().unwrap();
        *flag2.lock().unwrap() = Some(req.cancel.clone());
        let _ = pieces.send(GraphMailResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() });
        let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let h = host();
    // (A look is due: the clock is far past the last one.)
    let c = ctx(&store, 100_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { .. }));
    first_piece(&mut sr, &mut s, &c);
    let called_off = || flag.lock().unwrap().as_ref().is_some_and(|f| f.load(Ordering::Relaxed));
    assert!(!called_off(), "nothing new was made here");
    let outbox = s.mailbox.folder_of_kind(id, FolderKind::Outbox).unwrap();
    let m = Message { account: id, folder: outbox, subject: "Later".into(), to: vec![Address::new("", "kit@example.net")], ..Default::default() };
    s.mailbox.add_message(m).unwrap();
    let later = ctx(&store, 200_000.0);
    assert!(matches!(sr.take_result(&mut s, &later), Taken::Pending | Taken::Piece));
    assert!(called_off());
    let _ = release.send(());
    assert!(matches!(until_over(&mut sr, &mut s, &later), Taken::Done { .. }));
    assert!(s.driver.round.is_queued(id), "it goes on at once, as it was");
}

#[test]
fn an_explicit_stop_after_a_call_off_never_puts_the_run_back() {
    use mailcraft_model::{Address, FolderKind, Message};
    let (mut s, id, store) = one_with_a_sign_in();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let mut sr = SendReceive::with_runner(move |req: GraphRequest| {
        let pieces = req.mail_pieces.clone().unwrap();
        let _ = pieces.send(GraphMailResult { account: req.account.id, email: req.account.email.clone(), ..Default::default() });
        let _ = gate.lock().unwrap().recv_timeout(Duration::from_secs(5));
        GraphResult { account: req.account.id, ..Default::default() }
    });
    let h = host();
    let c = ctx(&store, 100_000.0);
    assert!(matches!(sr.what_to_run(&h, &mut s, &c), Step::Started { .. }));
    first_piece(&mut sr, &mut s, &c);
    let outbox = s.mailbox.folder_of_kind(id, FolderKind::Outbox).unwrap();
    let m = Message { account: id, folder: outbox, subject: "Later".into(), to: vec![Address::new("", "kit@example.net")], ..Default::default() };
    s.mailbox.add_message(m).unwrap();
    let later = ctx(&store, 200_000.0);
    let _ = sr.take_result(&mut s, &later); // called off
    sr.stop(&mut s);
    let _ = release.send(());
    assert!(matches!(until_over(&mut sr, &mut s, &later), Taken::Done { .. }));
    assert!(!s.driver.round.is_queued(id), "a stop is not undone by an earlier call-off");
}

#[test]
fn a_sign_in_that_could_not_be_noted_for_removal_is_not_written() {
    let (mut s, id, store) = one_with_a_sign_in();
    let mut sr = SendReceive::new();
    sr.fill_written(1000);
    let before = store.0.lock().unwrap().len();
    let c = ctx(&store, 1.0);
    let mine = Tokens { refresh: "fresh-token".into(), email: "me@example.org".into(), ..Default::default() };
    assert_eq!(sr.adopt_tokens(&s, &c, id, mine), Adopted::Kept);
    let kept = store.0.lock().unwrap();
    assert_eq!(kept.len(), before, "nothing new is written");
    assert!(!kept.values().any(|v| v.contains("fresh-token")), "the new token is in no entry");
    drop(kept);
    assert_eq!(sr.take_written_keys().len(), 1000);
    let _ = &mut s;
}
