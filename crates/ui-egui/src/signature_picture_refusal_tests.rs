// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures with an HTML signature that names a picture it can't be sent with: a file's
//! name, or a `cid:` address the signature doesn't own. The email-compatibility checker finds
//! nothing in it, but `signature.save` refuses it (`mailcraft_engine::cmd::signature::
//! sendable_pictures`), so the window treats it as it treats a compatibility error: Save is
//! disabled and its hover text says why, and leaving the page asks whether to keep editing or to
//! discard the changes, rather than leaving the owner on a page they can't leave without
//! rewriting the HTML.
//!
//! One synthetic signature, opened in the page's editor as its HTML source, in one headless
//! window driven as the owner drives it. What is asserted is what the window drew: the Save
//! button and its hover text as the accessibility tree gives them, the question's words and the
//! places of its answers in the page's hit map.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The window every frame is laid out in, tall enough for the page's own controls.
const SIZE: (f32, f32) = (1280.0, 900.0);

/// The fixture's signature.
const NAME: &str = "Harbour Studio";

/// The owner's HTML: a picture named by a file's name, which no file is kept beside.
const LOCAL: &str = "<p>Priya Raman</p>\n<img width=\"120\" height=\"40\" src=\"logo.png\" alt=\"Harbour Studio\">\n<p>Studio Two</p>";
/// The same with a `cid:` address the signature, which owns no picture, doesn't have.
const FOREIGN: &str =
    "<p>Priya Raman</p>\n<img width=\"120\" height=\"40\" src=\"cid:logo@pictures.example\" alt=\"Harbour Studio\">\n<p>Studio Two</p>";
/// The same with a picture on the web, which can be sent.
const WEB: &str =
    "<p>Priya Raman</p>\n<img width=\"120\" height=\"40\" src=\"https://pictures.example/logo.png\" alt=\"Harbour Studio\">\n<p>Studio Two</p>";

/// The engine's refusals of [`LOCAL`] and [`FOREIGN`] (their contract's exact words), which Save's
/// hover text gives.
const LOCAL_REFUSAL: &str = "this HTML names a picture that is neither on the web nor one of this signature's own: give it a web address (https://…), or name the picture's file relative to the HTML file and bring both in with Import HTML…";
const FOREIGN_REFUSAL: &str = "this HTML names a picture this signature doesn't have";

/// The question asked when the page is left with [`LOCAL`], and its two answers.
const KEEP_EDITING: &str = "Keep editing";
const DISCARD: &str = "Discard changes";
fn cant_save(why: &str) -> String {
    format!("This signature can't be saved: {why}")
}

/// The app over the sample mailbox, nothing happening by itself.
fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

/// The app with the fixture's signature open in the page's editor as `html`, not yet saved, and
/// File › Signatures shown. Nothing in `html` is for the email-compatibility checker.
fn opened(html: &str) -> MailApp {
    assert!(!crate::compat::Checked::of(html).blocks_save(), "fixture guard: the checker finds an error in {html:?}");
    let mut a = app();
    a.execute("signature.set", json!({"name": NAME, "text": "Priya Raman", "default": true})).expect("the fixture makes its signature");
    a.execute("signature.open", json!({"name": NAME})).expect("the fixture opens it in the editor");
    a.execute("signature.html", json!({"html": html})).expect("the editor takes the HTML as typing is taken");
    let e = a.session.signature_edit.as_ref().expect("a signature is open in the editor");
    assert_eq!(e.signature.html(), Some(html), "the open signature does not hold the fixture's HTML");
    assert!(e.dirty, "the fixture's HTML is not an unsaved change");
    assert!(e.signature.pictures().is_empty(), "fixture guard: the signature owns no picture");
    a.execute("ui.backstage", json!({"value": true, "page": "signatures"})).expect("File shows the signatures page");
    a
}

/// The whole window, laid out a few frames: File › Signatures as `MailApp::ui` draws it.
fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(SIZE.0, SIZE.1)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

/// The pointer moved to `at`, and a few frames.
fn hover(h: &mut Harness<'static, MailApp>, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(3);
}

/// The owner's own press: the pointer moved there, the button down, and up.
fn click(h: &mut Harness<'static, MailApp>, at: egui::Pos2) {
    h.event(egui::Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

/// Where the owner's control `key` with `id` was drawn this frame, on screen.
fn hit_rect(h: &Harness<'static, MailApp>, key: &str, id: u64) -> Option<egui::Rect> {
    h.state().hit.rect(key, id)
}

fn on_screen(r: egui::Rect) -> bool {
    r.top() >= 0.0 && r.bottom() <= SIZE.1 && r.left() >= 0.0 && r.right() <= SIZE.0
}

/// The page's Save button for the open signature, where it is and whether it is disabled: the
/// one control the accessibility tree names Save below the source editor.
fn save_button(h: &Harness<'static, MailApp>) -> (egui::Rect, bool) {
    let editor = hit_rect(h, "signatureHtmlSource", 0).expect("the source editor is in the page's hit map");
    let mut below: Vec<(egui::Rect, bool)> = h
        .query_all_by_label("Save")
        .map(|n| (n.rect(), n.accesskit_node().is_disabled()))
        .filter(|(r, _)| r.top() >= editor.bottom() && r.left() < editor.right() && r.right() > editor.left())
        .collect();
    below.sort_by(|a, b| a.0.top().total_cmp(&b.0.top()));
    let first = below.first().copied().expect("no Save button below the source editor");
    assert!(on_screen(first.0), "Save is drawn off screen at {:?}", first.0);
    first
}

/// The owner leaves File, as its back arrow and Escape do: the window's own command, then frames.
fn leave_file(h: &mut Harness<'static, MailApp>) {
    h.state_mut().execute("ui.backstage", json!({"value": false})).expect("File closes");
    h.run_steps(4);
}

#[test]
fn save_is_disabled_for_a_picture_that_cant_be_sent_and_says_why() {
    let _language = crate::i18n::RestoreLanguage::hold();
    for (html, why) in [(LOCAL, LOCAL_REFUSAL), (FOREIGN, FOREIGN_REFUSAL)] {
        let mut h = window(opened(html));
        let mailbox = h.state().session.mailbox.clone();
        let editor = h.state().session.signature_edit.clone();

        let (save, disabled) = save_button(&h);
        assert!(disabled, "Save can be pressed while the signature names a picture it can't be sent with: {html:?}");

        // Its hover text says why, in the engine's words.
        hover(&mut h, save.center());
        h.run_steps(60);
        assert!(h.query_by_label(why).is_some(), "Save's hover text does not say {why:?}");

        // Pressing it keeps nothing and asks nothing.
        click(&mut h, save.center());
        assert!(h.state().dialog.is_none(), "pressing the disabled Save opened a dialog");
        assert!(h.state().session.mailbox == mailbox, "pressing the disabled Save changed the mailbox");
        assert_eq!(h.state().session.signature_edit, editor, "pressing the disabled Save changed the signature open in the editor");
    }
}

#[test]
fn a_picture_on_the_web_leaves_save_enabled() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(WEB));
    let (save, disabled) = save_button(&h);
    assert!(!disabled, "Save is disabled for a picture on the web");
    click(&mut h, save.center());
    assert_eq!(h.state().session.mailbox.signature(NAME).and_then(|s| s.html()), Some(WEB), "Save did not keep the HTML with its web picture");
}

#[test]
fn leaving_with_such_a_picture_asks_and_keep_editing_keeps_every_edit() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(LOCAL));
    let mailbox = h.state().session.mailbox.clone();
    let editor = h.state().session.signature_edit.clone();

    leave_file(&mut h);
    let question = cant_save(LOCAL_REFUSAL);
    assert!(h.query_by_label(&question).is_some(), "leaving the page asked nothing: {question:?}");
    assert!(h.query_by_label(DISCARD).is_some(), "the question offers no {DISCARD:?}");
    assert!(h.query_by_label(KEEP_EDITING).is_some(), "the question offers no {KEEP_EDITING:?}");
    let keep = hit_rect(&h, "questionCancel", 0).expect("the question's Keep editing is in the hit map");
    assert!(on_screen(keep), "Keep editing is drawn off screen at {keep:?}");
    click(&mut h, keep.center());

    assert!(h.state().dialog.is_none(), "the question stayed open after Keep editing");
    let s = &h.state().session;
    assert_eq!(s.signature_edit, editor, "Keep editing changed the signature open in the editor");
    assert!(s.mailbox == mailbox, "Keep editing changed the mailbox");
    let ui = &h.state().ui;
    assert!(ui.backstage && ui.backstage_page == "signatures", "Keep editing did not stay on File › Signatures");

    // Leaving again asks again, and changes nothing meanwhile.
    leave_file(&mut h);
    assert!(h.query_by_label(&question).is_some(), "leaving again did not ask again");
    assert_eq!(h.state().session.signature_edit, editor, "leaving again changed the signature open in the editor");
}

#[test]
fn leaving_with_such_a_picture_and_discarding_closes_the_signature_and_leaves() {
    let _language = crate::i18n::RestoreLanguage::hold();
    for (html, why) in [(LOCAL, LOCAL_REFUSAL), (FOREIGN, FOREIGN_REFUSAL)] {
        let mut h = window(opened(html));
        let mailbox = h.state().session.mailbox.clone();
        assert_eq!(mailbox.signature(NAME).and_then(|s| s.html()), None, "the fixture's signature is kept as HTML already");

        leave_file(&mut h);
        let question = cant_save(why);
        assert!(h.query_by_label(&question).is_some(), "leaving the page asked nothing: {question:?}");
        let discard = hit_rect(&h, "questionAction", 0).expect("the question's Discard changes is in the hit map");
        assert!(on_screen(discard), "Discard changes is drawn off screen at {discard:?}");
        click(&mut h, discard.center());
        h.run_steps(4);

        assert!(h.state().dialog.is_none(), "the question stayed open after Discard changes");
        let s = &h.state().session;
        assert!(s.signature_edit.is_none(), "Discard changes left the signature open");
        assert!(s.mailbox == mailbox, "Discard changes changed the mailbox");
        assert_eq!(s.mailbox.signature(NAME).and_then(|x| x.html()), None, "the discarded HTML was kept");
        assert!(!h.state().ui.backstage, "Discard changes did not leave File");
    }
}

/// The open signature's text changed by a command (`compose.type`), which the engine saves as rich
/// text without the HTML it no longer matches: Save follows what the engine would save, and is
/// not held back by a picture of the HTML that is no longer kept.
#[test]
fn save_follows_the_signature_the_engine_would_save() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut a = opened(LOCAL);
    a.execute("compose.type", json!({"text": "Harbour "})).expect("typed into the open signature");
    let e = a.session.signature_edit.as_ref().expect("a signature is open");
    let mut saved = e.signature.clone();
    saved.set_body(e.body.text().clone()).expect("its text");
    assert!(e.signature.html().is_some() && saved.html().is_none(), "fixture guard: the engine would save it as rich text");
    let h = window(a);
    let (_, disabled) = save_button(&h);
    assert!(!disabled, "Save is held back by HTML that would not be saved");
}
