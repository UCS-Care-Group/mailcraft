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

/// (e) A second inline part under the snapshot's Content-ID, with other bytes: the snapshot can't
/// say which of the two is the signature's picture, so the draft opens as any draft does, with no
/// signature, and the other part is still one of the message's files.
#[test]
fn a_second_part_under_the_snapshots_content_id_keeps_the_signature_out_and_the_part_in() {
    let (mut s, saved) = saved_and_closed();
    let m = s.mailbox.message_mut(saved).expect("the saved draft");
    let cid = m.draft_signature.as_ref().expect("the snapshot").content_ids().first().cloned().expect("one Content-ID for the one picture");
    let mut other =
        m.attachments.iter().find(|a| a.inline && a.content_id == cid).cloned().expect("fixture guard: the part under the snapshot's Content-ID");
    let mut bytes = other.data.to_vec();
    let last = bytes.last_mut().expect("the picture has bytes");
    *last ^= 0xff;
    other.data = Arc::new(bytes.clone());
    other.name = "other-mark.png".into();
    m.attachments.push(other);
    assert_eq!(reopen(&mut s, saved), None, "two parts under one Content-ID: the draft gets no signature back");
    let c = s.compose.as_ref().expect("the draft is being written again");
    assert!(
        c.attachments.iter().any(|a| a.data.as_slice() == bytes.as_slice()),
        "the other part was dropped: {:?}",
        c.attachments.iter().map(|a| (&a.name, &a.content_id)).collect::<Vec<_>>()
    );
}

/// (f) The owner's words and the signature together are more than a message holds: the draft
/// opens as any draft does, with no signature, rather than with the signature cut short.
#[test]
fn a_draft_too_long_to_hold_its_signature_too_opens_without_it() {
    let (mut s, saved) = saved_and_closed();
    let m = s.mailbox.message_mut(saved).expect("the saved draft");
    let html = m.body.html.clone().expect("an HTML draft");
    let at = html.find("Thanks,").expect("fixture guard: the owner's words in the HTML");
    let filler = "a".repeat(mailcraft_model::rich::MAX_CHARS - 20);
    m.body.html = Some(format!("{}{filler}{}", &html[..at], &html[at..]));
    let answer = run(&mut s, "mail.editDraft", json!({"id": saved}));
    assert_eq!(answer["draft"].as_u64(), Some(saved), "the saved draft opened");
    let c = s.compose.as_ref().expect("the draft is being written again");
    assert!(c.body.text().text().contains("Thanks,"), "the owner's words are back");
    assert_eq!(c.signature.as_ref().map(|i| i.text.text()), None, "a signature cut short came back as the signature");
}

#[test]
fn an_unchanged_active_wrapper_restores_its_signature() {
    let (mut s, id) = saved_and_closed();
    assert!(reopen(&mut s, id).is_some());
}
#[test]
fn a_wrapper_only_inside_a_comment_is_not_restored() {
    let (mut s, id) = saved_and_closed();
    let m = s.mailbox.message_mut(id).unwrap();
    let rendered = m.draft_signature.as_ref().unwrap().rendered().to_string();
    let h = m.body.html.clone().unwrap();
    m.body.html = Some(h.replace(&rendered, &format!("<!--{rendered}-->")));
    let parts = m.attachments.len();
    run(&mut s, "mail.editDraft", json!({"id":id}));
    let c = s.compose.as_ref().unwrap();
    println!(
        "COMMENT reopen signature={} text={:?} attachments={} original_parts={}",
        c.signature.is_some(),
        c.body.text().text(),
        c.attachments.len(),
        parts
    );
    assert!(c.signature.is_none(), "comment resurrected signature");
    assert_eq!(c.attachments.len(), parts);
}
#[test]
fn a_wrapper_only_inside_raw_text_is_not_restored() {
    let (mut s, id) = saved_and_closed();
    let m = s.mailbox.message_mut(id).unwrap();
    let rendered = m.draft_signature.as_ref().unwrap().rendered().to_string();
    let h = m.body.html.clone().unwrap();
    m.body.html = Some(h.replace(&rendered, &format!("<script>{rendered}</script>")));
    run(&mut s, "mail.editDraft", json!({"id":id}));
    let c = s.compose.as_ref().unwrap();
    println!("RAWTEXT reopen signature={} text={:?}", c.signature.is_some(), c.body.text().text());
    assert!(c.signature.is_none(), "raw text resurrected signature");
}

#[test]
fn a_wrapper_inside_an_inert_element_or_an_attribute_value_is_not_restored() {
    let mut refused = 0;
    for (before, after) in [
        ("<textarea>", "</textarea>"),
        ("<xmp>", "</xmp>"),
        ("<title>", "</title>"),
        ("<template>", "</template>"),
        ("<svg>", "</svg>"),
        ("<div title='", "'>other</div>"),
    ] {
        let (mut s, id) = saved_and_closed();
        let m = s.mailbox.message_mut(id).unwrap();
        let rendered = m.draft_signature.as_ref().unwrap().rendered().to_string();
        m.body.html = m.body.html.as_ref().map(|h| h.replace(&rendered, &format!("{before}{rendered}{after}")));
        let parts = m.attachments.clone();
        run(&mut s, "mail.editDraft", json!({"id": id}));
        let c = s.compose.as_ref().unwrap();
        if c.signature.is_none() {
            refused += 1;
        }
        assert!(c.signature.is_none(), "restored inert fragment under {before}");
        assert_eq!(c.attachments, parts, "fallback reclaimed a part under {before}");
    }
    assert_eq!(refused, 6);
}

#[test]
fn a_legacy_unwrapped_snapshot_that_begins_with_an_element_still_restores() {
    let (mut s, id) = saved_and_closed();
    let m = s.mailbox.message_mut(id).unwrap();
    let old = m.draft_signature.as_ref().unwrap();
    let rendered = old.rendered();
    assert!(rendered.starts_with("<div class=\"mc-sig-"));
    let start = rendered.find('>').unwrap() + 1;
    let legacy = rendered.get(start..rendered.len() - 6).unwrap().to_string();
    let snapshot =
        mailcraft_model::draft_signature::DraftSignature::new(old.signature().clone(), legacy.clone(), old.content_ids().to_vec()).unwrap();
    m.body.html = m.body.html.as_ref().map(|h| h.replace(rendered, &legacy));
    m.draft_signature = Some(snapshot);
    assert!(reopen(&mut s, id).is_some(), "legacy unwrapped snapshot was lost");
    assert!(s.compose.as_ref().unwrap().attachments.is_empty());
}

#[test]
fn an_incomplete_snapshot_fragment_is_not_restored() {
    let (mut s, id) = saved_and_closed();
    let m = s.mailbox.message_mut(id).unwrap();
    let old = m.draft_signature.as_ref().unwrap().clone();
    let incomplete = old.rendered().strip_suffix("</div>").unwrap().to_string();
    m.body.html = m.body.html.as_ref().map(|h| h.replace(old.rendered(), &incomplete));
    m.draft_signature = mailcraft_model::draft_signature::DraftSignature::new(old.signature().clone(), incomplete, old.content_ids().to_vec());
    let parts = m.attachments.clone();
    run(&mut s, "mail.editDraft", json!({"id": id}));
    let c = s.compose.as_ref().unwrap();
    assert!(c.signature.is_none(), "an incomplete element became the signature");
    assert_eq!(c.attachments, parts, "uncertain placement reclaimed inline parts");
}

#[test]
fn the_simpler_copy_with_its_writers_line_ends_restores() {
    let (mut s, account, _) = imported();
    run(
        &mut s,
        "signature.import",
        json!({"name": "Fallback", "html": "<style>@font-face{font-family:Example}</style><div>Northwind Studio card</div><div>Priya Raman - Northwind Atelier</div>", "assets": []}),
    );
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Fallback"}));
    run(&mut s, "mail.new", json!({"account": account, "body": "Thanks,"}));
    let id = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().unwrap();
    let m = s.mailbox.message(id).unwrap();
    println!("FALLBACK HTML {:?} RENDERED {:?}", m.body.html, m.draft_signature.as_ref().unwrap().rendered());
    run(&mut s, "compose.close", json!({}));
    assert!(reopen(&mut s, id).is_some(), "the writer's fallback lost its signature snapshot");
}

#[test]
fn white_space_round_the_fragment_is_not_an_inert_placement() {
    let (mut s, id) = saved_and_closed();
    let m = s.mailbox.message_mut(id).unwrap();
    let old = m.draft_signature.as_ref().unwrap().clone();
    let with_space = format!("\r\n{}\r\n", old.rendered());
    m.body.html = m.body.html.as_ref().map(|h| h.replace(old.rendered(), &with_space));
    m.draft_signature = mailcraft_model::draft_signature::DraftSignature::new(old.signature().clone(), with_space, old.content_ids().to_vec());
    assert!(reopen(&mut s, id).is_some(), "boundary whitespace lost a complete active fragment");
}

/// The draft with the snapshot's fragment written between `before` and `after`, opened again:
/// whether it got its signature back, its inline parts as they were, and its parts now.
fn reopened_between(before: &str, after: &str) -> (bool, Vec<mailcraft_model::Attachment>, Vec<mailcraft_model::Attachment>) {
    let (mut s, id) = saved_and_closed();
    let m = s.mailbox.message_mut(id).expect("the saved draft");
    let rendered = m.draft_signature.as_ref().expect("the snapshot").rendered().to_string();
    let html = m.body.html.clone().expect("an HTML draft");
    assert_eq!(html.matches(&rendered).count(), 1, "fixture guard: the fragment is in the draft once");
    m.body.html = Some(html.replacen(&rendered, &format!("{before}{rendered}{after}"), 1));
    let parts = m.attachments.clone();
    run(&mut s, "mail.editDraft", json!({"id": id}));
    let c = s.compose.as_ref().expect("the draft is being written again");
    (c.signature.is_some(), parts, c.attachments.clone())
}

/// A quote opens an attribute's value only after its `=`, as the HTML reader has it: in a name, an
/// unquoted value or an end tag, and after a `<` that starts no tag, it is an ordinary character.
/// So it can't hide from the locator the comment, raw text or template that holds the fragment:
/// each draft opens with no signature and with every inline part kept.
#[test]
fn a_stray_quote_outside_an_attribute_value_hides_no_inert_placement() {
    let mut refused = 0;
    for (before, after) in [
        ("<p x'> <!-- '>", " -->"),
        ("<p title=a'b> <!-- '>", " -->"),
        ("<p></p '> <!-- '>", " -->"),
        ("<p x'> <textarea '>", "</textarea>"),
        ("<p x'> <template '>", "</template>"),
        ("5 <3 '<!-- '>", "-->"),
    ] {
        let (restored, parts, now) = reopened_between(before, after);
        assert!(!restored, "restored a fragment hidden by {before:?}");
        assert_eq!(now, parts, "the fallback under {before:?} reclaimed a part");
        refused += 1;
    }
    assert_eq!(refused, 6);
}

/// Where a quote does open a value (after `=` and any white space), the value is read to its
/// closing quote, `>` and all; a quote in a name, an unquoted value or an end tag before the
/// fragment opens nothing. The active fragment after each restores, its picture taken back.
#[test]
fn a_quote_opens_a_value_only_after_an_equals_sign() {
    for before in ["<p title=\"a>b\" x'>", "<p data-x = 'a>b'>", "<p x\">", "<p title=a'b>", "<span></span '>"] {
        let (restored, parts, now) = reopened_between(before, "");
        assert!(restored, "a harmless quote in {before:?} lost the signature");
        assert!(now.len() < parts.len(), "the signature's picture was not taken back after {before:?}");
    }
}
