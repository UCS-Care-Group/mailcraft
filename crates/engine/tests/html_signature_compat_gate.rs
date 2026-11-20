// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An HTML signature is not saved while the email-compatibility checker
//! (`mailcraft_html::compat::check`) finds an error in it, or cannot check it: `signature.save`
//! of the open signature, and `signature.html` written straight to a signature that is not open,
//! are refused with the two exact English texts the window matches on, and the refusal leaves
//! the editor (its identity, its text, its unsaved state) and the mailbox exactly as they were.
//! Importing such a document stays allowed, so that it can be fixed in the editor; saving it is
//! what waits for the fix. A signature with no HTML is not checked at all.
//!
//! Every rule of the checker's first part is an error, so a document whose only findings are
//! warnings or notes cannot be written yet: the clean document stands for "nothing that blocks".
//!
//! The people, hosts and picture are invented; nothing here reads a file, fetches a picture,
//! opens a dialog or sends anything.

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

/// The refusal for a signature with errors, as the window matches on it: the same words for any
/// count, which the window gives in the owner's language itself.
const ERRORS: &str = "this signature has email compatibility errors to fix before it can be saved";
/// The refusal for HTML the checker could not check.
const UNCHECKED: &str = "this signature's HTML could not be checked for email compatibility";

/// A picture sized by its inline style alone, with no `width` attribute (OL-IMG-005, an error).
const CSS_SIZED: &str =
    r#"<p>Ana Pereira - Lisbon Bindery</p><p><img src="https://pictures.example/mark.png" alt="Bindery mark" style="width:120px"></p>"#;
/// [`CSS_SIZED`] with the checker's one fix applied, written out by hand: the attribute right
/// after the tag's name, the style kept.
const CSS_SIZED_FIXED: &str =
    r#"<p>Ana Pereira - Lisbon Bindery</p><p><img width="120" src="https://pictures.example/mark.png" alt="Bindery mark" style="width:120px"></p>"#;
/// Two such pictures: two errors.
const TWO_CSS_SIZED: &str = r#"<p>Ana Pereira</p><img src="https://pictures.example/a.png" alt="A" style="width:40px"><img src="https://pictures.example/b.png" alt="B" style="height:30px">"#;
/// A document the checker finds nothing in.
const CLEAN: &str =
    r#"<p>Ana Pereira - Lisbon Bindery</p><p><img src="https://pictures.example/mark.png" alt="Bindery mark" width="120" style="width:120px"></p>"#;
/// A document cut off inside a start tag: the checker refuses to check it.
const TRUNCATED: &str = r#"<p>Ana Pereira - Lisbon Bindery</p><p><img src="https://pictures.example/mark.png" width="120""#;
/// The owner's file for the import: its one picture, named by its file name, sized by style alone.
const IMPORTED: &str = r#"<p>Ana Pereira - Lisbon Bindery</p><p><img src="bindery-mark.png" alt="Bindery mark" style="width:120px;height:80px"></p>"#;

fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 11, 17).unwrap(), 10, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The refusal of a command that must be refused for its own reason (not an unknown command or
/// bad parameters), as English text.
fn refusal(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Ok(answer) => panic!("{id} {params} was not refused: it answered {answer}"),
        Err(e @ (CmdError::Unknown(_) | CmdError::Params(_) | CmdError::Disabled(_))) => panic!("{id} {params} was refused for another reason: {e}"),
        Err(e) => e.to_string(),
    }
}

/// The error findings the checker gives `html` (a fixture guard: what the test stands on).
fn errors(html: &str) -> Vec<compat::Finding> {
    compat::check(html).expect("the checker reads this document").into_iter().filter(|f| f.severity == Severity::Error).collect()
}

/// A rich signature `name`, opened in the editor and made the HTML signature `html` there, unsaved.
fn open_as_html(s: &mut Session, name: &str, html: &str) {
    run(s, "signature.set", json!({"name": name, "text": "Ana Pereira\nLisbon Bindery"}));
    run(s, "signature.open", json!({"name": name}));
    run(s, "signature.html", json!({"html": html}));
    let e = s.signature_edit.as_ref().expect("the signature is open");
    assert_eq!(e.signature.html(), Some(html), "the open signature is the HTML signature of this document");
    assert!(e.dirty, "the HTML is a change the editor has not saved");
    assert_eq!(s.mailbox.signature(name).expect("the signature is in the mailbox").html(), None, "the mailbox keeps the rich signature until a save");
}

/// A refused save of the open signature, judged on everything it might have changed.
fn assert_save_refused(s: &mut Session, said: &str) {
    let mailbox = s.mailbox.clone();
    let editor = s.signature_edit.clone().expect("a signature is open");
    assert_eq!(refusal(s, "signature.save", json!({})), said, "the refusal's exact text");
    assert!(s.mailbox == mailbox, "the refused save left the mailbox exactly as it was");
    assert_eq!(s.signature_edit.as_ref(), Some(&editor), "the refused save left the editor, its text and its unsaved state as they were");
    assert_eq!(
        s.signature_edit.as_ref().map(|e| e.signature.name.as_str()),
        Some(editor.signature.name.as_str()),
        "the same signature is open in the editor"
    );
}

#[test]
fn a_picture_sized_by_its_style_alone_keeps_the_signature_from_being_saved() {
    let found = errors(CSS_SIZED);
    assert_eq!(found.len(), 1, "one error in the document: {found:?}");
    assert_eq!(found.first().map(|f| f.rule), Some(Rule::CssOnlyImageSize), "the picture sized by its style alone: {found:?}");

    let mut s = session();
    open_as_html(&mut s, "Card", CSS_SIZED);
    let before = s.mailbox.signature("Card").cloned().expect("the signature");
    assert_save_refused(&mut s, ERRORS);
    assert_eq!(s.mailbox.signature("Card"), Some(&before), "the mailbox's signature is the rich one it was");
    let e = s.signature_edit.as_ref().expect("still open");
    assert!(e.dirty, "the editor still has its unsaved HTML");
    assert_eq!(e.signature.html(), Some(CSS_SIZED), "and that HTML is the owner's, unchanged");
}

#[test]
fn the_refusal_counts_every_error() {
    assert_eq!(errors(TWO_CSS_SIZED).len(), 2, "two errors in the document");
    let mut s = session();
    open_as_html(&mut s, "Card", TWO_CSS_SIZED);
    assert_save_refused(&mut s, ERRORS);
}

#[test]
fn with_the_checkers_fix_applied_the_signature_saves() {
    let mut s = session();
    open_as_html(&mut s, "Card", CSS_SIZED);
    assert_save_refused(&mut s, ERRORS);

    // The finding's own edit, applied as the window applies it, then given to the open signature.
    let finding = errors(CSS_SIZED).into_iter().next().expect("the error");
    let fix = finding.fix.expect("the checker offers the edit that puts it right");
    let mut fixed = CSS_SIZED.to_string();
    fixed.replace_range(fix.range.clone(), &fix.replacement);
    assert_eq!(fixed, CSS_SIZED_FIXED, "the fix adds the attribute and keeps the style");
    assert!(errors(&fixed).is_empty(), "nothing is left to fix");
    run(&mut s, "signature.html", json!({"html": fixed}));

    assert_eq!(run(&mut s, "signature.save", json!({})), json!({"name": "Card"}), "saved");
    assert_eq!(s.mailbox.signature("Card").and_then(|x| x.html()), Some(CSS_SIZED_FIXED), "the mailbox keeps the fixed HTML");
    assert!(!s.signature_edit.as_ref().expect("still open").dirty, "and the editor has nothing left unsaved");
}

#[test]
fn html_with_nothing_to_fix_saves() {
    assert_eq!(compat::check(CLEAN), Ok(Vec::new()), "the checker finds nothing in the document");
    let mut s = session();
    open_as_html(&mut s, "Card", CLEAN);
    assert_eq!(run(&mut s, "signature.save", json!({})), json!({"name": "Card"}), "saved");
    assert_eq!(s.mailbox.signature("Card").and_then(|x| x.html()), Some(CLEAN), "the mailbox keeps the HTML");
}

#[test]
fn a_signature_without_html_is_not_checked() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira"}));
    run(&mut s, "signature.open", json!({"name": "Card"}));
    run(&mut s, "compose.type", json!({"text": " - Lisbon Bindery"}));
    assert_eq!(run(&mut s, "signature.save", json!({})), json!({"name": "Card"}), "saved");
    let kept = s.mailbox.signature("Card").expect("the signature");
    assert_eq!(kept.html(), None, "a rich signature still");
    assert!(kept.plain().contains("Lisbon Bindery"), "with the typed words: {:?}", kept.plain());
}

#[test]
fn html_the_checker_cannot_read_is_refused_with_its_own_text() {
    assert!(compat::check(TRUNCATED).is_err(), "the checker refuses a document cut off inside a tag");
    let mut s = session();
    open_as_html(&mut s, "Card", TRUNCATED);
    assert_save_refused(&mut s, UNCHECKED);
}

#[test]
fn html_written_straight_to_a_signature_that_is_not_open_is_refused_the_same_way() {
    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira\nLisbon Bindery"}));
    run(&mut s, "signature.set", json!({"name": "Other", "text": "Ana Pereira"}));
    run(&mut s, "signature.open", json!({"name": "Other"}));
    run(&mut s, "compose.type", json!({"text": "Unsaved words for the other signature"}));
    let mailbox = s.mailbox.clone();
    let editor = s.signature_edit.clone().expect("the other signature is open");
    assert!(editor.dirty, "the open editor has unsaved words");

    for (html, said) in [(CSS_SIZED, ERRORS), (TWO_CSS_SIZED, ERRORS), (TRUNCATED, UNCHECKED)] {
        assert_eq!(refusal(&mut s, "signature.html", json!({"name": "Card", "html": html})), said, "the refusal's exact text for {html}");
        assert!(s.mailbox == mailbox, "the refused write left the mailbox exactly as it was ({html})");
        assert_eq!(s.signature_edit.as_ref(), Some(&editor), "and the open editor as it was ({html})");
    }

    // The same door with nothing to fix writes the mailbox's signature, and leaves the editor be.
    run(&mut s, "signature.html", json!({"name": "Card", "html": CLEAN}));
    assert_eq!(s.mailbox.signature("Card").and_then(|x| x.html()), Some(CLEAN), "written straight to the mailbox");
    assert_eq!(s.signature_edit.as_ref(), Some(&editor), "the open editor is as it was");
}

#[test]
fn an_import_with_an_error_is_kept_and_saving_it_unchanged_is_refused() {
    let mut s = session();
    let answer = run(
        &mut s,
        "signature.import",
        json!({"name": "Imported", "html": IMPORTED, "assets": [{"name": "bindery-mark.png", "data": base64_encode(ONE_PIXEL_PNG)}]}),
    );
    assert_eq!(answer, json!({"name": "Imported"}), "the import is allowed");
    let stored = s.mailbox.signature("Imported").cloned().expect("the import put the signature in the mailbox");
    let html = stored.html().expect("an HTML signature");
    let found = errors(html);
    assert_eq!(found.len(), 1, "the stored document still has its one error: {found:?} in {html}");
    assert_eq!(found.first().map(|f| f.rule.id()), Some("OL-IMG-005"), "the picture sized by its style alone");

    run(&mut s, "signature.open", json!({"name": "Imported"}));
    assert!(!s.signature_edit.as_ref().expect("open").dirty, "opened, unchanged");
    assert_save_refused(&mut s, ERRORS);
    assert_eq!(s.mailbox.signature("Imported"), Some(&stored), "the imported signature is as the import made it");
    assert!(!s.signature_edit.as_ref().expect("still open").dirty, "and the editor is still unchanged");
}

/// HTML longer than a signature holds, written straight to a signature that is not open, is
/// refused for its length before it is checked: the checker never reads what can't be kept. The
/// document would have an error to fix too, which is not what is said.
#[test]
fn html_too_long_for_a_signature_is_refused_for_its_length_before_it_is_checked() {
    let mut html = CSS_SIZED.to_string();
    html.push_str(&"x".repeat(mailcraft_model::signature::MAX_HTML + 1 - html.len()));
    assert!(!errors(&html).is_empty(), "fixture guard: the checker finds an error in it");

    let mut s = session();
    run(&mut s, "signature.set", json!({"name": "Card", "text": "Ana Pereira\nLisbon Bindery"}));
    assert!(s.signature_edit.is_none(), "fixture guard: no signature is open");
    let mailbox = s.mailbox.clone();
    assert_eq!(refusal(&mut s, "signature.html", json!({"name": "Card", "html": html})), "an HTML signature is at most 64 KB");
    assert!(s.mailbox == mailbox, "the refusal left the mailbox exactly as it was");
}
