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
const OPAQUE_FILES: &[u8] = include_bytes!("../../../smime/testdata/opaque-files.eml");

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
    // The signature part isn't among the attachments: Details reaches it.
    let p7s = harness.state().session.mailbox.messages.get(&ids[0]).unwrap().attachments.iter().position(|a| a.name == "smime.p7s").unwrap();
    assert!(harness.state().hit.rect("attachment", crate::reading::attachment_key(ids[0], p7s)).is_none());
    assert!(harness.state().hit.rect("signatureFile", ids[0]).is_some());
}

/// The system's authorities come when the host has read them; the window never waits, and what
/// was checked without them is checked again (#30 review 6).
#[test]
fn the_authorities_are_taken_when_they_come() {
    let (mut harness, ids) = window(&[DETACHED.to_vec()], false);
    let (tx, rx) = std::sync::mpsc::channel();
    harness.state_mut().trust_anchors_arriving = Some(rx);
    open(&mut harness, ids[0]);
    assert!(says(&harness, "certification authorities haven't been read yet"));
    tx.send(vec![CA.to_vec()]).unwrap();
    harness.run_steps(4);
    assert_eq!(harness.state().session.trust_anchor_count(), 1);
    assert!(harness.state().trust_anchors_arriving.is_none());
    assert!(says(&harness, "Digitally signed by alice@smime.example, and verified."));
}

/// An opaque signed message shows what was signed under its bar, files and all: its `smime.p7m`
/// is the signature file, not in the strip, reached from Details.
#[test]
fn an_opaque_signed_message_shows_its_content() {
    let (mut harness, ids) = window(&[OPAQUE_FILES.to_vec()], true);
    open(&mut harness, ids[0]);
    assert!(says(&harness, "Digitally signed by alice@smime.example, and verified."));
    assert!(says(&harness, "minutes.pdf"), "the file inside, in the strip");
    let names: Vec<String> = harness.state().session.mailbox.messages.get(&ids[0]).unwrap().attachments.iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, ["smime.p7m", "minutes.pdf"]);
    assert!(harness.state().hit.rect("attachment", crate::reading::attachment_key(ids[0], 0)).is_none(), "the signature");
    assert!(harness.state().hit.rect("attachment", crate::reading::attachment_key(ids[0], 1)).is_some(), "the file inside");
    harness.get_by_label("Details").click();
    harness.run_steps(4);
    assert!(harness.state().hit.rect("signatureFile", ids[0]).is_some());
}

/// Only the part that was checked leaves the strip: a file the sender named `smime.p7s` beside a
/// signed message is a file like any other (#30 review 2: 2).
#[test]
fn a_file_merely_named_like_a_signature_stays_in_the_strip() {
    let detached = String::from_utf8_lossy(DETACHED).into_owned();
    let (head, body) = detached.split_once("\r\n\r\n").unwrap();
    let wrapped = format!(
        "From: alice@smime.example\r\nTo: bob@smime.example\r\nSubject: Notes\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\n{head}\r\n\r\n{body}\r\n--outer\r\nContent-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\nmy own notes\r\n--outer--\r\n"
    );
    let (mut harness, ids) = window(&[wrapped.into_bytes()], true);
    open(&mut harness, ids[0]);
    assert!(says(&harness, "Part of this message is Digitally signed"));
    let names: Vec<String> = harness.state().session.mailbox.messages.get(&ids[0]).unwrap().attachments.iter().map(|a| a.name.clone()).collect();
    assert_eq!(names, ["smime.p7s", "smime.p7s"]);
    assert!(harness.state().hit.rect("attachment", crate::reading::attachment_key(ids[0], 0)).is_none(), "the signature");
    assert!(harness.state().hit.rect("attachment", crate::reading::attachment_key(ids[0], 1)).is_some(), "the sender's own file");
}

#[test]
fn a_signer_not_verified_a_change_and_an_old_algorithm_are_said() {
    let changed: Vec<u8> = String::from_utf8_lossy(DETACHED).replace("Tuesday", "Thursday").into_bytes();
    let (mut harness, ids) = window(&[DETACHED.to_vec(), changed, SHA1.to_vec()], false);
    open(&mut harness, ids[0]);
    assert!(says(&harness, "but the signer isn't verified."));
    assert!(says(&harness, "certification authorities haven't been read yet"));
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
