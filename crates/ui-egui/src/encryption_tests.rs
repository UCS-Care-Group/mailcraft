// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Encrypt stop in the window (#30 part 7): Send with Encrypt on and a recipient without a
//! Public key opens the stop for the owner's press and sends nothing; the same Send from a
//! command or an agent is refused by the engine; Send unencrypted is a button, not a parameter.

use mailcraft_engine::Session;
use serde_json::json;

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::public_keys::{KeySource, PublicKey};

use crate::dialogs::Dialog;
use crate::encryption::Stop;
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

const SMIME_MAYA: &[u8] = include_bytes!("../../smime/testdata/maya.enc.der");
const SMIME_KIT: &[u8] = include_bytes!("../../smime/testdata/kit.enc.der");

#[test]
fn input_the_control_channel_injected_never_sends_in_the_clear() {
    // Its click on Send unencrypted does nothing: the message stays as it is, to be Encrypted.
    let mut app = app();
    app.driven = true;
    Stop::send_unencrypted(&mut app);
    assert!(app.session.compose.is_some());
    assert_eq!(app.session.compose.as_ref().unwrap().encrypt, Some(true));
    assert!(app.session.mailbox.messages.values().all(|m| m.draft || m.subject != "Rota"), "nothing was queued");
    // The owner's own click does: Encrypt is off for the message, and it goes.
    app.driven = false;
    Stop::send_unencrypted(&mut app);
    assert!(app.session.compose.is_none());
    let outbox = app.session.mailbox.messages.values().find(|m| m.subject == "Rota" && !m.draft).unwrap();
    assert_eq!(outbox.encrypt, Some(false));
    assert!(outbox.encrypt_with.is_none());
}

#[test]
fn the_ribbons_encrypt_is_not_turned_off_by_input_the_control_channel_injected() {
    let mut app = app();
    app.driven = true;
    // Off, by value or by toggling: refused. On is not.
    assert!(app.run("compose.encrypt", json!({"value": false})).is_err());
    assert!(app.run("compose.encrypt", json!({})).is_err());
    assert_eq!(app.session.compose.as_ref().unwrap().encrypt, Some(true));
    assert!(app.run("compose.encrypt", json!({"value": true})).is_ok());
    // The owner's own click turns it off.
    app.driven = false;
    assert!(app.run("compose.encrypt", json!({"value": false})).is_ok());
    assert_eq!(app.session.compose.as_ref().unwrap().encrypt, Some(false));
}

#[test]
fn a_key_is_chosen_by_the_owners_hand_alone() {
    let mut app = app();
    for (der, fingerprint) in [(SMIME_MAYA, "11".repeat(32)), (SMIME_KIT, "22".repeat(32))] {
        let key = PublicKey {
            format: SignatureFormat::Smime,
            data: std::sync::Arc::new(der.to_vec()),
            fingerprint,
            not_after: 0,
            seen: NOW,
            verified: false,
            source: KeySource::Collected,
        };
        assert!(app.session.mailbox.public_keys.add("kit@lumen.example", key));
    }
    app.session.clock.now = Some(1_900_000_000);
    let stop = crate::encryption::before_send(&app).expect("several keys stop it");
    app.driven = true;
    assert!(!stop.choose(&mut app, "kit@lumen.example", &"11".repeat(32)));
    assert!(crate::encryption::before_send(&app).is_some(), "still asked");
    app.driven = false;
    assert!(stop.choose(&mut app, "kit@lumen.example", &"11".repeat(32)));
    assert!(crate::encryption::before_send(&app).is_none(), "the owner's word settles it");
}

#[test]
fn the_account_setting_is_changed_in_the_digital_signatures_dialog_by_the_owner_alone() {
    let mut app = app();
    let account = app.session.mailbox.accounts[0].id;
    assert!(!app.session.mailbox.account(account).unwrap().encryption.when_possible);
    // (The dialog's OK runs `account.setEncryption` unless the input was injected.)
    app.driven = true;
    assert!(!crate::digital_signatures::apply_encryption(&mut app, account, true));
    assert!(!app.session.mailbox.account(account).unwrap().encryption.when_possible);
    app.driven = false;
    assert!(crate::digital_signatures::apply_encryption(&mut app, account, true));
    assert!(app.session.mailbox.account(account).unwrap().encryption.when_possible);
}

#[test]
fn the_line_says_what_will_happen_to_the_message() {
    use mailcraft_engine::cmd::encrypting::{KeyConflict, Will};
    let say = |will: Will| crate::encryption::words(&will);
    let (text, problem) = say(Will::ClearBecause { addresses: vec!["dan@lumen.example".into()] }).unwrap();
    assert!(problem && text.contains("Goes unencrypted") && text.contains("dan@lumen.example"), "{text}");
    let (text, problem) = say(Will::Conflict {
        format: SignatureFormat::Smime,
        conflicts: vec![KeyConflict { address: "bo@lumen.example".into(), candidates: Vec::new() }],
    })
    .unwrap();
    assert!(problem && text.contains("bo@lumen.example") && text.contains("several keys"), "{text}");
    assert!(say(Will::Clear).is_none());
}
