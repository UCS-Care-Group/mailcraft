// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The backstop for an HTML signature whose style sheet can't be written into a message safely:
//! one stored before the signature save gate refused such sheets (here put straight into the
//! mailbox, past that gate), or one imported with a sheet the writer refuses (`@font-face`).
//! Sending it is refused with one plain sentence, and nothing changes: the message being written
//! and the mailbox stay as they were. The signature's styles are never silently stripped and sent
//! without them. A draft is always saved: its HTML holds the signature as its simpler copy (as if
//! it were edited as text), the owner's words kept and no sheet, and on this computer only it keeps
//! the signature as it went in, bound to that copy. Closed and opened again, the draft holds the
//! HTML signature again, so Send is still refused until the signature is inserted again or edited
//! as text; the copy that goes to a server carries neither the sheet nor what is kept here. A
//! control with a clean sheet saves, with its scoped rule in the document's head, and sends.
//!
//! The people, hosts and styles are invented. Nothing here reads a file, fetches or sends.

use mailcraft_engine::{Session, sample};
use mailcraft_model::Signature;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// What the owner is told when a stored signature's styles can't go out.
const REFUSAL: &str = "this signature's styles can't be sent safely: fix them in File › Signatures, then insert the signature again (Message › Include › Signature), or choose Edit Signature as Text";
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

/// The class the engine scopes a message's HTML signature to, as its documentation says: `mc-sig-`
/// and the 64-bit FNV-1a digest, in hex, of the Message-ID's local part.
fn scope_of(message_id: &str) -> String {
    let local = message_id.split('@').next().unwrap_or("");
    let digest = local.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3));
    format!("mc-sig-{digest:016x}")
}

/// Every `mc-sig-…` class an element of `html` has, in order.
fn scope_classes(html: &str) -> Vec<String> {
    html.split("class=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .flat_map(str::split_whitespace)
        .filter(|class| class.starts_with("mc-sig-"))
        .map(str::to_string)
        .collect()
}

/// A session with an HTML signature using `sheet`, put into the mailbox directly (no save gate),
/// as the account's default, and a new message to a recipient with that signature in it.
fn writing_with(sheet: &str) -> Session {
    writing_with_text(sheet, "Thanks,")
}

/// [`writing_with`], the owner's own text being `typed`.
fn writing_with_text(sheet: &str, typed: &str) -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;
    let mut sig = Signature::plain_text(NAME, "Northwind Studio card\nPriya Raman");
    let own = format!("<html><head>{sheet}</head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>");
    sig.set_html(Some(own.clone())).expect("the model keeps the HTML");
    s.mailbox.signatures.push(sig);
    run(&mut s, "signature.defaults", json!({"account": account, "new": NAME}));
    run(&mut s, "mail.new", json!({"account": account, "to": "ada@example.org", "subject": "Card", "body": typed}));
    let c = s.compose.as_ref().expect("a message is being written");
    assert_eq!(c.signature_html(), Some(own.as_str()), "fixture guard: the HTML signature went in unedited");
    s
}

/// The draft keeps the signature as it went in (`own`, its HTML), bound to the simpler copy its
/// HTML holds once, with nothing of a sheet; its copy for a server has the same HTML and none of
/// it.
fn assert_kept_bound_to_stand_in(draft: &mailcraft_model::Message, own: &str) {
    let html = draft.body.html.clone().expect("an HTML draft");
    let kept = draft.draft_signature.as_ref().expect("the draft keeps, on this computer, the signature as it went in");
    assert_eq!(kept.signature().html(), Some(own), "kept as it went in, its HTML whole");
    assert_eq!(html.matches(kept.rendered()).count(), 1, "bound to the simpler copy the draft's HTML holds once: {html}");
    assert!(!kept.rendered().to_ascii_lowercase().contains("<style"), "the copy it is bound to has no sheet: {}", kept.rendered());
    let server = draft.clone().for_server();
    assert!(server.draft_signature.is_none(), "nothing of it goes to a server");
    assert_eq!(server.body.html.as_deref(), Some(html.as_str()), "the server's copy is the simpler copy");
}

/// A draft saved with a signature using `sheet`, closed, and opened again with Open Draft: the
/// session, the draft's id and the signature's own HTML.
fn reopened(sheet: &str) -> (Session, u64, String) {
    let mut s = writing_with(sheet);
    let own = s.compose.as_ref().and_then(|c| c.signature_html()).map(str::to_string).expect("fixture guard: an HTML signature");
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    run(&mut s, "compose.close", json!({}));
    assert!(s.compose.is_none(), "fixture guard: the draft was closed");
    run(&mut s, "mail.editDraft", json!({"id": saved}));
    assert_eq!(s.compose.as_ref().and_then(|c| c.signature_html()), Some(own.as_str()), "opened again, the draft holds the HTML signature");
    (s, saved, own)
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
/// the signature's words are kept, and nothing of the refused sheet goes with it. The draft keeps
/// the signature as it went in, bound to that copy, and its copy for a server keeps nothing of
/// it. Sending stays refused, and the message being written is as it was.
fn assert_draft_saved_with_stand_in(mut s: Session, typed: &str, words: &[&str], sheet_marks: &[&str]) {
    let own = s.compose.as_ref().and_then(|c| c.signature_html()).map(str::to_string).expect("fixture guard: an HTML signature");
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
    assert_kept_bound_to_stand_in(&draft, &own);
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

/// The refusal's remedies let the message being written go: the message keeps the signature as it
/// went in, so fixing it in File › Signatures alone leaves the message refused; inserting it again
/// then puts the fixed one in, and Edit Signature as Text sends its simpler copy.
#[test]
fn the_refusals_remedies_let_the_message_being_written_be_sent() {
    let mut s = writing_with(IMPORTING);
    run(&mut s, "signature.open", json!({"name": NAME}));
    let fixed = format!("<html><head>{CLEAN}</head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>");
    run(&mut s, "signature.html", json!({"html": fixed}));
    run(&mut s, "signature.save", json!({}));
    run(&mut s, "signature.close", json!({}));
    let err = s.run("compose.send", &json!({})).expect_err("the message still holds the signature as it went in");
    assert_eq!(err.to_string(), REFUSAL, "fixing the signature alone does not let the message go");
    run(&mut s, "compose.signature", json!({"name": NAME}));
    assert_eq!(s.compose.as_ref().and_then(|c| c.signature_html()), Some(fixed.as_str()), "the fixed signature is in the message");
    let sent = run(&mut s, "compose.send", json!({}));
    assert!(sent["id"].as_u64().is_some(), "sent with the fixed signature inserted again: {sent}");

    let mut s = writing_with(IMPORTING);
    run(&mut s, "compose.signatureAsText", json!({"value": true}));
    let sent = run(&mut s, "compose.send", json!({}));
    assert!(sent["id"].as_u64().is_some(), "sent with the signature edited as text: {sent}");
}

/// D1: a draft saved while its signature's styles can't be sent, closed and opened again, is still
/// refused at Send with the same words, and nothing changes: neither the message being written nor
/// the mailbox.
#[test]
fn a_reopened_draft_whose_signature_styles_cant_be_sent_is_still_refused() {
    for sheet in [IMPORTING, FETCHING] {
        let (mut s, _, _) = reopened(sheet);
        let (compose, mailbox) = (s.compose.clone(), s.mailbox.clone());
        let err = s.run("compose.send", &json!({})).expect_err("the reopened draft holds the signature as it went in");
        assert_eq!(err.to_string(), REFUSAL, "the refusal says what to do");
        assert_eq!(s.compose, compose, "the message being written is as it was");
        assert!(s.mailbox == mailbox, "the mailbox is as it was");
    }
}

/// The refusal's first remedy works on a reopened draft: once the signature is fixed in File ›
/// Signatures and inserted again, the message goes, with the fixed signature's sheet.
#[test]
fn a_reopened_draft_sends_once_the_fixed_signature_is_inserted_again() {
    let (mut s, _, _) = reopened(IMPORTING);
    run(&mut s, "signature.open", json!({"name": NAME}));
    let fixed = format!("<html><head>{CLEAN}</head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>");
    run(&mut s, "signature.html", json!({"html": fixed}));
    run(&mut s, "signature.save", json!({}));
    run(&mut s, "signature.close", json!({}));
    let err = s.run("compose.send", &json!({})).expect_err("the draft still holds the signature as it went in");
    assert_eq!(err.to_string(), REFUSAL, "fixing the signature alone does not let the draft go");
    run(&mut s, "compose.signature", json!({"name": NAME}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().expect("sent with the fixed signature inserted again");
    let m = s.mailbox.message(sent).expect("the message sent is kept").clone();
    let html = m.body.html.clone().expect("sent as HTML");
    assert!(html.contains("font-weight: bold") && !html.contains("@import"), "the fixed signature's sheet went: {html}");
    assert!(m.clone().for_server().draft_signature.is_none(), "nothing kept here goes with it");
}

/// The refusal's second remedy works on a reopened draft: edited as text, it goes as the simpler
/// copy, with nothing of the refused sheet.
#[test]
fn a_reopened_draft_sends_once_its_signature_is_edited_as_text() {
    let (mut s, _, _) = reopened(IMPORTING);
    run(&mut s, "compose.signatureAsText", json!({"value": true}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().expect("sent with the signature edited as text");
    let m = s.mailbox.message(sent).expect("the message sent is kept").clone();
    let html = m.body.html.clone().expect("sent as HTML");
    assert!(html.contains("Northwind Studio card") && html.contains("Priya Raman"), "the simpler copy went: {html}");
    assert!(!html.to_ascii_lowercase().contains("<style") && !html.contains("styles.example.net"), "nothing of the refused sheet: {html}");
    assert!(m.draft_signature.is_none(), "edited as text, nothing of the HTML signature is kept with it");
}

/// Saved again after it was opened again, the draft still holds the simpler copy for the server
/// and keeps the signature as it went in here, bound to that copy; opened once more, Send is
/// still refused.
#[test]
fn a_reopened_draft_saved_again_keeps_the_simpler_copy_and_the_refusal() {
    let (mut s, saved, own) = reopened(FETCHING);
    let again = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    assert_eq!(again, saved, "saved over the same draft");
    let draft = s.mailbox.message(again).expect("the draft is kept").clone();
    let html = draft.body.html.clone().expect("an HTML draft");
    assert!(!html.contains("url(") && !html.contains("track.example.net"), "nothing of the refused sheet: {html}");
    assert_kept_bound_to_stand_in(&draft, &own);
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": again}));
    let err = s.run("compose.send", &json!({})).expect_err("still the HTML signature");
    assert_eq!(err.to_string(), REFUSAL);
}

/// D1 on this computer: the mailbox file written and read again keeps what the draft keeps, so
/// the draft opened from it is still refused at Send, and nothing changes.
#[test]
fn a_draft_read_back_from_the_mailbox_file_is_still_refused() {
    let mut s = writing_with(IMPORTING);
    let own = s.compose.as_ref().and_then(|c| c.signature_html()).map(str::to_string).expect("fixture guard: an HTML signature");
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    run(&mut s, "compose.close", json!({}));
    let bytes = mailcraft_engine::io::write_mailbox(&s.mailbox).expect("the mailbox file is written");
    let back = mailcraft_engine::io::read_mailbox(&bytes).expect("the mailbox file is read back");
    let kept = |m: &mailcraft_model::Mailbox| m.message(saved).and_then(|d| d.draft_signature.clone());
    assert!(kept(&s.mailbox).is_some(), "fixture guard: the draft keeps the signature as it went in");
    assert_eq!(kept(&back), kept(&s.mailbox), "the file keeps it as it was");
    let mut t = Session::at(back, s.now(), 0);
    run(&mut t, "mail.editDraft", json!({"id": saved}));
    assert_eq!(t.compose.as_ref().and_then(|c| c.signature_html()), Some(own.as_str()), "opened again, the draft holds the HTML signature");
    let (compose, mailbox) = (t.compose.clone(), t.mailbox.clone());
    let err = t.run("compose.send", &json!({})).expect_err("still the HTML signature");
    assert_eq!(err.to_string(), REFUSAL, "the refusal says what to do");
    assert_eq!(t.compose, compose, "the message being written is as it was");
    assert!(t.mailbox == mailbox, "the mailbox is as it was");
}

/// D1 holds on this computer only: the server's copy of the draft keeps nothing of the signature
/// as it went in, so a draft replaced by it opens with the simpler copy as its words, and sends
/// that copy, with nothing of the refused sheet.
#[test]
fn a_draft_replaced_by_the_servers_copy_opens_with_the_simpler_copy_and_sends_it() {
    let mut s = writing_with(FETCHING);
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    run(&mut s, "compose.close", json!({}));
    let server = s.mailbox.message(saved).cloned().expect("the saved draft").for_server();
    assert!(server.draft_signature.is_none(), "fixture guard: the server's copy keeps nothing of it");
    *s.mailbox.message_mut(saved).expect("the saved draft") = server;
    run(&mut s, "mail.editDraft", json!({"id": saved}));
    let c = s.compose.as_ref().expect("the draft is being written again");
    assert_eq!(c.signature_html(), None, "no snapshot, no HTML signature");
    let words = c.body.text().to_plain();
    assert!(words.contains("Thanks,") && words.contains("Priya Raman"), "the owner's words and the simpler copy's are its text: {words:?}");
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().expect("sent with the simpler copy");
    let html = s.mailbox.message(sent).and_then(|m| m.body.html.clone()).expect("sent as HTML");
    assert!(html.contains("Priya Raman"), "the simpler copy went: {html}");
    assert!(!html.to_ascii_lowercase().contains("<style") && !html.contains("track.example.net"), "nothing of the refused sheet: {html}");
}

/// CodeRabbit (compose.rs:1662): the owner's text says what the signature's simpler copy says, the
/// same two lines. The draft's HTML still holds the copy its signature is bound to once, so opened
/// again the draft holds the HTML signature and Send is still refused, nothing changing; edited as
/// text it goes, the owner's words and the signature's, with nothing that marked the signature's
/// place in the draft.
#[test]
fn a_reopened_draft_whose_text_repeats_the_signatures_words_is_still_refused() {
    let mut s = writing_with_text(IMPORTING, "Northwind Studio card\nPriya Raman");
    let own = s.compose.as_ref().and_then(|c| c.signature_html()).map(str::to_string).expect("fixture guard: an HTML signature");
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    let draft = s.mailbox.message(saved).expect("the draft is kept").clone();
    let html = draft.body.html.clone().expect("an HTML draft");
    assert_eq!(html.matches("<div>Priya Raman</div>").count(), 2, "fixture guard: the owner's text and the simpler copy say the same: {html}");
    assert_kept_bound_to_stand_in(&draft, &own);
    run(&mut s, "compose.close", json!({}));
    run(&mut s, "mail.editDraft", json!({"id": saved}));
    let c = s.compose.as_ref().expect("the draft is being written again");
    assert_eq!(c.signature_html(), Some(own.as_str()), "opened again, the draft holds the HTML signature");
    assert_eq!(c.body.text().to_plain().matches("Priya Raman").count(), 2, "the owner's words and the signature's: {:?}", c.body.text().to_plain());
    let (compose, mailbox) = (s.compose.clone(), s.mailbox.clone());
    let err = s.run("compose.send", &json!({})).expect_err("the reopened draft holds the signature as it went in");
    assert_eq!(err.to_string(), REFUSAL, "the refusal says what to do");
    assert_eq!(s.compose, compose, "the message being written is as it was");
    assert!(s.mailbox == mailbox, "the mailbox is as it was");
    run(&mut s, "compose.signatureAsText", json!({"value": true}));
    let sent = run(&mut s, "compose.send", json!({}))["id"].as_u64().expect("sent with the signature edited as text");
    let html = s.mailbox.message(sent).and_then(|m| m.body.html.clone()).expect("sent as HTML");
    assert_eq!(html.matches("Priya Raman").count(), 2, "the owner's words and the signature's went: {html}");
    assert!(
        !html.contains("mc-sig-") && !html.to_ascii_lowercase().contains("<style"),
        "nothing of the draft's binding or the refused sheet: {html}"
    );
}

/// The binding is still refused when the draft's HTML holds the copy its signature is bound to
/// twice, above the signature or below it where a quoted original would be: which one is the
/// signature can't be told, so the draft opens with no HTML signature, its words as text.
#[test]
fn a_draft_whose_html_holds_the_signatures_copy_twice_opens_without_it() {
    for place in ["above", "below"] {
        let mut s = writing_with(IMPORTING);
        let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
        run(&mut s, "compose.close", json!({}));
        let m = s.mailbox.message_mut(saved).expect("the saved draft");
        let rendered = m.draft_signature.as_ref().expect("the snapshot").rendered().to_string();
        let html = m.body.html.clone().expect("an HTML draft");
        let changed = match place {
            "above" => html.replacen(&rendered, &format!("{rendered}{rendered}"), 1),
            _ => html.replacen("</body>", &format!("{rendered}</body>"), 1),
        };
        assert_eq!(changed.matches(&rendered).count(), 2, "fixture guard ({place}): the copy twice: {changed}");
        m.body.html = Some(changed);
        run(&mut s, "mail.editDraft", json!({"id": saved}));
        let c = s.compose.as_ref().expect("the draft is being written again");
        assert_eq!(c.signature_html(), None, "{place}: no HTML signature when its copy is there twice");
        assert!(c.signature.is_none(), "{place}: no signature at all");
        assert!(c.body.text().to_plain().contains("Priya Raman"), "{place}: its words are text: {:?}", c.body.text().to_plain());
    }
}

/// What the reply to CodeRabbit (compose.rs:1662) says: the `<div>` the simpler copy is put in has
/// the class an HTML signature of the same message is scoped to (`mc-sig-` and the digest of the
/// Message-ID's local part) and no other, so the copy of the draft a server gets carries nothing
/// that a sendable HTML signature of that message doesn't carry already.
#[test]
fn the_simpler_copys_wrapper_has_the_class_of_the_messages_html_signature() {
    let mut s = writing_with(IMPORTING);
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    let draft = s.mailbox.message(saved).expect("the draft is kept").clone();
    let scope = scope_of(&draft.message_id);
    let kept = draft.draft_signature.as_ref().expect("the draft keeps the signature as it went in");
    assert!(kept.rendered().starts_with(&format!("<div class=\"{scope}\">")), "the copy is in the message's scope class: {}", kept.rendered());
    let html = draft.body.html.clone().expect("an HTML draft");
    assert_eq!(scope_classes(&html), std::slice::from_ref(&scope), "the draft's HTML has that class once, and no other: {html}");
    // The same message with a signature whose styles can be sent: it goes in that same class.
    let mut clean = Signature::plain_text("Clean", "Northwind Studio card\nPriya Raman");
    let own = format!("<html><head>{CLEAN}</head><body><div class=\"studio-card\">Northwind Studio card</div><div>Priya Raman</div></body></html>");
    clean.set_html(Some(own)).expect("the model keeps the HTML");
    s.mailbox.signatures.push(clean);
    run(&mut s, "compose.signature", json!({"name": "Clean"}));
    let again = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    assert_eq!(again, saved, "fixture guard: the same draft");
    let m = s.mailbox.message(saved).expect("the draft is kept");
    assert_eq!(m.message_id, draft.message_id, "fixture guard: the same message");
    let html = m.body.html.clone().expect("an HTML draft");
    let classes = scope_classes(&html);
    assert!(
        !classes.is_empty() && classes.iter().all(|c| *c == scope),
        "a sendable signature of the message is in the same class: {classes:?} {html}"
    );
    assert!(html.contains(&format!(".{scope} ")), "and its sheet is scoped to that class: {html}");
}
