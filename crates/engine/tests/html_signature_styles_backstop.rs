// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The backstop for an HTML signature whose style sheet can't be written into a message safely:
//! one stored before the signature save gate refused such sheets (here put straight into the
//! mailbox, past that gate), or one imported with a sheet the writer refuses (`@font-face`).
//! Sending it is refused with one plain sentence, and nothing changes: the message being written
//! and the mailbox stay as they were. The signature's styles are never silently stripped and sent
//! without them. A draft is always saved: with the signature as its simpler copy (as if it were
//! edited as text), the owner's words kept, no sheet and no snapshot of the HTML signature. A
//! control with a clean sheet saves, with its scoped rule in the document's head, and sends.
//!
//! The people, hosts and styles are invented. Nothing here reads a file, fetches or sends.

use mailcraft_engine::{Session, sample};
use mailcraft_model::Signature;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// What the owner is told when a stored signature's styles can't go out.
const REFUSAL: &str = "this signature's styles can't be sent safely: fix them in File › Signatures";
const NAME: &str = "Studio";
/// A sheet that fetches another (an at-rule the writer refuses).
const IMPORTING: &str = "<style>@import \"https://styles.example.net/card.css\";\n.studio-card { color: #1f4e79; }</style>";
/// A sheet whose declaration fetches a picture.
const FETCHING: &str = "<style>.studio-card { color: #1f4e79; background: url(https://track.example.net/p.png); }</style>";
/// A sheet with nothing that runs or fetches.
const CLEAN: &str = "<style>.studio-card { color: #1f4e79; font-weight: bold; }</style>";

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A session with an HTML signature using `sheet`, put into the mailbox directly (no save gate),
/// as the account's default, and a new message to a recipient with that signature in it.
fn writing_with(sheet: &str) -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;
    let mut sig = Signature::plain_text(NAME, "Northwind Studio card\nPriya Raman");
    let own = format!("<html><head>{sheet}</head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>");
    sig.set_html(Some(own.clone())).expect("the model keeps the HTML");
    s.mailbox.signatures.push(sig);
    run(&mut s, "signature.defaults", json!({"account": account, "new": NAME}));
    run(&mut s, "mail.new", json!({"account": account, "to": "ada@example.org", "subject": "Card", "body": "Thanks,"}));
    let c = s.compose.as_ref().expect("a message is being written");
    assert_eq!(c.signature_html(), Some(own.as_str()), "fixture guard: the HTML signature went in unedited");
    s
}

/// `id` is refused with [`REFUSAL`], and neither the message being written nor the mailbox changed.
fn assert_refused(sheet: &str, id: &str) {
    let mut s = writing_with(sheet);
    let (compose, composing, mailbox) = (s.compose.clone(), s.composing.clone(), s.mailbox.clone());
    let err = s.run(id, &json!({})).expect_err("a signature whose styles can't be sent safely is not saved or sent");
    assert_eq!(err.to_string(), REFUSAL, "{id}: the refusal says what to do");
    assert_eq!(s.compose, compose, "{id}: the message being written is as it was");
    assert_eq!(s.composing, composing, "{id}: the other messages being written are as they were");
    assert!(s.mailbox == mailbox, "{id}: the mailbox is as it was");
}

/// The draft saved with the signature's simpler copy in place of its HTML: the owner's words and
/// the signature's words are kept, and nothing of the refused sheet, nor a snapshot of the HTML
/// signature, goes with it. Sending stays refused, and the message being written is as it was.
fn assert_draft_saved_with_stand_in(mut s: Session, typed: &str, words: &[&str], sheet_marks: &[&str]) {
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    let draft = s.mailbox.message(saved).expect("the draft is kept").clone();
    assert!(draft.draft, "kept as a draft");
    assert!(draft.body.text.contains(typed), "the owner's words are in the draft: {}", draft.body.text);
    let html = draft.body.html.clone().expect("an HTML draft");
    assert!(html.contains(typed), "the owner's words are in its HTML: {html}");
    for w in words {
        assert!(draft.body.text.contains(w), "the signature's words {w:?} are in the draft's text: {}", draft.body.text);
        assert!(html.contains(w), "the signature's words {w:?} are in its HTML: {html}");
    }
    assert!(!html.to_ascii_lowercase().contains("<style"), "no sheet goes with the draft: {html}");
    for mark in sheet_marks {
        assert!(!html.contains(mark), "nothing of the refused sheet ({mark}): {html}");
    }
    assert!(draft.draft_signature.is_none(), "no snapshot of the HTML signature is kept with the draft");
    let compose = s.compose.clone();
    let err = s.run("compose.send", &json!({})).expect_err("sending is still refused");
    assert_eq!(err.to_string(), REFUSAL, "the refusal says what to do");
    assert_eq!(s.compose, compose, "the message being written is as it was");
}

#[test]
fn a_draft_with_a_signature_sheet_that_imports_is_saved_with_the_signatures_simpler_copy() {
    assert_draft_saved_with_stand_in(
        writing_with(IMPORTING),
        "Thanks,",
        &["Northwind Studio card", "Priya Raman"],
        &["@import", "styles.example.net"],
    );
}

#[test]
fn sending_with_a_signature_sheet_that_imports_is_refused_and_nothing_changes() {
    assert_refused(IMPORTING, "compose.send");
}

#[test]
fn a_draft_with_a_signature_sheet_that_fetches_a_picture_is_saved_with_the_signatures_simpler_copy() {
    assert_draft_saved_with_stand_in(writing_with(FETCHING), "Thanks,", &["Northwind Studio card", "Priya Raman"], &["url(", "track.example.net"]);
}

/// The review's case: a signature imported with a sheet the writer refuses (`@font-face`, which
/// generated signatures carry) is made the default; a new message's draft is still saved, with
/// the words typed into it and the signature's text, and sending it is refused.
#[test]
fn an_imported_signature_with_a_font_face_still_lets_a_draft_be_saved() {
    let now = DateTime::new(Date::new(2026, 11, 17).unwrap(), 10, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;
    let html = "<html><head><style>@font-face { font-family: Probe; } p.c { color: #123456; }</style></head><body><p class=\"c\">Ana Pereira</p><p>Lisbon Bindery</p></body></html>";
    run(&mut s, "signature.import", json!({"name": "Generated", "html": html, "assets": []}));
    let found = mailcraft_html::compat::check(html).expect("checked");
    let ids: Vec<&str> = found.iter().map(|f| f.rule.id()).collect();
    assert_eq!(ids, ["MC-CSS-001"], "fixture guard: the writer refuses the sheet");
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Generated"}));
    run(&mut s, "mail.new", json!({"account": account, "to": "ada@example.org", "subject": "Card"}));
    run(&mut s, "compose.type", json!({"text": "Long text the owner typed"}));
    let c = s.compose.as_ref().expect("a message is being written");
    assert!(c.signature_html().is_some(), "fixture guard: the HTML signature is in it, unedited by the typing");
    assert_draft_saved_with_stand_in(s, "Long text the owner typed", &["Ana Pereira", "Lisbon Bindery"], &["@font-face", "Probe"]);
}

#[test]
fn sending_with_a_signature_sheet_that_fetches_a_picture_is_refused_and_nothing_changes() {
    assert_refused(FETCHING, "compose.send");
}

/// Control: a clean sheet saves, its rule scoped in the head and its words in the scoped body, and
/// the message sends.
#[test]
fn a_signature_with_a_clean_sheet_saves_and_sends() {
    let mut s = writing_with(CLEAN);
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    let html = s.mailbox.message(saved).and_then(|m| m.body.html.clone()).expect("an HTML draft");
    let (head, body) = html.split_once("</head>").expect("a document with a head");
    assert!(head.contains("<style>") && head.contains("{ color: #1f4e79; font-weight: bold; }"), "the rule goes in the head: {html}");
    assert!(!body.contains("<style"), "no sheet in the body: {html}");
    assert!(body.contains("Northwind Studio card"), "the signature's words are in the body: {html}");
    let sent = run(&mut s, "compose.send", json!({}));
    assert!(sent["id"].as_u64().is_some(), "sent: {sent}");
    assert!(s.compose.is_none(), "nothing is being written any more");
}
