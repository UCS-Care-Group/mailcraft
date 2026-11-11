// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Encrypt stop in the window (#30 part 7): Send with Encrypt on and a recipient without a
//! Public key opens the stop for the owner's press and sends nothing; the same Send from a
//! command or an agent is refused by the engine; Send unencrypted is a button, not a parameter.

use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut session = Session::empty();
    session.clock.now = Some(NOW);
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    app.run("account.add", json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    app.run("mail.new", json!({"to": "kit@lumen.example", "subject": "Rota"})).unwrap();
    app.run("compose.encrypt", json!({"value": true})).unwrap();
    app
}

#[test]
fn the_owners_press_opens_the_stop_and_sends_nothing() {
    let mut app = app();
    app.by_hand = true;
    let out = app.run("compose.send", json!({})).unwrap();
    assert_eq!(out["pending"], "encryptStop");
    assert!(matches!(app.dialog, Some(Dialog::EncryptStop(_))));
    assert!(app.session.compose.is_some(), "still being written");
}

#[test]
fn a_command_or_an_agent_is_refused_and_no_dialog_opens() {
    let mut app = app();
    app.by_hand = false;
    let e = app.run("compose.send", json!({})).unwrap_err();
    assert!(e.contains("kit@lumen.example"), "{e}");
    assert!(app.dialog.is_none());
    assert!(app.run("compose.send", json!({"unencrypted": true})).is_err(), "no parameter sends it in the clear");
}

#[test]
fn the_words_say_what_will_happen() {
    let app = app();
    let c = app.session.compose.as_ref().unwrap();
    let will = mailcraft_engine::cmd::encrypting::will_encrypt(&app.session, c);
    let (text, problem) = crate::encryption::words(&will).unwrap();
    assert!(problem && text.contains("kit@lumen.example"), "{text}");
}
