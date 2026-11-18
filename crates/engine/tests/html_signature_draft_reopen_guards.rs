// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Guards on the HTML signature a reopened draft gets back from its local snapshot
//! (`Message::draft_signature`), through the commands the app runs: the snapshot is believed
//! only while the draft still holds the fragment it names and the inline parts under its
//! Content-IDs are the signature's own pictures, and a draft whose signature was edited, or that
//! had no HTML signature, keeps none.
//!
//! The people, hosts and picture are invented: the picture is the synthetic one-pixel PNG the
//! MIME tests use, and says nothing of pixels. Nothing here reads a file, fetches or sends.

use std::sync::Arc;

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::{Session, sample};
use mailcraft_model::Signature;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// A one-pixel PNG, whole and valid: the same fixture the MIME integration tests use.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

const PICTURE_FILE: &str = "studio-mark.png";
const NAME: &str = "Northwind Studio";
const SHEET: &str = "<style>.studio-card { color: #1f4e79; font-weight: bold; padding: 13px; }</style>";
const BODY: &str = r#"<table cellpadding="0" cellspacing="0" border="0" style="background-color:#fdf6e3"><tr>
  <td style="padding-right:16px"><img src="studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></td>
  <td><div class="studio-card">Northwind Studio card</div><div>Priya Raman - Northwind Atelier</div><div><a href="mailto:hello@example.org">hello@example.org</a></div></td>
</tr></table>
<div><img src="./studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></div>"#;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A session with the document imported by `signature.import`, and that signature as stored.
fn imported() -> (Session, u64, Signature) {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;
    let own = format!("<html><head>{SHEET}</head><body>{BODY}</body></html>");
    let answer =
        run(&mut s, "signature.import", json!({"name": NAME, "html": own, "assets": [{"name": PICTURE_FILE, "data": base64_encode(ONE_PIXEL_PNG)}]}));
    assert_eq!(answer, json!({"name": NAME}));
    let stored = s.mailbox.signature(NAME).expect("the import put the signature in the mailbox").clone();
    assert_eq!(stored.pictures().len(), 1, "one payload for both places: {:?}", stored.pictures());
    assert!(stored.html().is_some(), "an HTML signature");
    (s, account, stored)
}

/// The imported signature as the account's default, put in a new message saved as a draft, which
/// is then closed. Fixture guard: the draft keeps a snapshot of the signature it was saved with.
fn saved_and_closed() -> (Session, u64) {
    let (mut s, account, stored) = imported();
    run(&mut s, "signature.defaults", json!({"account": account, "new": NAME}));
    run(&mut s, "mail.new", json!({"account": account, "body": "Thanks,"}));
    assert_eq!(s.compose.as_ref().and_then(|c| c.signature_html()), stored.html(), "the HTML signature went in unedited");
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    let draft = s.mailbox.message(saved).expect("the saved draft");
    let snapshot = draft.draft_signature.as_ref().expect("fixture guard: the saved draft keeps a snapshot of its HTML signature");
    assert_eq!(snapshot.signature().html(), stored.html(), "fixture guard: the snapshot is of the signature that went in");
    assert!(draft.body.html.as_deref().is_some_and(|h| h.contains(snapshot.rendered())), "fixture guard: the draft holds the snapshot's fragment");
    run(&mut s, "compose.close", json!({}));
    assert!(s.compose.is_none(), "the draft is closed");
    (s, saved)
}

/// The draft opened again: the signature the reopened message knows, if any.
fn reopen(s: &mut Session, saved: u64) -> Option<mailcraft_engine::InsertedSignature> {
    let answer = run(s, "mail.editDraft", json!({"id": saved}));
    assert_eq!(answer["draft"].as_u64(), Some(saved), "the saved draft opened");
    let c = s.compose.as_ref().expect("the draft is being written again");
    let words = c.body.text().text();
    assert!(words.contains("Thanks,") && words.contains("Priya Raman - Northwind Atelier"), "its words are back: {words:?}");
    c.signature.clone()
}

/// (a) Reopened with its signature, the signature edited (as text, in its own paragraphs) and
/// saved: the old snapshot is cleared, and the draft opened again knows no signature.
#[test]
fn a_reopened_draft_whose_signature_is_edited_and_saved_keeps_no_snapshot() {
    let (mut s, saved) = saved_and_closed();
    assert!(reopen(&mut s, saved).is_some(), "precondition: the unchanged draft reopens with its signature");
    run(&mut s, "compose.signatureAsText", json!({"value": true}));
    let start = s.compose.as_ref().and_then(|c| c.body.text().signature_start()).expect("the signature is in the text");
    run(&mut s, "compose.select", json!({"from": start + 1}));
    run(&mut s, "compose.type", json!({"text": "Edited "}));
    let c = s.compose.as_ref().expect("still being written");
    assert!(!c.signature_unedited(), "the signature's own text was edited: {:?}", c.body.text().text());
    assert_eq!(c.signature_html(), None, "the edited signature no longer goes out as HTML");
    run(&mut s, "compose.saveDraft", json!({}));
    let again = s.mailbox.message(saved).expect("saved again in place");
    assert!(again.draft_signature.is_none(), "an edited signature clears the old snapshot: {:?}", again.draft_signature);
    run(&mut s, "compose.close", json!({}));
    assert_eq!(reopen(&mut s, saved), None, "opened again, the edited draft knows no signature");
}

/// (b) The stored draft's HTML no longer holds the snapshot's fragment: no signature.
#[test]
fn a_draft_whose_html_no_longer_holds_the_fragment_reopens_without_a_signature() {
    let (mut s, saved) = saved_and_closed();
    let m = s.mailbox.message_mut(saved).expect("the saved draft");
    let rendered = m.draft_signature.as_ref().expect("the snapshot").rendered().to_string();
    let html = m.body.html.clone().expect("an HTML draft");
    let changed = html.replace("Northwind Studio card", "Northwind Studio cart");
    assert!(!changed.contains(&rendered), "fixture guard: the fragment is gone from the changed HTML");
    assert!(changed.contains("Priya Raman - Northwind Atelier"), "fixture guard: the rest of the words stay");
    m.body.html = Some(changed);
    assert_eq!(reopen(&mut s, saved), None, "a draft no longer holding the fragment gets no signature back");
}

/// (c) The inline part under the snapshot's Content-ID no longer holds the picture's bytes: no
/// signature.
#[test]
fn a_draft_whose_inline_part_no_longer_matches_reopens_without_a_signature() {
    let (mut s, saved) = saved_and_closed();
    let m = s.mailbox.message_mut(saved).expect("the saved draft");
    let cid = m.draft_signature.as_ref().expect("the snapshot").content_ids().first().cloned().expect("one Content-ID for the one picture");
    let part = m.attachments.iter_mut().find(|a| a.inline && a.content_id == cid).expect("fixture guard: the part under the snapshot's Content-ID");
    let mut bytes = part.data.to_vec();
    let last = bytes.last_mut().expect("the picture has bytes");
    *last ^= 0xff;
    assert_ne!(bytes.as_slice(), ONE_PIXEL_PNG, "fixture guard: other bytes");
    part.data = Arc::new(bytes);
    assert_eq!(reopen(&mut s, saved), None, "a draft whose part doesn't match the picture gets no signature back");
}

/// (d) A draft saved with a signature that is not HTML keeps no snapshot.
#[test]
fn a_draft_saved_without_an_html_signature_keeps_no_snapshot() {
    let (mut s, account, _) = imported();
    run(&mut s, "signature.set", json!({"name": "Front Desk", "text": "Priya Raman\nFront desk"}));
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Front Desk"}));
    run(&mut s, "mail.new", json!({"account": account, "body": "Thanks,"}));
    let c = s.compose.as_ref().expect("a message is being written");
    assert_eq!(c.signature.as_ref().map(|i| i.name.as_str()), Some("Front Desk"), "precondition: the plain signature went in");
    assert_eq!(c.signature_html(), None, "precondition: it is no HTML signature");
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    let draft = s.mailbox.message(saved).expect("the saved draft");
    assert!(draft.draft_signature.is_none(), "no HTML signature, no snapshot: {:?}", draft.draft_signature);
}
