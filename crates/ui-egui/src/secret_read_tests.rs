// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A locked credential store never stops the window (#613): the remembered sign-ins are read on
//! a background thread, and the store is asked again only when the owner acts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult};
use serde_json::json;

use super::*;
use crate::{SecretStore, Services, StoreTrouble};

const PASSWORD_KEY: &str = "password:avery@mail.example|imap=imap.mail.example:993|smtp=smtp.mail.example:465";

/// A credential store that can be locked (`get` waits for the owner, as a keychain's prompt does)
/// or refuse, and that notes who asked.
#[derive(Clone)]
struct Locked {
    asked: Arc<Mutex<Vec<(ThreadId, String)>>>,
    /// Waiting here is the unlock prompt on screen; a message, or the sender going, answers it.
    prompt: Arc<Mutex<Receiver<()>>>,
    blocks: Arc<AtomicBool>,
    /// What the owner's answer was: no, so nothing comes back and the store says why.
    refuses: Arc<AtomicBool>,
    kept: Arc<Mutex<Option<String>>>,
}

impl Locked {
    fn new() -> (Self, Sender<()>) {
        let (answer, prompt) = mpsc::channel();
        let store = Locked {
            asked: Default::default(),
            prompt: Arc::new(Mutex::new(prompt)),
            blocks: Arc::new(AtomicBool::new(false)),
            refuses: Arc::new(AtomicBool::new(false)),
            kept: Arc::new(Mutex::new(Some("pw".to_string()))),
        };
        (store, answer)
    }

    /// How many times the window's own thread asked, and how many times anybody did.
    fn asks(&self) -> (usize, usize) {
        let me = std::thread::current().id();
        let asked = self.asked.lock().unwrap();
        (asked.iter().filter(|(who, _)| *who == me).count(), asked.len())
    }
}

impl SecretStore for Locked {
    fn get(&self, key: &str) -> Option<String> {
        self.asked.lock().unwrap().push((std::thread::current().id(), key.to_string()));
        if self.blocks.load(Ordering::SeqCst) {
            // (A bound, so that code that waits on the window's thread fails instead of hanging.)
            let _ = self.prompt.lock().unwrap().recv_timeout(Duration::from_secs(3));
        }
        if self.refuses.load(Ordering::SeqCst) {
            return None;
        }
        (key == PASSWORD_KEY).then(|| self.kept.lock().unwrap().clone()).flatten()
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _: &str) {}
    fn take_trouble(&self) -> Option<StoreTrouble> {
        self.refuses.load(Ordering::SeqCst).then_some(StoreTrouble::Locked)
    }
}

type Seen = Arc<Mutex<Vec<String>>>;

/// A window with a mail account and a Microsoft one, remembered sign-ins on, and a store in front.
fn window(store: &Locked) -> (MailApp, Id, Id, Seen) {
    let mut a = MailApp::new(Session::empty(), Services::default());
    a.autosave = false;
    a.ui.remember_sign_in = true;
    a.ui.sign_ins_bound = true;
    let imap = a
        .execute(
            "account.add",
            json!({"email": "Avery@Mail.example", "incoming": {"host": "imap.mail.example", "port": 993}, "outgoing": {"host": "smtp.mail.example", "port": 465}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let microsoft = a.execute("account.add", json!({"email": "me@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let seen = Seen::default();
    let note = seen.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        note.lock().unwrap().push(req.password.clone());
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    a.services.sign_in = Some(Box::new(|_| mpsc::channel().1));
    a.services.graph = Some(Box::new(|_| mpsc::channel().1));
    a.services.secrets = Some(Arc::new(store.clone()));
    (a, imap, microsoft, seen)
}

/// What the window does on its own, frame after frame: its logic, and every check that reads a
/// remembered sign-in (automatic Send/Receive, the Inbox pass, the quiet checks).
fn frames(a: &mut MailApp, imap: Id, microsoft: Id, count: usize) {
    let ctx = egui::Context::default();
    for _ in 0..count {
        a.logic(&ctx);
        a.can_sync_quietly(imap);
        a.can_sync_quietly(microsoft);
        a.start_inbox_pass(microsoft, true, true);
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Wait for the background read to answer (the first one is started if it hasn't been).
pub(crate) fn settle(a: &mut MailApp) {
    for _ in 0..2_000 {
        a.secrets_tick();
        if a.secret_read.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the store never answered");
}

#[test]
fn a_store_waiting_for_its_unlock_never_stops_the_window() {
    let (store, answer) = Locked::new();
    store.blocks.store(true, Ordering::SeqCst);
    let (mut a, imap, microsoft, seen) = window(&store);
    a.auto_sync = true;
    let started = std::time::Instant::now();
    frames(&mut a, imap, microsoft, 300);
    // A window that waited on the store would wait until the end of the test; a slow test machine
    // takes a few seconds for 300 frames.
    assert!(started.elapsed() < Duration::from_secs(30), "the frames waited for the store: {:?}", started.elapsed());
    let (by_window, _) = store.asks();
    assert_eq!(by_window, 0, "the window's own thread asked the store");
    assert!(!a.can_sync_quietly(imap), "not ready to sync by itself until the store has answered");
    assert!(seen.lock().unwrap().is_empty());
    // The owner unlocks it: what it holds is in memory, and no more asking was needed.
    store.blocks.store(false, Ordering::SeqCst);
    drop(answer);
    settle(&mut a);
    frames(&mut a, imap, microsoft, 20);
    assert_eq!(a.remembered("password", "avery@mail.example").as_deref(), Some("pw"));
    assert!(a.can_sync_quietly(imap));
    let (by_window, all) = store.asks();
    assert_eq!(by_window, 0);
    assert_eq!(all, 4, "one background read: a password and a refresh token for each of the two accounts");
}

#[test]
fn a_refusal_is_not_asked_again_until_the_owner_acts() {
    let (store, _answer) = Locked::new();
    store.refuses.store(true, Ordering::SeqCst);
    let (mut a, imap, microsoft, seen) = window(&store);
    a.auto_sync = true;
    frames(&mut a, imap, microsoft, 50);
    settle(&mut a);
    frames(&mut a, imap, microsoft, 300);
    assert_eq!(store.asks(), (0, 4), "asked once for each secret, in the background, and no more");
    assert!(seen.lock().unwrap().is_empty(), "nothing synchronised without a sign-in");
    assert_eq!(a.secrets_trouble, Some(StoreTrouble::Locked), "the owner is told once");

    // The owner presses Send/Receive: the store is asked again, and the run waits for the answer.
    let held = a.execute("ui.sync", json!({"account": imap})).unwrap();
    assert_eq!(held["pending"], "secrets");
    settle(&mut a);
    assert_eq!(store.asks(), (0, 8), "asked again, off the window's thread");
    assert_eq!(a.dialog.as_ref().map(|d| d.name()), Some("password"), "refused again: the owner is asked for the password");
    frames(&mut a, imap, microsoft, 300);
    assert_eq!(store.asks(), (0, 8), "and no more until the next time");

    // They unlock it, and press again: the remembered password goes with the run.
    store.refuses.store(false, Ordering::SeqCst);
    a.dialog = None;
    a.execute("ui.sync", json!({"account": imap})).unwrap();
    settle(&mut a);
    frames(&mut a, imap, microsoft, 5);
    assert_eq!(seen.lock().unwrap().as_slice(), ["pw"]);
    assert_eq!(store.asks().0, 0);
}

#[test]
fn a_secret_written_this_session_is_answered_from_memory() {
    let (store, _answer) = Locked::new();
    store.refuses.store(true, Ordering::SeqCst);
    let (mut a, imap, microsoft, _) = window(&store);
    settle(&mut a);
    assert!(!a.can_sync_quietly(imap));
    a.remember("password", "avery@mail.example", "typed");
    assert_eq!(a.remembered("password", "avery@mail.example").as_deref(), Some("typed"));
    assert!(a.can_sync_quietly(imap));
    frames(&mut a, imap, microsoft, 5);
    assert_eq!(store.asks(), (0, 4));
}
