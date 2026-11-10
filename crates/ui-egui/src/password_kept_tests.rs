// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A password is remembered as soon as the server has let the Send/Receive in, not only when the
//! whole run ends with no problem (#802): a run with a problem in one folder, or one that MailCraft
//! is closed during, keeps it all the same. A sign-in refused in the run still forgets it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{EwsFolderDeltas, EwsMailResult, EwsMessagePage, SyncRequest, SyncResult, SyncStage, SyncStep};
use serde_json::json;

use crate::{MailApp, SecretStore, Services};

const NOW: i64 = 1_791_993_600;
const TYPED: &str = "invented-password";
const EWS: &str = "https://exchange.example.org/EWS/Exchange.asmx";

/// The system's credential store, which counts what it was asked to keep.
#[derive(Clone, Default)]
struct Store {
    kept: Arc<Mutex<HashMap<String, String>>>,
    writes: Arc<Mutex<usize>>,
}

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.kept.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
        *self.writes.lock().unwrap() += 1;
        self.kept.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.kept.lock().unwrap().remove(key);
    }
}

impl Store {
    fn writes(&self) -> usize {
        *self.writes.lock().unwrap()
    }
}

/// The pieces of the run under way: the test hands them over one by one, as a server would.
type Pipe<T> = Arc<Mutex<Option<mpsc::Sender<T>>>>;

fn app(store: &Store) -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.services.secrets = Some(Arc::new(store.clone()));
    a
}

/// An IMAP account and a Send/Receive of it under way with the password just typed.
fn imap_run(store: &Store) -> (MailApp, u64, Pipe<SyncResult>) {
    let mut a = app(store);
    let pipe: Pipe<SyncResult> = Pipe::default();
    let feed = pipe.clone();
    a.services.sync = Some(Box::new(move |_: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        *feed.lock().unwrap() = Some(tx);
        rx
    }));
    let added = a.execute(
        "account.add",
        json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example", "port": 993}, "outgoing": {"host": "smtp.mail.example", "port": 465}}),
    );
    let id = added.unwrap()["id"].as_u64().unwrap();
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap()["started"], true);
    (a, id, pipe)
}

fn piece(account: u64, stage: SyncStage, errors: &[&str]) -> SyncResult {
    let errors = errors.iter().map(|e| e.to_string()).collect();
    SyncResult { account, partial: true, step: Some(SyncStep::new(stage, "Inbox", 0, 0)), errors, ..Default::default() }
}

fn send<T>(a: &mut MailApp, pipe: &Pipe<T>, piece: T) {
    pipe.lock().unwrap().as_ref().unwrap().send(piece).unwrap();
    a.poll_sync();
}

fn kept(a: &MailApp, store: &Store, id: u64) -> Option<String> {
    store.get(&MailApp::secret_key("password", a.session.mailbox.account(id).unwrap()))
}

#[test]
fn an_imap_password_is_kept_once_the_server_lets_the_run_in_whatever_comes_after() {
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    // Sending through the outgoing server, and connecting to the incoming one, prove nothing yet.
    send(&mut a, &pipe, piece(id, SyncStage::Sending, &[]));
    send(&mut a, &pipe, piece(id, SyncStage::Connecting, &[]));
    assert_eq!(kept(&a, &store, id), None);
    // The server listed its folders: it took the password.
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    assert_eq!(kept(&a, &store, id).as_deref(), Some(TYPED));
    assert!(a.syncing(), "while the run still goes on");
    // Once a run: the pieces after it don't ask the system again.
    send(&mut a, &pipe, piece(id, SyncStage::Checking, &[]));
    send(&mut a, &pipe, piece(id, SyncStage::Reading, &["Couldn't open \"Archive\": the server said no"]));
    assert_eq!(store.writes(), 1);
    // A folder with a problem ends the run with it: the password stays, and the problem is said.
    send(&mut a, &pipe, SyncResult { account: id, ..Default::default() });
    assert!(!a.syncing() && a.has_sync_problems());
    assert_eq!((kept(&a, &store, id).as_deref(), store.writes()), (Some(TYPED), 1));
    // The next runs: the same password isn't written again, a new one is.
    for (typed, writes) in [(TYPED, 1), ("a newer one", 2)] {
        a.execute("ui.sync", json!({"account": id, "password": typed})).unwrap();
        send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
        send(&mut a, &pipe, SyncResult { account: id, ..Default::default() });
        assert_eq!((kept(&a, &store, id).as_deref(), store.writes()), (Some(typed), writes));
    }

    // MailCraft closed during the first download: the next start finds the password.
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    send(&mut a, &pipe, piece(id, SyncStage::Reading, &[]));
    let mailbox = a.session.mailbox.clone();
    drop(a);
    let mut b = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    b.services.secrets = Some(Arc::new(store.clone()));
    crate::secret_read_tests::settle(&mut b);
    assert_eq!(b.remembered("password", b.account_at("avery@mail.example").map_or(0, |x| x.id)).as_deref(), Some(TYPED));
}

#[test]
fn an_imap_password_the_run_had_refused_is_not_kept_and_a_refusal_later_forgets_it() {
    // The outgoing server refused the sign-in, then the incoming one let the run in: nothing is
    // kept, and the end of the run forgets what was kept before.
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    let key = MailApp::secret_key("password", a.session.mailbox.account(id).unwrap());
    store.set(&key, "an older one").unwrap();
    send(&mut a, &pipe, piece(id, SyncStage::Connecting, &["Couldn't send mail: the server refused the sign-in"]));
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    assert_eq!((store.get(&key).as_deref(), store.writes()), (Some("an older one"), 1));
    send(&mut a, &pipe, SyncResult { account: id, ..Default::default() });
    assert_eq!(store.get(&key), None);

    // Kept once the run was let in; a sign-in refused later in the same run still forgets it.
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    assert_eq!(kept(&a, &store, id).as_deref(), Some(TYPED));
    send(
        &mut a,
        &pipe,
        SyncResult { account: id, errors: vec!["Couldn't open \"Inbox\": the server refused the sign-in".into()], ..Default::default() },
    );
    assert_eq!(kept(&a, &store, id), None);
    assert!(!a.passwords.contains_key(&id), "the next Send/Receive asks");
}

#[test]
fn an_imap_password_is_kept_only_when_the_owner_lets_it_be() {
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    a.execute("ui.rememberSignIn", json!({"value": false})).unwrap();
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    send(&mut a, &pipe, SyncResult { account: id, ..Default::default() });
    assert!(store.kept.lock().unwrap().is_empty());
    assert_eq!(a.passwords.get(&id).map(String::as_str), Some(TYPED), "kept in memory for the session");

    // Switched on while a run goes on: its next piece keeps the password.
    a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    a.execute("ui.rememberSignIn", json!({"value": true})).unwrap();
    assert!(store.kept.lock().unwrap().is_empty());
    send(&mut a, &pipe, piece(id, SyncStage::Reading, &[]));
    assert_eq!(kept(&a, &store, id).as_deref(), Some(TYPED));
}

#[test]
fn only_the_password_the_run_went_with_is_kept() {
    // A new one typed in Account Settings before the server let the run in: neither is kept by
    // this run, and the next one tries the new one.
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    a.adopt_password(id, "typed in settings".into());
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    send(&mut a, &pipe, SyncResult { account: id, ..Default::default() });
    assert_eq!(kept(&a, &store, id), None);
    assert_eq!(a.passwords.get(&id).map(String::as_str), Some("typed in settings"));

    // Typed after it: the one the server took stays kept, and the new one isn't written over it.
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    a.adopt_password(id, "typed in settings".into());
    send(&mut a, &pipe, piece(id, SyncStage::Reading, &[]));
    send(&mut a, &pipe, SyncResult { account: id, ..Default::default() });
    assert_eq!((kept(&a, &store, id).as_deref(), store.writes()), (Some(TYPED), 1));
}

#[test]
fn a_run_called_off_before_it_signed_in_keeps_nothing() {
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    send(&mut a, &pipe, piece(id, SyncStage::Connecting, &[]));
    send(&mut a, &pipe, SyncResult { account: id, stopped: true, ..Default::default() });
    assert!(!a.syncing());
    assert_eq!(kept(&a, &store, id), None);
}

#[test]
fn a_refusal_from_a_run_that_ended_unexpectedly_is_not_the_next_runs() {
    let store = Store::default();
    let (mut a, id, pipe) = imap_run(&store);
    send(&mut a, &pipe, piece(id, SyncStage::Connecting, &["Couldn't send mail: the server refused the sign-in"]));
    // The run's thread ends before its last piece.
    *pipe.lock().unwrap() = None;
    a.poll_sync();
    assert!(!a.syncing());
    // The next run is let in: its password is kept, and stays kept at its end.
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap()["started"], true);
    send(&mut a, &pipe, piece(id, SyncStage::Listed, &[]));
    send(&mut a, &pipe, SyncResult { account: id, ..Default::default() });
    assert_eq!(kept(&a, &store, id).as_deref(), Some(TYPED));
}

/// An Exchange server account and a Send/Receive of it under way with the password just typed.
fn ews_run(store: &Store) -> (MailApp, u64, Pipe<EwsMailResult>) {
    let mut a = app(store);
    let pipe: Pipe<EwsMailResult> = Pipe::default();
    let feed = pipe.clone();
    a.services.ews = Some(Box::new(move |_| {
        let (tx, rx) = mpsc::channel();
        *feed.lock().unwrap() = Some(tx);
        rx
    }));
    let added = a.execute("account.add", json!({"email": "ada@example.org", "kind": "exchange", "ews": {"url": EWS, "username": "CORP\\ada"}}));
    let id = added.unwrap()["id"].as_u64().unwrap();
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap()["started"], true);
    (a, id, pipe)
}

fn ews_piece(account: u64) -> EwsMailResult {
    EwsMailResult { account, email: "ada@example.org".into(), url: EWS.into(), partial: true, ..Default::default() }
}

#[test]
fn an_exchange_password_is_kept_once_the_server_answers_with_its_folders() {
    let store = Store::default();
    let (mut a, id, pipe) = ews_run(&store);
    // What went up with nothing taken, and how the server offers to sign in, prove nothing yet.
    send(&mut a, &pipe, EwsMailResult { signed_in_with: "ntlm".into(), ..ews_piece(id) });
    assert_eq!(kept(&a, &store, id), None);
    // The server listed its folders: it took the password.
    send(&mut a, &pipe, EwsMailResult { folders: Some(EwsFolderDeltas::default()), ..ews_piece(id) });
    assert_eq!(kept(&a, &store, id).as_deref(), Some(TYPED));
    assert!(a.syncing(), "while the run still goes on");
    // A run that ends with a folder it couldn't read keeps it, and asked the system once.
    send(&mut a, &pipe, EwsMailResult { partial: false, errors: vec!["Some folders couldn't be read from the server".into()], ..ews_piece(id) });
    assert!(!a.syncing());
    assert_eq!((kept(&a, &store, id).as_deref(), store.writes()), (Some(TYPED), 1));

    // A run with no folder list (the Inbox alone): its first page of mail says it.
    let store = Store::default();
    let (mut a, id, pipe) = ews_run(&store);
    let page = EwsMessagePage { folder: "inbox".into(), last: true, afresh: true, state: "s1".into(), ..Default::default() };
    send(&mut a, &pipe, EwsMailResult { pages: vec![page], ..ews_piece(id) });
    assert_eq!(kept(&a, &store, id).as_deref(), Some(TYPED));

    // A password the server turned down in the run is forgotten at its end, as before.
    let store = Store::default();
    let (mut a, id, pipe) = ews_run(&store);
    send(&mut a, &pipe, EwsMailResult { folders: Some(EwsFolderDeltas::default()), ..ews_piece(id) });
    assert_eq!(kept(&a, &store, id).as_deref(), Some(TYPED));
    send(&mut a, &pipe, EwsMailResult { partial: false, auth_refused: true, ..ews_piece(id) });
    assert_eq!(kept(&a, &store, id), None);
    assert!(!a.passwords.contains_key(&id), "the next Send/Receive asks");
}
