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
    fn set(&self, key: &str, secret: &str) -> Result<(), String> {
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

#[test]
fn imported_keys_naming_anything_else_in_the_store_are_dropped() {
    let mut other = kept();
    other.store_key = "imap:bob@example.org".into();
    assert!(other.sane().is_none());
    assert!(kept().sane().is_some());
}
