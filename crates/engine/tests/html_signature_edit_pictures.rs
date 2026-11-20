// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An imported HTML signature that owns pictures can be edited in the HTML editor
//! (`signature.html`), fixed and saved:
//!
//! * the checker's own one-click fix of an imported picture sized by style alone (OL-IMG-005) is
//!   applied through `signature.html`, after which `signature.save` succeeds and the stored
//!   signature keeps its picture, its bytes and its internal address in the HTML;
//! * an edit that takes the picture's reference out lets the picture go when saved, while the
//!   exact undo of that edit before saving (the HTML editor sends its previous text again, through
//!   the same command) brings it back, bound as before;
//! * HTML that names an internal address the signature does not own is refused, and nothing
//!   changes;
//! * converting a rich signature with pictures into HTML stays refused.
//!
//! The people and the picture are invented; nothing here reads a file, fetches or sends.

use mailcraft_engine::cmd::base64_encode;
use mailcraft_engine::{CmdError, Session, sample};
use mailcraft_html::compat::{self, Rule, Severity};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

/// A one-pixel PNG, whole and valid: the fixture the other signature import tests use.
const ONE_PIXEL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// The owner's file: its words and one local picture sized by its inline style alone (OL-IMG-005).
const IMPORTED: &str = r#"<p>Ana Pereira - Lisbon Bindery</p><p><img src="bindery-mark.png" alt="Bindery mark" style="width:120px;height:80px"></p>"#;
const NAME: &str = "Bindery";
/// The refusal of HTML naming a picture by an address the signature doesn't own.
const FOREIGN_PICTURE: &str = "this HTML names a picture this signature doesn't have";
/// The words the signature's text keeps.
const WORDS: &str = "Ana Pereira - Lisbon Bindery";

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 11, 17).unwrap(), 10, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The refusal of a command that must be refused for its own reason, as English text.
fn refusal(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Ok(answer) => panic!("{id} {params} was not refused: it answered {answer}"),
        Err(e @ (CmdError::Unknown(_) | CmdError::Params(_) | CmdError::Disabled(_))) => panic!("{id} {params} was refused for another reason: {e}"),
        Err(e) => e.to_string(),
    }
}

/// A session with [`IMPORTED`] imported (its picture bound) and opened in the editor; the stored
/// HTML (with the picture's internal address) and that address.
fn imported_and_open() -> (Session, String, String) {
    let mut s = session();
    run(
        &mut s,
        "signature.import",
        json!({"name": NAME, "html": IMPORTED, "assets": [{"name": "bindery-mark.png", "data": base64_encode(ONE_PIXEL_PNG)}]}),
    );
    let sig = s.mailbox.signature(NAME).expect("the imported signature");
    assert_eq!(sig.pictures().len(), 1, "fixture guard: one picture bound");
    assert_eq!(sig.pictures()[0].data.as_slice(), ONE_PIXEL_PNG, "fixture guard: its bytes");
    let html = sig.html().expect("fixture guard: an HTML signature").to_string();
    let cid = mailcraft_engine::mime::rich::internal_cid(&sig.pictures()[0]);
    assert!(html.contains(&format!("cid:{cid}")), "fixture guard: the HTML names the picture by its internal address: {html}");
    run(&mut s, "signature.open", json!({"name": NAME}));
    (s, html, cid)
}

/// The words of the editor's text and the pictures its text holds.
fn editor_text(s: &Session) -> (String, Vec<u32>) {
    let e = s.signature_edit.as_ref().expect("the signature is open");
    (e.body.text().to_plain(), e.body.text().pictures())
}

#[test]
fn the_checkers_fix_of_an_imported_picture_applies_and_the_signature_saves_with_its_picture() {
    let (mut s, html, cid) = imported_and_open();
    // The checker's one error and its exact fix, applied to the exact checked text.
    let found = compat::check(&html).expect("the checker reads it");
    let errors: Vec<_> = found.iter().filter(|f| f.severity == Severity::Error).collect();
    assert_eq!(errors.len(), 1, "fixture guard: one error: {found:?}");
    assert_eq!(errors[0].rule, Rule::CssOnlyImageSize, "fixture guard: OL-IMG-005");
    let fix = errors[0].fix.clone().expect("fixture guard: the finding has a fix");
    let mut fixed = html.clone();
    fixed.replace_range(fix.range.clone(), &fix.replacement);
    assert!(compat::check(&fixed).expect("checked").iter().all(|f| f.severity != Severity::Error), "fixture guard: the fix clears it: {fixed}");

    run(&mut s, "signature.html", json!({"html": fixed}));
    let (words, pictures) = editor_text(&s);
    assert!(words.contains(WORDS), "the editor's text keeps the words: {words}");
    assert_eq!(pictures.len(), 1, "the editor's text holds the picture: {pictures:?}");
    run(&mut s, "signature.save", json!({}));

    let sig = s.mailbox.signature(NAME).expect("the saved signature");
    assert_eq!(sig.html(), Some(fixed.as_str()), "the fixed HTML is kept as given");
    assert_eq!(sig.pictures().len(), 1, "the picture is kept");
    assert_eq!(sig.pictures()[0].data.as_slice(), ONE_PIXEL_PNG, "with its bytes");
    assert_eq!(mailcraft_engine::mime::rich::internal_cid(&sig.pictures()[0]), cid, "under the address the HTML names");
    assert_eq!(sig.body().pictures(), vec![sig.pictures()[0].id], "the stand-in text holds the picture where it stands");
    assert!(sig.body().to_plain().contains(WORDS), "and the words");
}

#[test]
fn an_edit_without_the_picture_lets_it_go_on_save_and_its_exact_undo_brings_it_back() {
    // Undo before saving: the HTML editor sends its previous text again.
    let (mut s, html, cid) = imported_and_open();
    let without = html.replace(&format!("<img src=\"cid:{cid}\" alt=\"Bindery mark\" style=\"width:120px;height:80px\">"), "");
    assert!(!without.contains(&cid) && without.contains(WORDS), "fixture guard: the edit takes out only the picture: {without}");
    run(&mut s, "signature.html", json!({"html": without}));
    assert_eq!(editor_text(&s).1, Vec::<u32>::new(), "the edited text holds no picture");
    run(&mut s, "signature.html", json!({"html": html}));
    assert_eq!(editor_text(&s).1.len(), 1, "the undo brings the picture back into the text");
    let e = s.signature_edit.as_ref().expect("open");
    assert_eq!(e.signature.html(), Some(html.as_str()), "and the HTML as it was");
    // Saved after the undo (the error stays, so the import's own HTML is fixed first).
    let fixed = html.replacen("<img ", "<img width=\"120\" height=\"80\" ", 1);
    run(&mut s, "signature.html", json!({"html": fixed}));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature(NAME).expect("saved");
    assert_eq!(sig.pictures().len(), 1, "the picture survives the edit and its undo");
    assert_eq!(sig.pictures()[0].data.as_slice(), ONE_PIXEL_PNG, "with its bytes");
    assert_eq!(mailcraft_engine::mime::rich::internal_cid(&sig.pictures()[0]), cid, "under its address");

    // Saved without the picture: it is let go.
    let (mut s, html, cid) = imported_and_open();
    let without = html.replace(&format!("<img src=\"cid:{cid}\" alt=\"Bindery mark\" style=\"width:120px;height:80px\">"), "");
    run(&mut s, "signature.html", json!({"html": without}));
    run(&mut s, "signature.save", json!({}));
    let sig = s.mailbox.signature(NAME).expect("saved");
    assert_eq!(sig.html(), Some(without.as_str()), "the HTML as edited");
    assert!(sig.pictures().is_empty(), "the picture it no longer names is let go: {:?}", sig.pictures().len());
    assert!(sig.body().to_plain().contains(WORDS), "the words stay");
}

#[test]
fn html_naming_an_internal_address_the_signature_does_not_own_is_refused() {
    let (mut s, html, cid) = imported_and_open();
    let before = (s.signature_edit.clone(), s.mailbox.signature(NAME).cloned());
    let foreign = html.replace(&cid, "sig-99@signature.invalid");
    assert_ne!(foreign, html, "fixture guard: the address changed");
    let why = refusal(&mut s, "signature.html", json!({"html": foreign}));
    assert_eq!(why, FOREIGN_PICTURE, "the refusal says why");
    assert_eq!((s.signature_edit.clone(), s.mailbox.signature(NAME).cloned()), before, "nothing changed");
}

#[test]
fn a_rich_signature_with_pictures_is_still_not_converted_to_html() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Rich", "text": "Ana Pereira"}));
    run(&mut s, "signature.picture.add", json!({"name": "Rich", "file": "mark.png", "data": base64_encode(ONE_PIXEL_PNG)}));
    assert_eq!(s.mailbox.signature("Rich").expect("rich").pictures().len(), 1, "fixture guard: a rich signature with a picture");
    let why = refusal(&mut s, "signature.html", json!({"name": "Rich", "html": "<p>Ana Pereira</p>"}));
    assert_eq!(why, "this HTML conversion would lose the signature's embedded pictures");
}
