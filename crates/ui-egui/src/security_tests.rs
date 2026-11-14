// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › Security (#30): an OpenPGP key is marked Verified only by the owner, in the
//! dialog that shows its fingerprint (R6, R23), and an import goes to the host and is kept by
//! its public facts alone; with synthetic `example.org` keys made for these tests alone.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use crate::files::base64_lite::encode;
use egui::PointerButton;
use egui_kittest::kittest::Queryable;
use mailcraft_model::digital_signature::SignatureFormat;
use serde_json::{Value, json};

use crate::MailApp;
use crate::control::{ControlRequest, Outcome};
use crate::dialogs::Dialog;
use crate::own_keys::{Imported, ImportedKey, OwnKeysAnswer, OwnKeysJob};
use crate::people_view::testing::{self, Window};
use crate::security::{PURPOSE_OPENPGP, SecurityDialog};
use crate::security_fixtures_tests::{CAROL_PUBLIC, CAROL_SECRET};
use crate::{SecretStore, security};

const CAROL: &str = "carol@example.org";
const CAROL_FPR: &str = "B218EB030C441424207AA6C010C35688F1ADA5A2";

fn window() -> Window {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("keys.import", json!({"data": encode(CAROL_PUBLIC.as_bytes())})).unwrap();
    w.run_steps(2);
    w
}

fn verified(a: &MailApp) -> bool {
    a.session.mailbox.public_keys.of(CAROL).iter().any(|k| k.verified)
}

fn agent(a: &mut MailApp, command: &str, params: Value) -> Option<Value> {
    let (req, _rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    match crate::control::handle(a, &egui::Context::default(), &req.from_agent(Vec::new())) {
        Outcome::Done(v) => Some(v),
        _ => None,
    }
}

#[test]
fn an_agent_can_neither_open_the_dialog_nor_mark_a_key() {
    let mut w = window();
    let a = w.state_mut();
    for command in ["ui.verifyKey", "ui.markKeyVerified"] {
        let out = agent(a, command, json!({"address": CAROL, "fingerprint": CAROL_FPR, "verified": true}));
        assert!(out.is_some_and(|v| v["ok"] == false), "{command}");
    }
    assert!(a.dialog.is_none());
    // Not even with the dialog open in front of the owner.
    a.execute("ui.verifyKey", json!({"address": CAROL, "fingerprint": CAROL_FPR})).unwrap();
    let out = agent(a, "ui.markKeyVerified", json!({"address": CAROL, "fingerprint": CAROL_FPR, "verified": true}));
    assert!(out.is_some_and(|v| v["ok"] == false));
    assert!(!verified(a));
}

#[test]
fn a_key_is_marked_only_while_its_dialog_is_open() {
    let mut w = window();
    let a = w.state_mut();
    let err = a.execute("ui.markKeyVerified", json!({"address": CAROL, "fingerprint": CAROL_FPR, "verified": true})).unwrap_err();
    assert!(err.contains("only in the dialog"), "{err}");
    assert!(!verified(a));
    // Another key's dialog doesn't count.
    a.execute("ui.verifyKey", json!({"address": CAROL, "fingerprint": CAROL_FPR})).unwrap();
    assert!(a.execute("ui.markKeyVerified", json!({"address": CAROL, "fingerprint": "AAAA", "verified": true})).is_err());
    assert!(!verified(a));
}

#[test]
fn the_dialogs_own_button_marks_the_key() {
    let mut w = window();
    w.state_mut().execute("ui.verifyKey", json!({"address": CAROL, "fingerprint": CAROL_FPR})).unwrap();
    w.run_steps(3);
    assert!(w.get_by_label("Mark as verified").rect().width() > 0.0);
    let at = w.state().hit.rect("keyVerified", 0).unwrap().center();
    testing::click(&mut w, at, PointerButton::Primary, 1);
    w.run_steps(2);
    assert!(verified(w.state()));
    assert!(w.state().dialog.is_none());
}

#[derive(Default)]
struct Store(Mutex<HashMap<String, String>>);

impl SecretStore for Store {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), mailcraft_host::SecretError> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().unwrap().remove(key);
    }
}

fn kept() -> ImportedKey {
    ImportedKey {
        format: SignatureFormat::OpenPgp,
        fingerprint: "B218 EB03 0C44 1424 207A A6C0 10C3 5688 F1AD A5A2".into(),
        name: "Carol Example <carol@example.org>".into(),
        addresses: vec![CAROL.into()],
        not_after: 0,
        store_key: format!("key:openpgp:{}", CAROL_FPR.to_lowercase()),
        can_sign: true,
    }
}

#[test]
fn an_import_goes_to_the_host_and_keeps_public_facts_only() {
    let mut w = testing::window("view.mail");
    let store = Arc::new(Store::default());
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = asked.clone();
    let host_store = store.clone();
    w.state_mut().services.secrets = Some(store.clone());
    w.state_mut().services.own_keys = Some(Box::new(move |job: OwnKeysJob| -> Receiver<OwnKeysAnswer> {
        let (tx, rx) = channel();
        let answer = match job {
            OwnKeysJob::Import { format, file, passphrase } => {
                seen.lock().unwrap().push(format!("{format:?} {} {}", file.0.len(), passphrase.is_some()));
                host_store.set(&kept().store_key, "a file").unwrap();
                OwnKeysAnswer::Imported(Ok(Imported::Kept { key: kept(), public: None }))
            }
            OwnKeysJob::List => OwnKeysAnswer::Listed(Default::default()),
            OwnKeysJob::GnupgValid => OwnKeysAnswer::GnupgValid(Ok(Vec::new())),
        };
        let _ = tx.send(answer);
        rx
    }));
    security::picked(w.state_mut(), PURPOSE_OPENPGP, "/home/someone/carol.asc", Ok(CAROL_SECRET.as_bytes().to_vec()));
    assert!(matches!(&w.state().dialog, Some(Dialog::Security(d)) if matches!(&**d, SecurityDialog::Import(_))));
    w.run_steps(2);
    let at = w.get_by_label("Import").rect().center();
    testing::click(&mut w, at, PointerButton::Primary, 1);
    w.run_steps(3);
    // An OpenPGP key is only read: no passphrase goes with it.
    assert_eq!(*asked.lock().unwrap(), [format!("OpenPgp {} false", CAROL_SECRET.len())]);
    assert!(w.state().dialog.is_none());
    assert_eq!(w.state().ui.imported_keys, [kept()]);
    let prefs = serde_json::to_string(&w.state().ui).unwrap();
    assert!(!prefs.contains("PGP PRIVATE KEY") && !prefs.contains("a file"));
    // Removing the program's data takes the key out of the credential store too.
    assert!(security::forget_imported(w.state_mut()).is_empty());
    assert!(store.0.lock().unwrap().is_empty() && w.state().ui.imported_keys.is_empty());
}

#[test]
fn a_hosts_refusal_stays_in_the_dialog_and_nothing_is_kept() {
    let mut w = testing::window("view.mail");
    w.state_mut().services.own_keys = Some(Box::new(|_job: OwnKeysJob| {
        let (tx, rx) = channel();
        let _ = tx.send(OwnKeysAnswer::Imported(Err("The passphrase is wrong.".into())));
        rx
    }));
    security::picked(w.state_mut(), PURPOSE_OPENPGP, "carol.asc", Ok(CAROL_SECRET.as_bytes().to_vec()));
    w.run_steps(2);
    let at = w.get_by_label("Import").rect().center();
    testing::click(&mut w, at, PointerButton::Primary, 1);
    w.run_steps(3);
    let Some(Dialog::Security(d)) = &w.state().dialog else { panic!("the dialog closed") };
    let SecurityDialog::Import(i) = &**d else { panic!("another dialog") };
    assert_eq!(i.error, "The passphrase is wrong.");
    assert!(!i.busy);
    assert!(w.state().ui.imported_keys.is_empty());
}

/// Frames as the desktop runs them, each with what the control channel injected for it, until
/// all of it is in, and a few more.
fn driven_frames(w: &mut Window) {
    for _ in 0..60 {
        let last = w.state().synthetic.is_empty();
        let mut raw = std::mem::take(w.input_mut());
        w.state_mut().raw_input_hook(&mut raw);
        *w.input_mut() = raw;
        w.step();
        if last {
            break;
        }
    }
    w.run_steps(3);
}

/// An agent allowed to click (`--allow input`) clicks the window's own buttons.
fn agent_clicks(w: &mut Window, kind: &str, id: u64) {
    let (req, _rx) = ControlRequest::new("ui.click", json!({"kind": kind, "id": id}));
    let ctx = w.ctx.clone();
    let _ = crate::control::handle(w.state_mut(), &ctx, &req.from_agent(vec![mailcraft_engine::agent::Class::Input]));
    driven_frames(w);
}

#[test]
fn an_agent_that_may_click_cant_mark_a_key_verified() {
    let mut w = window();
    w.state_mut().execute("ui.verifyKey", json!({"address": CAROL, "fingerprint": CAROL_FPR})).unwrap();
    w.run_steps(3);
    assert!(w.state().hit.rect("keyVerified", 0).is_some());
    agent_clicks(&mut w, "keyVerified", 0);
    assert!(!verified(w.state()), "a click the control channel injected is nobody's word");
    assert!(matches!(&w.state().dialog, Some(Dialog::Security(_))), "the dialog stays for the owner");
    // Nor a request of the control channel, even with the dialog open.
    let was = std::mem::replace(&mut w.state_mut().driven, true);
    let err = w.state_mut().execute("ui.markKeyVerified", json!({"address": CAROL, "fingerprint": CAROL_FPR, "verified": true})).unwrap_err();
    w.state_mut().driven = was;
    assert!(err.contains("by you"), "{err}");
    assert!(!verified(w.state()));
}

#[test]
fn a_driven_removal_of_an_imported_key_is_refused() {
    let mut w = testing::window("view.mail");
    let store = Arc::new(Store::default());
    store.set(&kept().store_key, "a file").unwrap();
    w.state_mut().services.secrets = Some(store.clone());
    w.state_mut().ui.imported_keys.push(kept());
    w.state_mut().driven = true;
    let r = w.state_mut().execute("ui.removePrivateKey", json!({"fingerprint": CAROL_FPR, "confirmed": true}));
    w.state_mut().driven = false;
    assert!(r.is_err());
    assert_eq!(w.state().ui.imported_keys.len(), 1);
    assert!(store.get(&kept().store_key).is_some(), "still in the credential store");
    // Nor a script's request through the control channel, input not driven.
    assert!(w.state_mut().execute("ui.removePrivateKey", json!({"fingerprint": CAROL_FPR, "confirmed": true})).is_err());
    assert!(store.get(&kept().store_key).is_some(), "a script never removes the owner's key");
    // The owner's own Confirm (the question's answer goes through `run`) removes it.
    w.state_mut().run("ui.removePrivateKey", json!({"fingerprint": CAROL_FPR, "confirmed": true})).unwrap();
    assert!(store.get(&kept().store_key).is_none() && w.state().ui.imported_keys.is_empty());
}

/// The control channel is a script, never the owner (Martin, 2026-11-11): with the very key's
/// dialog open and no input driven, its `engine.execute` of `ui.markKeyVerified` is refused, as
/// for `ui.restartToUpdate`; only the dialog's button, the owner's press, marks it.
#[test]
fn the_control_channel_never_marks_a_key_verified() {
    let mut w = window();
    w.state_mut().execute("ui.verifyKey", json!({"address": CAROL, "fingerprint": CAROL_FPR})).unwrap();
    w.run_steps(3);
    assert!(!w.state().driven);
    let (req, _rx) = ControlRequest::new(
        "engine.execute",
        json!({"command": "ui.markKeyVerified", "params": {"address": CAROL, "fingerprint": CAROL_FPR, "verified": true}}),
    );
    let ctx = w.ctx.clone();
    let out = crate::control::handle(w.state_mut(), &ctx, &req);
    assert!(matches!(&out, Outcome::Done(v) if v["ok"] == false), "refused");
    assert!(!verified(w.state()));
    assert!(matches!(&w.state().dialog, Some(Dialog::Security(_))), "the dialog stays for the owner");
    // The same request as the window's own press goes through.
    w.state_mut().run("ui.markKeyVerified", json!({"address": CAROL, "fingerprint": CAROL_FPR, "verified": true})).unwrap();
    assert!(verified(w.state()));
}

/// A host that counts the jobs of each kind, and answers them as `answer` says.
type Jobs = Arc<Mutex<Vec<&'static str>>>;

fn host(w: &mut Window, answer: impl Fn(&OwnKeysJob) -> Option<OwnKeysAnswer> + Send + Sync + 'static) -> Jobs {
    let jobs: Jobs = Arc::default();
    let seen = jobs.clone();
    w.state_mut().services.own_keys = Some(Box::new(move |job: OwnKeysJob| -> Receiver<OwnKeysAnswer> {
        seen.lock().unwrap().push(match &job {
            OwnKeysJob::List => "list",
            OwnKeysJob::GnupgValid => "gnupg",
            OwnKeysJob::Import { .. } => "import",
        });
        let (tx, rx) = channel();
        if let Some(a) = answer(&job) {
            let _ = tx.send(a);
        }
        // (None: the host let go of the job, its sender dropped, without an answer.)
        rx
    }));
    jobs
}

fn count(jobs: &Jobs, kind: &str) -> usize {
    jobs.lock().unwrap().iter().filter(|k| **k == kind).count()
}

#[test]
fn gnupg_is_asked_again_when_the_window_comes_back_at_most_once_a_minute() {
    let mut w = testing::window("view.mail");
    let jobs = host(&mut w, |job| matches!(job, OwnKeysJob::GnupgValid).then(|| OwnKeysAnswer::GnupgValid(Ok(Vec::new()))));
    w.run_steps(2);
    assert_eq!(count(&jobs, "gnupg"), 1, "once when the window is up");
    assert!(w.state().security.gnupg_read.is_some());
    let a = w.state_mut();
    security::tick(a, false);
    security::tick(a, true);
    assert_eq!(count(&jobs, "gnupg"), 1, "not again within the minute");
    a.session.clock.now = Some(testing::now() + 61);
    security::tick(a, true);
    assert_eq!(count(&jobs, "gnupg"), 1, "not without the window having gone to the back");
    security::tick(a, false);
    security::tick(a, true);
    assert_eq!(count(&jobs, "gnupg"), 2, "back to the front after a minute");
    security::tick(a, true);
    assert_eq!(w.state().security.gnupg_read, Some(testing::now() + 61));
}

#[test]
fn a_host_that_lets_go_of_a_job_ends_it() {
    let mut w = testing::window("view.mail");
    let jobs = host(&mut w, |_| None);
    // A listing that never comes is a listing that says so, and isn't asked for every frame.
    w.state_mut().execute("ui.refreshOwnKeys", json!({})).unwrap();
    w.run_steps(4);
    let listing = w.state().security.listing.clone().unwrap();
    assert_eq!(listing.trouble.len(), 1, "{:?}", listing.trouble);
    assert_eq!(count(&jobs, "list"), 1);
    // An import is no longer busy, and the dialog says why.
    security::picked(w.state_mut(), PURPOSE_OPENPGP, "carol.asc", Ok(CAROL_SECRET.as_bytes().to_vec()));
    w.run_steps(2);
    let at = w.get_by_label("Import").rect().center();
    testing::click(&mut w, at, PointerButton::Primary, 1);
    w.run_steps(3);
    let Some(Dialog::Security(d)) = &w.state().dialog else { panic!("the dialog closed") };
    let SecurityDialog::Import(i) = &**d else { panic!("another dialog") };
    assert!(!i.busy && !i.error.is_empty(), "{}", i.error);
    assert_eq!(count(&jobs, "import"), 1);
}

#[test]
fn an_own_key_is_verified_only_for_the_owners_accounts() {
    for own in [false, true] {
        let mut w = testing::window("view.mail");
        if own {
            w.state_mut().session.mailbox.accounts[0].email = "Carol@Example.org".into();
        }
        assert_eq!(w.state().session.mailbox.accounts.iter().any(|a| a.email.eq_ignore_ascii_case(CAROL)), own);
        host(&mut w, |job| {
            matches!(job, OwnKeysJob::Import { .. })
                .then(|| OwnKeysAnswer::Imported(Ok(Imported::Kept { key: kept(), public: Some(CAROL_PUBLIC.as_bytes().to_vec()) })))
        });
        security::picked(w.state_mut(), PURPOSE_OPENPGP, "carol.asc", Ok(CAROL_SECRET.as_bytes().to_vec()));
        w.run_steps(2);
        let at = w.get_by_label("Import").rect().center();
        testing::click(&mut w, at, PointerButton::Primary, 1);
        w.run_steps(3);
        assert_eq!(w.state().ui.imported_keys.len(), 1);
        assert!(!w.state().session.mailbox.public_keys.of(CAROL).is_empty(), "its public half is kept");
        assert_eq!(verified(w.state()), own, "a User ID that isn't one of the owner's accounts stays not Verified");
    }
}

#[test]
fn no_key_is_kept_past_what_the_page_lists() {
    let mut w = testing::window("view.mail");
    let store = Arc::new(Store::default());
    w.state_mut().services.secrets = Some(store.clone());
    let held: Arc<Mutex<Option<std::sync::mpsc::Sender<OwnKeysAnswer>>>> = Arc::default();
    let hold = held.clone();
    w.state_mut().services.own_keys = Some(Box::new(move |_job: OwnKeysJob| {
        let (tx, rx) = channel();
        *hold.lock().unwrap() = Some(tx);
        rx
    }));
    security::picked(w.state_mut(), PURPOSE_OPENPGP, "carol.asc", Ok(CAROL_SECRET.as_bytes().to_vec()));
    w.run_steps(2);
    let at = w.get_by_label("Import").rect().center();
    testing::click(&mut w, at, PointerButton::Primary, 1);
    // Full while the host works: what it kept is taken out of the store again.
    let full: Vec<ImportedKey> = (0..crate::own_keys::MAX_IMPORTED)
        .map(|i| ImportedKey { store_key: format!("key:openpgp:{:040x}", i + 1), fingerprint: format!("{:040X}", i + 1), ..kept() })
        .collect();
    w.state_mut().ui.imported_keys = full.clone();
    store.set(&kept().store_key, "a file").unwrap();
    let _ = held.lock().unwrap().take().unwrap().send(OwnKeysAnswer::Imported(Ok(Imported::Kept { key: kept(), public: None })));
    w.run_steps(3);
    assert_eq!(w.state().ui.imported_keys, full);
    assert!(store.get(&kept().store_key).is_none(), "nothing in the store that the page doesn't list");
    let (ok, said) = w.state().security.said.clone().unwrap();
    assert!(!ok && said.contains("at most"), "{said}");
    // And full, nothing is started at all.
    let err = w.state_mut().execute("ui.importPrivateKey", json!({"format": "openpgp"})).unwrap_err();
    assert!(err.contains("at most"), "{err}");
}

#[test]
fn a_key_picked_for_a_contacts_card_goes_to_the_form_that_asked() {
    let mut w = testing::window("view.people");
    let key = || mailcraft_engine::cmd::keys::card_key_from(CAROL_PUBLIC.as_bytes()).unwrap();
    let on_card = |w: &mut Window| match &mut w.state_mut().dialog {
        Some(Dialog::Form(crate::forms::Form::Contact(f))) => f.clone().add_key(key()).is_err(),
        _ => panic!("no contact form"),
    };
    w.state_mut().execute("ui.dialog", json!({"name": "contact"})).unwrap();
    w.run_steps(3);
    w.get_by_label("Add from file…").scroll_to_me();
    w.run_steps(3);
    let at = w.get_by_label("Add from file…").rect().center();
    testing::click(&mut w, at, PointerButton::Primary, 1);
    w.run_steps(2);
    assert!(w.state().security.card_pick.is_some(), "the first form asked at {at:?}: {:?}", w.state().status_msg);
    // Another form in its place before the file comes: not that one's key.
    w.state_mut().dialog = None;
    w.state_mut().execute("ui.dialog", json!({"name": "contact"})).unwrap();
    w.run_steps(2);
    security::picked(w.state_mut(), crate::security::PURPOSE_CARD, "carol.asc", Ok(CAROL_PUBLIC.as_bytes().to_vec()));
    assert!(!on_card(&mut w), "a form that didn't ask gets nothing");
    // The form that asks gets it.
    w.get_by_label("Add from file…").scroll_to_me();
    w.run_steps(3);
    let at = w.get_by_label("Add from file…").rect().center();
    testing::click(&mut w, at, PointerButton::Primary, 1);
    w.run_steps(2);
    let asked = w.state().security.card_pick;
    assert!(asked.is_some(), "the form asked");
    assert!(matches!(&w.state().dialog, Some(Dialog::Form(crate::forms::Form::Contact(f))) if f.key_pick() == asked));
    security::picked(w.state_mut(), crate::security::PURPOSE_CARD, "carol.asc", Ok(CAROL_PUBLIC.as_bytes().to_vec()));
    assert!(on_card(&mut w), "{:?}", w.state().status_msg);
    // Once: a second file with no pick asked for goes nowhere.
    let before = w.state().dialog.as_ref().map(|d| d.name());
    security::picked(w.state_mut(), crate::security::PURPOSE_CARD, "carol.asc", Ok(CAROL_PUBLIC.as_bytes().to_vec()));
    assert_eq!(w.state().dialog.as_ref().map(|d| d.name()), before);
}

#[test]
fn imported_keys_naming_anything_else_in_the_store_are_dropped() {
    let mut other = kept();
    other.store_key = "imap:bob@example.org".into();
    assert!(other.sane().is_none());
    assert!(kept().sane().is_some());
}
