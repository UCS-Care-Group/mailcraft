// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An imported HTML signature with a style sheet of its own, followed through the commands the
//! app runs, as three separate tracers so that one failure hides none of the others:
//!
//! * an unrelated edit in the signature editor, undone exactly, then saved, closed and opened
//!   again, and the mailbox written and read back: the signature is the one imported (a baseline);
//! * the signature put in a new message and saved as a draft: its pictures are bound, and the
//!   presentation its style sheet's class rule gives the marked words is what the draft shows;
//! * that draft closed, the account's default changed, and the draft opened again: the message
//!   keeps the HTML signature it was saved with, not merely its words.
//!
//! The people, hosts, style and picture are invented: the picture is the synthetic one-pixel PNG
//! the MIME tests use, and says nothing of pixels. Nothing here reads a file, fetches or sends.

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::{Session, sample};
use mailcraft_html::{Block, BlockKind, Rgb, Span};
use mailcraft_mime_model::WriteOptions;
use mailcraft_mime_model::rich::{ImgRef, from_html_with_pictures, internal_cid};
use mailcraft_mime_model::signature_import::required_assets;
use mailcraft_model::signature::Picture;
use mailcraft_model::{Message, Signature};
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
/// The words the class rule styles, and what that rule gives them.
const MARKED: &str = "Northwind Studio card";
const CARD_COLOR: Rgb = Rgb(0x1f, 0x4e, 0x79);
/// The owner's style sheet: one safe class rule, nothing that runs or fetches.
const SHEET: &str = "<style>.studio-card { color: #1f4e79; font-weight: bold; padding: 13px; }</style>";
/// The owner's body: a table with the picture and the marked words, and the same picture again
/// under another spelling of its file name.
const BODY: &str = r#"<table cellpadding="0" cellspacing="0" border="0" style="background-color:#fdf6e3"><tr>
  <td style="padding-right:16px"><img src="studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></td>
  <td><div class="studio-card">Northwind Studio card</div><div>Priya Raman - Northwind Atelier</div><div><a href="mailto:hello@example.org">hello@example.org</a></div></td>
</tr></table>
<div><img src="./studio-mark.png" alt="Northwind mark" width="120" height="80" style="border:0"></div>"#;

fn own_html() -> String {
    format!("<html><head>{SHEET}</head><body>{BODY}</body></html>")
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A session with the document imported by `signature.import`, and that signature as stored.
fn imported() -> (Session, u64, Signature) {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s.mailbox.default_account().expect("the sample mailbox has an account").id;
    let own = own_html();
    // Fixture guard: the planner names the one file, under both spellings of it.
    assert_eq!(required_assets(&own).expect("a document the importer reads"), [PICTURE_FILE], "one picture file, named twice");
    let answer =
        run(&mut s, "signature.import", json!({"name": NAME, "html": own, "assets": [{"name": PICTURE_FILE, "data": base64_encode(ONE_PIXEL_PNG)}]}));
    assert_eq!(answer, json!({"name": NAME}));
    let stored = s.mailbox.signature(NAME).expect("the import put the signature in the mailbox").clone();
    assert_eq!(stored.pictures().len(), 1, "one payload for both places: {:?}", stored.pictures());
    let picture = stored.pictures().first().expect("the picture just counted");
    assert_eq!(picture.data.as_slice(), ONE_PIXEL_PNG, "the picture's own bytes");
    assert_eq!(stored.html(), Some(stored_form(&own, picture).as_str()), "the document as written, only the picture addresses changed");
    (s, account, stored)
}

/// `own` with each picture value the tokenizer reads written as the picture's internal address,
/// at the byte ranges the real locator gives, every other byte kept.
fn stored_form(own: &str, picture: &Picture) -> String {
    let sources = mailcraft_html::image_sources(own).expect("the locator reads the fixture");
    assert_eq!(sources.len(), 2, "two <img> in the fixture");
    let mut out = String::new();
    let mut at = 0;
    for source in &sources {
        out.push_str(own.get(at..source.value_range.start).expect("the locator's ranges are in order"));
        out.push_str(&format!("cid:{}", internal_cid(picture)));
        at = source.value_range.end;
    }
    out.push_str(own.get(at..).expect("the last range is in the document"));
    out
}

fn image_cids(html: &str) -> Vec<String> {
    let mut cids = Vec::new();
    let _ = from_html_with_pictures(html, &mut |img: &ImgRef| {
        cids.push(img.cid.clone());
        Some(0)
    });
    cids
}

fn inline_pngs(m: &Message) -> Vec<(String, Vec<u8>)> {
    m.attachments.iter().filter(|a| a.inline && a.mime == "image/png").map(|a| (a.content_id.clone(), a.data.to_vec())).collect()
}

/// Both `<img>` of the signature name the one inline part of the message, with its own bytes, by
/// a Content-ID of this message; no inline part is unnamed.
fn assert_pictures_bound(label: &str, html: &str, parts: &[(String, Vec<u8>)]) {
    let cids = image_cids(html);
    assert_eq!(cids.len(), 2, "{label}: both places the picture is written: {cids:?} in {html}");
    assert_eq!(parts.len(), 1, "{label}: one inline part for the one picture: {parts:?}");
    for cid in &cids {
        assert!(!cid.contains("signature.invalid"), "{label}: the signature's own address `{cid}` left in: {html}");
        let part = parts.iter().find(|(id, _)| id == cid).unwrap_or_else(|| panic!("{label}: `{cid}` is no inline part: {parts:?}"));
        assert_eq!(part.1.as_slice(), ONE_PIXEL_PNG, "{label}: `{cid}` is not the picture's bytes");
    }
    for (id, _) in parts {
        assert!(cids.contains(id), "{label}: the inline part `{id}` is named by no picture: {html}");
    }
}

/// The span the display reader makes of `text`, looking inside tables.
fn span_of(blocks: &[Block], text: &str) -> Option<Span> {
    blocks.iter().find_map(|b| match &b.kind {
        BlockKind::Table(t) => t.rows.iter().flat_map(|r| &r.cells).find_map(|c| span_of(&c.blocks, text)),
        _ => b.spans.iter().find(|s| s.text.contains(text)).cloned(),
    })
}

/// A: an unrelated edit, its exact undo, save, close and open again, and the mailbox file written
/// and read: the imported HTML and its picture are kept. Expected to pass (baseline, not a RED).
#[test]
fn an_imported_html_signature_edited_then_undone_and_saved_is_kept_as_imported() {
    let (mut s, _, stored) = imported();
    run(&mut s, "signature.open", json!({"name": NAME}));
    let first = s.signature_edit.clone().expect("the signature is open in the editor");
    assert_eq!(first.signature, stored, "the editor opened the stored signature");
    assert_eq!(first.body.text(), stored.body(), "the editor's text is the stored stand-in");
    let own_words = first.body.text().text();
    assert!(own_words.contains("Priya Raman - Northwind Atelier"), "the signature's own words are in the editor: {own_words:?}");

    // A real edit through the editor's typing command: the text changes.
    let typed = "Unrelated words typed into the signature editor ";
    run(&mut s, "compose.type", json!({"text": typed}));
    let edited = s.signature_edit.clone().expect("still open");
    assert_eq!(edited.signature.name, first.signature.name, "the same signature in the editor");
    assert!(edited.dirty, "the edit is unsaved");
    assert_ne!(edited.body.text(), stored.body(), "the typing changed the text");
    let words = edited.body.text().text();
    assert!(words.contains(typed.trim_end()) && words.contains("Priya Raman - Northwind Atelier"), "typed and own words: {words:?}");

    // The editor's own undo (Undo Typing; `edit.undo` is the mailbox's) takes the whole edit back.
    run(&mut s, "compose.undo", json!({}));
    let undone = s.signature_edit.clone().expect("still open");
    assert_eq!(undone.body.text(), stored.body(), "the undo gave back the whole text as it was");
    assert!(!undone.body.text().text().contains(typed.trim_end()), "nothing typed is left");

    run(&mut s, "signature.save", json!({}));
    assert_eq!(s.mailbox.signature(NAME), Some(&stored), "saved after the exact undo: the whole signature as imported");
    run(&mut s, "signature.close", json!({}));
    assert!(s.signature_edit.is_none(), "closed");
    run(&mut s, "signature.open", json!({"name": NAME}));
    let again = s.signature_edit.clone().expect("opened again");
    assert!(!again.dirty, "a fresh editor, opened again after the close, with nothing unsaved");
    assert_eq!(again.signature, stored, "opened again: the whole signature as imported");
    assert_eq!(again.signature.html(), stored.html(), "its raw HTML");
    assert_eq!(again.signature.pictures(), stored.pictures(), "its pictures");

    // The mailbox as its file is written and read, in memory.
    let bytes = mailcraft_engine::io::write_mailbox(&s.mailbox).expect("the mailbox writes");
    let read = mailcraft_engine::io::read_mailbox(&bytes).expect("and reads back");
    assert_eq!(read.signature(NAME), Some(&stored), "the mailbox file keeps the whole signature as imported");
}

/// B: the signature in a new message saved as a draft. The pictures are bound (a guard already
/// known to work); then the presentation the style sheet's class rule gives the marked words
/// must be what the draft shows (the style sheet goes with it, scoped to the signature).
#[test]
fn a_saved_draft_keeps_the_presentation_an_imported_signatures_class_rule_gives() {
    // Fixture guard: the display reader really gives the marked words the rule's look, and only
    // because of the sheet (the same body without it gives them none).
    let own = own_html();
    let styled = span_of(&mailcraft_html::parse(&own), MARKED).expect("the display reader reads the marked words");
    assert!(styled.bold && styled.color == Some(CARD_COLOR), "the class rule styles the marked words: {styled:?}");
    let bare = format!("<html><head></head><body>{BODY}</body></html>");
    let unstyled = span_of(&mailcraft_html::parse(&bare), MARKED).expect("the same words without the sheet");
    assert!(!unstyled.bold && unstyled.color.is_none(), "without the sheet the words have no such look: {unstyled:?}");

    let (mut s, account, stored) = imported();
    run(&mut s, "signature.defaults", json!({"account": account, "new": NAME}));
    run(&mut s, "mail.new", json!({"account": account, "body": "Thanks,"}));
    let c = s.compose.as_ref().expect("a message is being written");
    assert_eq!(c.signature.as_ref().map(|i| i.name.as_str()), Some(NAME), "the account's signature went in");
    assert_eq!(c.signature_html(), stored.html(), "the HTML signature goes out unedited");
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");
    let draft = s.mailbox.message(saved).expect("the saved draft").clone();
    let html = draft.body.html.as_deref().expect("an HTML draft");
    assert!(html.contains(MARKED), "the marked words are in the draft: {html}");

    // First the binding, on the saved draft and on the wire.
    assert_pictures_bound("the saved draft", html, &inline_pngs(&draft));
    let parsed = mailcraft_mime_model::parse(&mailcraft_mime_model::write(&draft, &WriteOptions { include_bcc: true, ..Default::default() }));
    let wire = parsed.body.html.as_deref().expect("the written draft keeps its HTML");
    assert_pictures_bound("the written draft", wire, &inline_pngs(&parsed));
    assert!(wire.contains("class=\"studio-card\""), "the marked words keep their class: {wire}");

    // Then the presentation: what the display reader makes of the marked words in the message.
    for (label, html) in [("the saved draft", html), ("the written draft", wire)] {
        let shown = span_of(&mailcraft_html::parse(html), MARKED).unwrap_or_else(|| panic!("{label}: the marked words are read: {html}"));
        assert_eq!(
            (shown.bold, shown.color),
            (styled.bold, styled.color),
            "{label}: the marked words lose the look the signature's class rule gives them: {html}"
        );
    }
    // The rule itself, as a boundary on its source (its selector may be scoped later).
    let rule = "{ color: #1f4e79; font-weight: bold; padding: 13px; }";
    assert!(wire.contains(rule), "the class rule's declarations go with the message: {wire}");
}

/// C: the draft closed, the default changed, the draft opened again. The message must still own
/// the HTML signature it was saved with (from the draft's snapshot of it).
#[test]
fn a_reopened_draft_keeps_the_html_signature_it_was_saved_with() {
    let (mut s, account, stored) = imported();
    run(&mut s, "signature.defaults", json!({"account": account, "new": NAME}));
    run(&mut s, "mail.new", json!({"account": account, "body": "Thanks,"}));
    assert_eq!(s.compose.as_ref().and_then(|c| c.signature_html()), stored.html(), "the HTML signature went in unedited");
    let saved = run(&mut s, "compose.saveDraft", json!({}))["draft"].as_u64().expect("the saved draft's id");

    // The draft as saved binds its pictures (a guard already known to work).
    let draft = s.mailbox.message(saved).expect("the saved draft").clone();
    let parsed = mailcraft_mime_model::parse(&mailcraft_mime_model::write(&draft, &WriteOptions { include_bcc: true, ..Default::default() }));
    assert_pictures_bound("the written draft", parsed.body.html.as_deref().expect("its HTML"), &inline_pngs(&parsed));

    // Closed: nothing of it is open any more.
    assert!(!s.compose.as_ref().expect("still open").dirty, "saved, nothing unsaved");
    run(&mut s, "compose.close", json!({}));
    assert!(s.compose.is_none() && s.composing.iter().all(|c| c.draft != Some(saved)), "the draft is closed");
    assert!(s.mailbox.message(saved).is_some_and(|m| m.draft), "and kept in Drafts");

    // The account's default is another signature now: the draft's own must not follow it.
    run(&mut s, "signature.set", json!({"name": "Front Desk", "text": "Priya Raman\nFront desk"}));
    run(&mut s, "signature.defaults", json!({"account": account, "new": "Front Desk"}));

    let answer = run(&mut s, "mail.editDraft", json!({"id": saved}));
    assert_eq!(answer["draft"].as_u64(), Some(saved), "the saved draft opened");
    let c = s.compose.clone().expect("the draft is being written again");
    assert_eq!(c.draft, Some(saved), "the same stored draft");
    let words = c.body.text().text();
    assert!(words.contains("Thanks,") && words.contains("Priya Raman - Northwind Atelier"), "its words are back: {words:?}");
    assert!(!words.contains("Front desk"), "the new default did not go into it: {words:?}");

    // The snapshot the message keeps of its signature.
    let inserted = c.signature.as_ref().unwrap_or_else(|| panic!("the reopened draft keeps no signature snapshot at all"));
    assert_eq!(inserted.name, NAME, "the signature it was saved with");
    assert_eq!(inserted.html.as_deref(), stored.html(), "its own raw HTML");
    // (Not reached while the snapshot is lost: what a later edit and save need of it.)
    assert_eq!(inserted.pictures, stored.pictures(), "its pictures, with their bytes");
    assert_eq!(c.signature_html(), stored.html(), "it goes out as HTML again while unedited");
    run(&mut s, "compose.saveDraft", json!({}));
    let again = s.mailbox.message(saved).expect("saved again in place").clone();
    assert_pictures_bound("the draft saved again", again.body.html.as_deref().expect("its HTML"), &inline_pngs(&again));
}
