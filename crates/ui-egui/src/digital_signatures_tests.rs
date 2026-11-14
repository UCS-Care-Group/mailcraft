// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital Signatures (#30 part 5): how an account signs is the owner's own choice. Input the
//! control channel injected (`MailApp::driven`) neither picks a key, ticks Always nor presses OK
//! (`account.setSigning` is never an agent's); the owner's own clicks do. Every key is invented.

use std::sync::Arc;
use std::sync::mpsc::Receiver;

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::signing::SigningKey;
use serde_json::json;

use super::{SigningChoice, SigningHost};
use crate::own_keys::KeyPlace;
use crate::{MailApp, Services};

const NOW: i64 = 1_794_441_600;
const KEY: &str = "owner@example.org — OpenPGP key";
const ALWAYS: &str = "Always add a Digital signature to messages from this account";

/// One OpenPGP key of the owner's, for owner@example.org.
struct OneKey;

impl SigningHost for OneKey {
    fn keys(&self) -> Receiver<Result<Vec<SigningChoice>, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let key = SigningKey::new(SignatureFormat::OpenPgp, &"9C".repeat(20), "owner@example.org");
        let _ = tx.send(Ok(key
            .into_iter()
            .map(|key| SigningChoice { key, addresses: vec!["owner@example.org".into()], place: KeyPlace::Gnupg, expires: 0 })
            .collect()));
        rx
    }
    fn pin_wanted(&self) -> Option<String> {
        None
    }
    fn give_pin(&self, _: String) {}
    fn no_pin(&self) {}
}

/// The window with Digital Signatures open for an IMAP account of owner@example.org; its id.
fn opened() -> (Harness<'static, MailApp>, u64) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.services.signing = Some(Arc::new(OneKey));
    let server = json!({"email": "owner@example.org", "incoming": {"host": "imap.example.org"}, "outgoing": {"host": "smtp.example.org"}});
    let id = app.execute("account.add", server).unwrap()["id"].as_u64().unwrap();
    app.execute("ui.dialog", json!({"name": "digitalSignatures", "account": id})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    assert!(h.query_by_label(KEY).is_some(), "the key is listed");
    (h, id)
}

fn open(h: &Harness<'static, MailApp>) -> bool {
    matches!(h.state().dialog, Some(crate::dialogs::Dialog::DigitalSignatures(_)))
}

fn signing(h: &Harness<'static, MailApp>, id: u64) -> (Option<String>, bool) {
    let s = &h.state().session.mailbox.account(id).unwrap().signing;
    (s.key.as_ref().map(|k| k.fingerprint.clone()), s.always)
}

/// The key and Always as the open dialog has them.
fn chosen(h: &Harness<'static, MailApp>) -> (Option<String>, bool) {
    match &h.state().dialog {
        Some(crate::dialogs::Dialog::DigitalSignatures(d)) => (d.chosen.as_ref().map(|k| k.fingerprint.clone()), d.always),
        _ => panic!("Digital Signatures isn't open"),
    }
}

fn click(h: &mut Harness<'static, MailApp>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(4);
}

#[test]
fn input_the_control_channel_injected_changes_nothing_and_the_owners_clicks_do() {
    let (mut h, id) = opened();
    let before = signing(&h, id);
    assert_eq!(before, (None, false));

    // Driven: the key's radio and Always change nothing in the dialog.
    h.state_mut().driven = true;
    click(&mut h, KEY);
    click(&mut h, ALWAYS);
    assert_eq!(chosen(&h), (None, false), "injected clicks pick nothing");
    // The owner picks the key; then an injected OK and Enter still set nothing, and the dialog stays.
    h.state_mut().driven = false;
    click(&mut h, KEY);
    assert_eq!(chosen(&h).0, Some("9C".repeat(20)));
    h.state_mut().driven = true;
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    assert_eq!(signing(&h, id), before, "an injected OK sets nothing");
    assert!(open(&h), "and the dialog stays open");
    assert!(h.query_by_label_contains("Only you choose how this account signs").is_some(), "it says why");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(signing(&h, id), before, "nor does an injected Enter");
    assert!(open(&h));

    // The owner's own OK sets it and closes.
    h.state_mut().driven = false;
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    assert_eq!(signing(&h, id), (Some("9C".repeat(20)), false));
    assert!(!open(&h));
}

/// A host whose smartcard wants its PIN once, keeping what the window gives it.
#[derive(Default)]
struct PinHost {
    asked: std::sync::Mutex<bool>,
    given: std::sync::Mutex<Vec<String>>,
}

impl SigningHost for PinHost {
    fn keys(&self) -> Receiver<Result<Vec<SigningChoice>, String>> {
        std::sync::mpsc::channel().1
    }
    fn pin_wanted(&self) -> Option<String> {
        let mut asked = self.asked.lock().unwrap();
        (!std::mem::replace(&mut *asked, true)).then(|| "Card 7".to_string())
    }
    fn give_pin(&self, pin: String) {
        self.given.lock().unwrap().push(pin);
    }
    fn no_pin(&self) {}
}

fn pin_open(h: &Harness<'static, MailApp>) -> bool {
    matches!(h.state().dialog, Some(crate::dialogs::Dialog::SigningPin(_)))
}

/// Security review of #1036: the smartcard PIN question takes no input the control channel
/// injected: neither what it types nor its OK or Enter; the owner's own typing and OK give the PIN.
#[test]
fn the_pin_question_ignores_input_the_control_channel_injected() {
    let host = Arc::new(PinHost::default());
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.services.signing = Some(host.clone());
    app.ask_for_signing_pin();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    assert!(pin_open(&h), "the question is asked");
    let typed = |h: &Harness<'static, MailApp>| match &h.state().dialog {
        Some(crate::dialogs::Dialog::SigningPin(d)) => d.pin.to_string(),
        _ => String::new(),
    };

    // Injected typing changes nothing.
    h.state_mut().driven = true;
    h.get_by_role(Role::PasswordInput).click();
    h.run_steps(2);
    h.get_by_role(Role::PasswordInput).type_text("9999");
    h.run_steps(4);
    assert_eq!(typed(&h), "", "injected typing is dropped");

    // The owner types; an injected OK and Enter give nothing, and the question stays.
    h.state_mut().driven = false;
    h.get_by_role(Role::PasswordInput).click();
    h.run_steps(2);
    h.get_by_role(Role::PasswordInput).type_text("1234");
    h.run_steps(4);
    assert_eq!(typed(&h), "1234");
    h.state_mut().driven = true;
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(host.given.lock().unwrap().is_empty(), "an injected OK or Enter gives no PIN");
    assert!(pin_open(&h));

    // The owner's own OK gives it.
    h.state_mut().driven = false;
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    assert_eq!(*host.given.lock().unwrap(), vec!["1234".to_string()]);
    assert!(!pin_open(&h));
}

/// Security review of #1036: Digital Signatures closes when another mailbox takes the place of
/// the one it was opened in, and offers no key at all as a choice.
#[test]
fn digital_signatures_closes_with_its_mailbox_and_offers_no_key() {
    let (mut h, id) = opened();
    click(&mut h, KEY);
    assert_eq!(chosen(&h).0, Some("9C".repeat(20)));
    click(&mut h, "No certificate or key");
    assert_eq!(chosen(&h), (None, false), "no key, and so no Always");
    h.state_mut().session.replace_mailbox(mailcraft_model::Mailbox::with_local_account("", ""));
    h.run_steps(4);
    assert!(!open(&h), "closed with its mailbox");
    let _ = id;
}
