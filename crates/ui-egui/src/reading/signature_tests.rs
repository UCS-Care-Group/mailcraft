// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's Digital signature in the window (#30): the bar with what the check found, its
//! Details, and the seal on the message's row. RFC 9216's example certificates, signed by
//! OpenSSL; the text is invented.

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use serde_json::json;

use crate::{MailApp, Services};

const CA: &[u8] = include_bytes!("../../../smime/testdata/ca.rsa.der");
const DETACHED: &[u8] = include_bytes!("../../../smime/testdata/detached.eml");
const SHA1: &[u8] = include_bytes!("../../../smime/testdata/sha1.eml");

/// 2026-11-09.
const NOW: i64 = 1_794_182_400;

type Window = egui_kittest::Harness<'static, MailApp>;

/// Bob's window with `messages` in his Inbox, the RFC 9216 authority trusted when `trusted`.
fn window(messages: &[Vec<u8>], trusted: bool) -> (Window, Vec<u64>) {
    let mut mb = Mailbox::with_local_account("Bob", "bob@smime.example");
    let inbox = mb.inbox().unwrap();
    let mut ids = Vec::new();
    for bytes in messages {
        let mut m = mailcraft_mime::parse_fetched(bytes);
        m.folder = inbox;
        ids.push(mb.add_message(m).unwrap());
    }
    let mut s = Session::at(mb, NOW, 0);
    if trusted {
        s.set_trust_anchors([CA]);
    }
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let mut harness =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(3);
    (harness, ids)
}

fn open(harness: &mut Window, id: u64) {
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(4);
}

fn says(harness: &Window, text: &str) -> bool {
    harness.query_all_by_label_contains(text).next().is_some()
}

#[test]
fn a_verified_signature_has_its_bar_details_and_seal() {
    let (mut harness, ids) = window(&[DETACHED.to_vec()], true);
    open(&mut harness, ids[0]);
    assert!(harness.state().hit.rect("digitalSignature", ids[0]).is_some());
    assert!(says(&harness, "Digitally signed by alice@smime.example, and verified."));
    assert!(!says(&harness, "Fingerprint (SHA-256):"));
    harness.get_by_label("Details").click();
    harness.run_steps(4);
    assert!(says(&harness, "Fingerprint (SHA-256):"));
    assert!(says(&harness, "Sample LAMPS RSA Certification Authority"));
    assert!(says(&harness, "Revocation not checked."));
    assert!(harness.state().hit.rect("messageSignature", ids[0]).is_some(), "the row wears the seal");
}

#[test]
fn a_signer_not_verified_a_change_and_an_old_algorithm_are_said() {
    let changed: Vec<u8> = String::from_utf8_lossy(DETACHED).replace("Tuesday", "Thursday").into_bytes();
    let (mut harness, ids) = window(&[DETACHED.to_vec(), changed, SHA1.to_vec()], false);
    open(&mut harness, ids[0]);
    assert!(says(&harness, "but the signer isn't verified."));
    assert!(says(&harness, "wasn't issued by a certification authority this computer trusts"));
    open(&mut harness, ids[1]);
    assert!(says(&harness, "This message was changed after it was signed."));
    open(&mut harness, ids[2]);
    assert!(says(&harness, "This message's Digital signature can't be checked."));
    assert!(says(&harness, "It uses an algorithm that's no longer safe."));
}

/// A signed message inside a list's wrapping, with a footer nobody signed: never verified for
/// the message, and the bar says only part of it is signed.
#[test]
fn a_message_signed_only_in_part_is_never_verified() {
    let inner = String::from_utf8_lossy(DETACHED).into_owned();
    let wrapped = format!(
        "From: alice@smime.example\r\nTo: bob@smime.example\r\nSubject: Minutes\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\n{inner}\r\n--outer\r\nContent-Type: text/plain\r\n\r\nWire 10000 EUR to Mallory.\r\n--outer--\r\n"
    );
    let (mut harness, ids) = window(&[wrapped.into_bytes()], true);
    open(&mut harness, ids[0]);
    assert!(says(&harness, "Part of this message is Digitally signed by alice@smime.example, and verified; the rest isn't signed."));
    assert!(!says(&harness, "Digitally signed by alice@smime.example, and verified."));
    let d = harness.state_mut().session.digital_signature(ids[0]).unwrap();
    assert_eq!(d.verdict, mailcraft_model::digital_signature::Verdict::NotVerified);
}

#[test]
fn a_message_that_isnt_signed_has_no_bar() {
    let plain = b"From: Ada <ada@example.org>\r\nSubject: Hi\r\n\r\nHello.\r\n".to_vec();
    let (mut harness, ids) = window(&[plain], true);
    open(&mut harness, ids[0]);
    assert!(harness.state().hit.rect("digitalSignature", ids[0]).is_none());
    assert!(harness.state().hit.rect("messageSignature", ids[0]).is_none());
}
