// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Signatures' HTML source editor and the email-compatibility checker
//! (`mailcraft_html::compat`): what the owner sees of a finding in the editor itself.
//!
//! One synthetic signature, opened in the page's editor as its HTML source, holds a picture sized
//! by its inline style alone (rule `OL-IMG-005`), with the owner's words before and after it.
//! Everything is driven as the owner drives it, in one headless window: the pointer moved over the
//! source, pressed and let go. What is asserted is what the window drew this frame: the source
//! editor's own laid-out text (its galley), the underline painted in it, and the popup's words and
//! buttons as the accessibility tree and the page's hit map give them.

use std::ops::Range;
use std::sync::Arc;

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use serde_json::json;

use crate::theme::Tokens;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The window every frame is laid out in, tall enough for the page's own controls.
const SIZE: (f32, f32) = (1280.0, 900.0);

/// The fixture's signature.
const NAME: &str = "Harbour Studio";

/// The owner's HTML: a picture sized by its inline style alone, with no `width` or `height`
/// attribute, between two paragraphs of their words.
const SIZED_BY_STYLE: &str = "<p>Priya Raman</p>\n<img src=\"https://example.org/m.png\" style=\"width:120px;height:40px\">\n<p>Studio Two</p>";

/// What the popup says of `OL-IMG-005`, in English.
const TITLE: &str = "Picture sized only by its style";
const EXPLANATION: &str =
    "Classic Outlook for Windows ignores a picture's size when only its style sets it, and shows the picture at the file's own size.";
const FIX: &str = "Fix: add the size as width and height attributes";

/// The app over the sample mailbox, nothing happening by itself.
fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

/// The app with the fixture's signature open in the page's editor as `html`, not yet saved, and
/// File › Signatures shown.
fn opened(html: &str) -> MailApp {
    let mut a = app();
    a.execute("signature.set", json!({"name": NAME, "text": "Priya Raman", "default": true})).expect("the fixture makes its signature");
    a.execute("signature.open", json!({"name": NAME})).expect("the fixture opens it in the editor");
    a.execute("signature.html", json!({"html": html})).expect("the fixture makes it an HTML signature");
    let e = a.session.signature_edit.as_ref().expect("a signature is open in the editor");
    assert_eq!(e.signature.html(), Some(html), "the open signature does not hold the fixture's HTML");
    assert!(e.dirty, "the fixture's HTML is not an unsaved change");
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

/// Every text shape painted this frame, with where it was painted.
fn texts(h: &Harness<'static, MailApp>) -> Vec<(Arc<egui::Galley>, egui::Pos2)> {
    fn find(shape: &egui::Shape, out: &mut Vec<(Arc<egui::Galley>, egui::Pos2)>) {
        match shape {
            egui::Shape::Text(t) => out.push((t.galley.clone(), t.pos)),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| find(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    h.output().shapes.iter().for_each(|s| find(&s.shape, &mut out));
    out
}

/// The source editor's own laid-out text this frame: the one galley that is the whole `source`.
fn source_galley(h: &Harness<'static, MailApp>, source: &str) -> (Arc<egui::Galley>, egui::Pos2) {
    let found: Vec<_> = texts(h).into_iter().filter(|(g, _)| g.text() == source).collect();
    assert_eq!(found.len(), 1, "the page painted {} texts that are the signature's whole HTML source, not one", found.len());
    found.into_iter().next().expect("one source text")
}

/// The on-screen rectangle of the characters `chars` of a galley painted at `pos`, one per row
/// they are on: from the first glyph's left to the last one's right, the row's full height.
fn char_rects(galley: &egui::Galley, pos: egui::Pos2, chars: Range<usize>) -> Vec<egui::Rect> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for row in &galley.rows {
        let mut left: Option<f32> = None;
        let mut right = 0.0f32;
        for (i, glyph) in row.glyphs.iter().enumerate() {
            if chars.contains(&(at + i)) {
                let x = pos.x + row.pos.x + glyph.pos.x;
                left = Some(left.map_or(x, |l| l.min(x)));
                right = right.max(x + glyph.advance_width);
            }
        }
        if let Some(left) = left {
            let top = pos.y + row.pos.y;
            out.push(egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + row.size.y)));
        }
        at += row.glyphs.len() + usize::from(row.ends_with_newline);
    }
    out
}

/// What is painted in a galley's rows besides its glyphs (its underlines, by epaint's own
/// layout: the vertices after each row's glyph vertices), on screen: for each row that has any,
/// one rectangle around its visible vertices with the colours they have. The fully transparent
/// vertices epaint adds round a line to smooth its edges are left out.
fn underlines(galley: &egui::Galley, pos: egui::Pos2) -> Vec<(egui::Rect, Vec<egui::Color32>)> {
    let mut out = Vec::new();
    for row in &galley.rows {
        let mesh = &row.visuals.mesh;
        let after = mesh.vertices.get(row.visuals.glyph_vertex_range.end..).unwrap_or(&[]);
        let seen: Vec<_> = after.iter().filter(|v| v.color != egui::Color32::TRANSPARENT).collect();
        if seen.is_empty() {
            continue;
        }
        let mut r = egui::Rect::NOTHING;
        for v in &seen {
            r.extend_with(pos + row.pos.to_vec2() + v.pos.to_vec2());
        }
        out.push((r, seen.iter().map(|v| v.color).collect()));
    }
    out
}

/// The characters (not bytes) of `needle` in `hay`, where it first is.
fn chars_of(hay: &str, start: &str, end: &str) -> Range<usize> {
    let from = hay.find(start).expect("the fixture holds the start of its finding");
    let to = hay.find(end).expect("the fixture holds the end of its finding") + end.len();
    hay[..from].chars().count()..hay[..to].chars().count()
}

/// Where the owner's control `key` with `id` was drawn this frame, on screen.
fn hit_rect(h: &Harness<'static, MailApp>, key: &str, id: u64) -> Option<egui::Rect> {
    h.state().hit.rect(key, id)
}

fn on_screen(r: egui::Rect) -> bool {
    r.top() >= 0.0 && r.bottom() <= SIZE.1 && r.left() >= 0.0 && r.right() <= SIZE.0
}

/// A place in the window away from the editor and from any popup next to it.
const AWAY: egui::Pos2 = egui::pos2(SIZE.0 - 30.0, SIZE.1 - 30.0);

#[test]
fn a_picture_sized_by_its_style_alone_is_underlined_and_explained() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(SIZED_BY_STYLE));
    let (galley, pos) = source_galley(&h, SIZED_BY_STYLE);
    // The finding is the style's two sizes, `width:120px;height:40px`.
    let finding = chars_of(SIZED_BY_STYLE, "width:120px", "height:40px");
    let rects = char_rects(&galley, pos, finding.clone());
    assert!(!rects.is_empty(), "the source editor laid out none of the finding's characters");
    let red = Tokens::get(&h.ctx).red;

    // Underlined, in the theme's error colour, only over the finding's own characters.
    let lines = underlines(&galley, pos);
    assert!(!lines.is_empty(), "nothing is underlined in the source editor");
    for (line, colours) in &lines {
        assert!(colours.iter().all(|c| *c == red), "an underline in the source editor is not the error colour {red:?}: {colours:?}");
        assert!(
            rects.iter().any(|g| line.left() >= g.left() - 0.5
                && line.right() <= g.right() + 0.5
                && line.top() >= g.top() - 0.5
                && line.bottom() <= g.bottom() + 1.0),
            "an underline at {line:?} lies outside the finding's characters {rects:?}"
        );
    }
    for g in &rects {
        assert!(
            lines.iter().any(|(line, _)| (line.left() - g.left()).abs() <= 1.0 && (line.right() - g.right()).abs() <= 1.0),
            "the finding's characters at {g:?} are not underlined from end to end: {lines:?}"
        );
    }
    assert!(h.query_by_label(TITLE).is_none(), "the popup is open before the owner pointed at anything");

    // Hovering the underline opens the popup: what is wrong, why, and the fix.
    let widest = rects.iter().copied().max_by(|a, b| a.width().total_cmp(&b.width())).expect("a row of the finding");
    hover(&mut h, widest.center());
    assert!(h.query_by_label(TITLE).is_some(), "hovering the underline showed no popup titled {TITLE:?}");
    assert!(h.query_by_label(EXPLANATION).is_some(), "the popup does not explain the problem");
    assert!(h.query_by_label(FIX).is_some(), "the popup offers no fix button {FIX:?}");
    let popup = hit_rect(&h, "signatureCompatPopup", 0).expect("the popup is in the page's hit map");
    assert!(on_screen(popup), "the popup is drawn off screen at {popup:?}");
    let fix = hit_rect(&h, "signatureCompatFix", 0).expect("the popup's fix button is in the page's hit map");
    assert!(popup.contains_rect(fix), "the fix button at {fix:?} is not inside the popup at {popup:?}");

    // Moving away closes it.
    hover(&mut h, AWAY);
    assert!(h.query_by_label(TITLE).is_none(), "the hover popup stayed open after the pointer left the underline");

    // A left click on the underline opens it to stay, even when the pointer then leaves.
    click(&mut h, widest.center());
    hover(&mut h, AWAY);
    assert!(h.query_by_label(TITLE).is_some(), "a click on the underline did not keep its popup open");

    // A press elsewhere closes it.
    click(&mut h, AWAY);
    assert!(h.query_by_label(TITLE).is_none(), "the clicked popup stayed open after a press elsewhere");
}

/// The same signature with the fix of its one finding made: the two sizes added as attributes
/// right after the tag's name, and every other byte as the owner wrote it.
const FIXED: &str =
    "<p>Priya Raman</p>\n<img width=\"120\" height=\"40\" src=\"https://example.org/m.png\" style=\"width:120px;height:40px\">\n<p>Studio Two</p>";

/// The HTML the signature open in the editor holds now.
fn open_html(h: &Harness<'static, MailApp>) -> String {
    h.state().session.signature_edit.as_ref().and_then(|e| e.signature.html()).map(str::to_string).expect("an HTML signature is open in the editor")
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

/// Where the finding's underline is widest, after the window settled on `source`.
fn finding_spot(h: &Harness<'static, MailApp>, source: &str) -> egui::Pos2 {
    let (galley, pos) = source_galley(h, source);
    let rects = char_rects(&galley, pos, chars_of(source, "width:120px", "height:40px"));
    rects.iter().copied().max_by(|a, b| a.width().total_cmp(&b.width())).expect("a row of the finding").center()
}

/// Ctrl+Z, as the keyboard sends it, in the field that has the focus.
fn undo_key(h: &mut Harness<'static, MailApp>) {
    let ctrl = egui::Modifiers { ctrl: true, command: true, ..egui::Modifiers::NONE };
    for pressed in [true, false] {
        h.event(egui::Event::Key { key: egui::Key::Z, physical_key: None, pressed, repeat: false, modifiers: ctrl });
        h.step();
    }
    h.run_steps(2);
}

/// The owner's press at the end of the source's last row: the caret there, in the editor.
fn caret_at_end(h: &mut Harness<'static, MailApp>, source: &str) {
    let (galley, pos) = source_galley(h, source);
    let last = galley.rows.last().expect("the source has a row");
    let y = pos.y + last.pos.y + last.size.y / 2.0;
    let x = pos.x + last.pos.x + last.size.x + 2.0;
    click(h, egui::pos2(x, y));
    assert!(h.ctx.memory(|m| m.has_focus(crate::compat::editor_id())), "the press at the end of the source did not give the editor the keyboard");
}

#[test]
fn the_popups_fix_adds_the_size_attributes_and_the_underline_goes() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(SIZED_BY_STYLE));
    let spot = finding_spot(&h, SIZED_BY_STYLE);
    hover(&mut h, spot);
    let fix = hit_rect(&h, "signatureCompatFix", 0).expect("the popup's fix button is in the page's hit map");
    assert!(h.query_by_label(FIX).is_some(), "the popup offers no fix button {FIX:?}");
    click(&mut h, fix.center());

    // Exactly the fix's edit, sent as typing is: the open signature holds it, unsaved.
    assert_eq!(open_html(&h), FIXED, "the fix did not make exactly its own edit");
    let at = SIZED_BY_STYLE.find("<img").expect("the fixture's picture") + "<img".len();
    assert!(FIXED.starts_with(&SIZED_BY_STYLE[..at]) && FIXED.ends_with(&SIZED_BY_STYLE[at..]), "the fix changed text outside its own place");
    assert!(h.state().session.signature_edit.as_ref().is_some_and(|e| e.dirty), "the fixed signature is not an unsaved change");
    assert_eq!(h.state().session.mailbox.signature(NAME).and_then(|s| s.html()), None, "the fix saved the signature by itself; only Save keeps it");

    // The underline and the popup are gone; Save can be pressed, and keeps the fixed HTML.
    let (galley, pos) = source_galley(&h, FIXED);
    assert!(underlines(&galley, pos).is_empty(), "the fixed source is still underlined");
    assert!(hit_rect(&h, "signatureCompatMark", 0).is_none(), "the fixed source still has a finding in the hit map");
    assert!(h.query_by_label(TITLE).is_none(), "the popup stayed open after its fix was made");
    let (save, disabled) = save_button(&h);
    assert!(!disabled, "Save is disabled once the only error was fixed");
    click(&mut h, save.center());
    assert_eq!(h.state().session.mailbox.signature(NAME).and_then(|s| s.html()), Some(FIXED), "Save did not keep the fixed HTML");
}

#[test]
fn undo_in_the_editor_takes_back_the_fix_as_it_takes_back_typing() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(SIZED_BY_STYLE));
    // The owner has not been in the editor yet: the fix is the first change it sees.
    let spot = finding_spot(&h, SIZED_BY_STYLE);
    hover(&mut h, spot);
    let fix = hit_rect(&h, "signatureCompatFix", 0).expect("the popup's fix button is in the page's hit map");
    click(&mut h, fix.center());
    assert_eq!(open_html(&h), FIXED, "the fix did not make its edit");

    // Into the editor, and Ctrl+Z: the text before the fix, sent as typing is.
    caret_at_end(&mut h, FIXED);
    undo_key(&mut h);
    assert_eq!(open_html(&h), SIZED_BY_STYLE, "Ctrl+Z in the editor did not take back the fix");

    // The control: typing, then Ctrl+Z, takes back the typing the same way.
    caret_at_end(&mut h, SIZED_BY_STYLE);
    h.event(egui::Event::Text("!".into()));
    h.run_steps(2);
    assert_eq!(open_html(&h), format!("{SIZED_BY_STYLE}!"), "typing at the end of the source did not reach the signature");
    undo_key(&mut h);
    assert_eq!(open_html(&h), SIZED_BY_STYLE, "Ctrl+Z in the editor did not take back the typing");
}

#[test]
fn a_fix_is_made_only_on_the_text_it_was_found_in() {
    let checked = crate::compat::Checked::of(SIZED_BY_STYLE);
    assert_eq!(checked.marks().len(), 1, "the fixture has one finding: {:?}", checked.marks);
    assert_eq!(checked.fixed(SIZED_BY_STYLE, 0).as_deref(), Some(FIXED), "the fix of the checked text is not its exact edit");
    let edited = format!("{SIZED_BY_STYLE} ");
    assert_eq!(checked.fixed(&edited, 0), None, "a fix was made on a text other than the one it was found in");
    assert_eq!(checked.fixed(SIZED_BY_STYLE, 1), None, "a fix was made for a finding there is not");
}

#[test]
fn findings_are_kept_for_their_exact_text_in_characters() {
    // Words before the picture that take more bytes than characters.
    let html = "<p>Prïyà Ramán — Studio</p><img src=\"https://example.org/m.png\" style=\"width:120px\">";
    let checked = crate::compat::Checked::of(html);
    let mark = checked.marks().first().expect("the picture sized by its style is found");
    let from = html.find("width:120px").expect("the size");
    assert_eq!(mark.bytes, from..from + "width:120px".len(), "the finding is not the style's size");
    let chars = html[..from].chars().count();
    assert_eq!(mark.chars, chars..chars + "width:120px".len(), "the finding's characters are not its bytes counted as characters");
    assert_ne!(mark.chars, mark.bytes, "the fixture's words take no more bytes than characters");

    let ctx = egui::Context::default();
    let first = crate::compat::checked(&ctx, html);
    assert!(Arc::ptr_eq(&first, &crate::compat::checked(&ctx, html)), "the same text was checked again");
    let other = crate::compat::checked(&ctx, SIZED_BY_STYLE);
    assert!(!Arc::ptr_eq(&first, &other) && other.text == SIZED_BY_STYLE, "a changed text kept the findings of the one before");
}

/// What Save's hover text says of one error, and the line below the editor.
const SAVE_REFUSAL: &str = "Fix 1 email compatibility error to save this signature.";
const SUMMARY: &str = "1 email compatibility error";

#[test]
fn save_is_disabled_while_an_error_remains_and_says_why() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(SIZED_BY_STYLE));
    let mailbox = h.state().session.mailbox.clone();
    let editor = h.state().session.signature_edit.clone();
    assert!(editor.as_ref().is_some_and(|e| e.dirty), "the fixture's signature has no unsaved change, so Save would be disabled anyway");

    let (save, disabled) = save_button(&h);
    assert!(disabled, "Save can be pressed while the signature has an email compatibility error");
    assert!(h.query_by_label(SUMMARY).is_some(), "the line below the editor does not count the error as {SUMMARY:?}");

    // Its hover text says how many errors must be fixed.
    hover(&mut h, save.center());
    h.run_steps(60);
    assert!(h.query_by_label(SAVE_REFUSAL).is_some(), "Save's hover text does not say {SAVE_REFUSAL:?}");

    // Pressing it keeps nothing.
    click(&mut h, save.center());
    assert!(h.state().session.mailbox == mailbox, "pressing the disabled Save changed the mailbox");
    assert_eq!(h.state().session.signature_edit, editor, "pressing the disabled Save changed the signature open in the editor");
}

/// The engine's refusal of a signature with errors (its contract's exact words, the same for
/// any count).
const REFUSED: &str = "this signature has email compatibility errors to fix before it can be saved";

/// A signature with nothing to report: its picture has its size as attributes too.
const CLEAN: &str = "<p>Priya Raman</p>\n<img width=\"120\" height=\"40\" src=\"https://example.org/m.png\">\n<p>Studio Two</p>";

/// The owner leaves File, as its back arrow and Escape do: the window's own command, then frames.
fn leave_file(h: &mut Harness<'static, MailApp>) {
    h.state_mut().execute("ui.backstage", json!({"value": false})).expect("File closes");
    h.run_steps(4);
}

#[test]
fn leaving_the_page_keeps_the_edits_when_the_save_is_refused() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(SIZED_BY_STYLE));
    let mailbox = h.state().session.mailbox.clone();
    let editor = h.state().session.signature_edit.clone();
    assert!(editor.as_ref().is_some_and(|e| e.dirty && e.signature.html() == Some(SIZED_BY_STYLE)), "the fixture has no unsaved HTML");

    leave_file(&mut h);
    let s = &h.state().session;
    assert!(s.signature_edit.is_some(), "leaving the page closed the signature whose save was refused: the owner's edits are gone");
    assert_eq!(s.signature_edit, editor, "leaving the page changed the signature open in the editor");
    assert!(s.mailbox == mailbox, "the refused save changed the mailbox");
    let ui = &h.state().ui;
    assert!(ui.backstage && ui.backstage_page == "signatures", "the window did not stay on File › Signatures with the signature");
    assert!(h.query_by_label(REFUSED).is_some(), "the page does not say why the signature was not saved: {REFUSED:?}");
}

#[test]
fn leaving_the_page_saves_a_clean_signature_and_closes_it() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(CLEAN));
    assert!(h.query_by_label(SUMMARY).is_none(), "the clean fixture is counted as having an error");
    leave_file(&mut h);
    let s = &h.state().session;
    assert!(s.signature_edit.is_none(), "leaving the page did not close the saved signature");
    assert_eq!(s.mailbox.signature(NAME).and_then(|x| x.html()), Some(CLEAN), "leaving the page did not keep the clean signature");
    assert!(!h.state().ui.backstage, "leaving the page with a clean signature kept File open");
}

/// A signature whose style sheet asks for another sheet from the web (rule `MC-CSS-001`).
const STYLE_SHEET: &str = "<style>@import \"https://example.org/brand.css\";</style>\n<p>Priya Raman</p>";

/// What the popup says of `MC-CSS-001`, in English.
const SHEET_TITLE: &str = "Style rule that can't be sent";
const SHEET_EXPLANATION: &str = "This style rule uses something MailCraft can't send safely (such as an import, a web address or an unsupported selector). Remove or simplify the rule.";

#[test]
fn a_style_sheet_that_cant_be_sent_is_underlined_and_explained_without_a_fix() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(STYLE_SHEET));
    let (galley, pos) = source_galley(&h, STYLE_SHEET);
    let rects = char_rects(&galley, pos, chars_of(STYLE_SHEET, "<style>", "</style>"));
    assert!(!rects.is_empty(), "the source editor laid out none of the style element's characters");
    assert!(!underlines(&galley, pos).is_empty(), "the style element is not underlined");
    let widest = rects.iter().copied().max_by(|a, b| a.width().total_cmp(&b.width())).expect("a row of the finding");
    hover(&mut h, widest.center());
    assert!(h.query_by_label(SHEET_TITLE).is_some(), "hovering the style element showed no popup titled {SHEET_TITLE:?}");
    assert!(h.query_by_label(SHEET_EXPLANATION).is_some(), "the popup does not explain the style sheet's problem");
    assert!(hit_rect(&h, "signatureCompatFix", 0).is_none(), "the popup offers a fix for a rule that has none");
    let (_, disabled) = save_button(&h);
    assert!(disabled, "Save can be pressed while the style sheet can't be sent");
}

/// The question asked when the page is left with a signature that can't be saved, and its two
/// answers.
const CANT_SAVE: &str = "This signature can't be saved until its email compatibility errors are fixed.";
const KEEP_EDITING: &str = "Keep editing";
const DISCARD: &str = "Discard changes";

#[test]
fn leaving_with_an_error_asks_and_keep_editing_keeps_every_edit() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(SIZED_BY_STYLE));
    let mailbox = h.state().session.mailbox.clone();
    let editor = h.state().session.signature_edit.clone();
    assert!(editor.as_ref().is_some_and(|e| e.dirty && e.signature.html() == Some(SIZED_BY_STYLE)), "the fixture has no unsaved HTML");

    leave_file(&mut h);
    assert!(h.query_by_label(CANT_SAVE).is_some(), "leaving the page asked nothing: {CANT_SAVE:?}");
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
    assert_eq!(open_html(&h), SIZED_BY_STYLE, "the editor does not hold the owner's HTML after Keep editing");

    // Keep editing leaves nothing armed: leaving again asks again, and changes nothing meanwhile.
    leave_file(&mut h);
    assert!(h.query_by_label(CANT_SAVE).is_some(), "leaving again did not ask again");
    assert_eq!(h.state().session.signature_edit, editor, "leaving again changed the signature open in the editor");
}

#[test]
fn leaving_with_an_error_and_discarding_closes_the_signature_and_leaves() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(SIZED_BY_STYLE));
    let mailbox = h.state().session.mailbox.clone();
    assert_eq!(mailbox.signature(NAME).and_then(|s| s.html()), None, "the fixture's signature is kept as HTML already");

    leave_file(&mut h);
    assert!(h.query_by_label(CANT_SAVE).is_some(), "leaving the page asked nothing: {CANT_SAVE:?}");
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

/// A key pressed and let go, as the owner presses it, a frame each.
fn key(h: &mut Harness<'static, MailApp>, key: egui::Key) {
    for pressed in [true, false] {
        h.event(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

/// A signature whose only finding is a warning (rule `OL-BOX-007`): a paragraph with rounded
/// corners.
const ROUNDED: &str = "<p style=\"border-radius:4px;padding:4px\">Priya Raman</p>\n<p>Studio Two</p>";

/// What the popup says of `OL-BOX-007`, in English, and the line below the editor.
const ROUNDED_TITLE: &str = "Rounded corners";
const ONE_WARNING: &str = "1 warning";

#[test]
fn a_warning_is_underlined_in_the_warning_colour_and_does_not_stop_save() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(ROUNDED));
    let (galley, pos) = source_galley(&h, ROUNDED);
    let rects = char_rects(&galley, pos, chars_of(ROUNDED, "border-radius", "4px"));
    assert!(!rects.is_empty(), "the source editor laid out none of the finding's characters");
    let tokens = Tokens::get(&h.ctx);
    assert_ne!(tokens.orange, tokens.red, "the theme's warning colour is its error colour: the test could not tell them apart");

    // Underlined in the warning colour, only over the declaration's own characters.
    let lines = underlines(&galley, pos);
    assert!(!lines.is_empty(), "the warning is not underlined in the source editor");
    for (line, colours) in &lines {
        assert!(colours.iter().all(|c| *c == tokens.orange), "an underline is not the warning colour {:?}: {colours:?}", tokens.orange);
        assert!(
            rects.iter().any(|g| line.left() >= g.left() - 0.5
                && line.right() <= g.right() + 0.5
                && line.top() >= g.top() - 0.5
                && line.bottom() <= g.bottom() + 1.0),
            "an underline at {line:?} lies outside the warning's characters {rects:?}"
        );
    }
    for g in &rects {
        assert!(
            lines.iter().any(|(line, _)| (line.left() - g.left()).abs() <= 1.0 && (line.right() - g.right()).abs() <= 1.0),
            "the warning's characters at {g:?} are not underlined from end to end: {lines:?}"
        );
    }
    assert!(h.query_by_label(ONE_WARNING).is_some(), "the line below the editor does not count the warning as {ONE_WARNING:?}");
    assert!(h.query_by_label(SUMMARY).is_none(), "the warning is counted as an error");

    // Its popup says what it is, with no fix to offer.
    let widest = rects.iter().copied().max_by(|a, b| a.width().total_cmp(&b.width())).expect("a row of the finding");
    hover(&mut h, widest.center());
    assert!(h.query_by_label(ROUNDED_TITLE).is_some(), "hovering the warning showed no popup titled {ROUNDED_TITLE:?}");
    assert!(hit_rect(&h, "signatureCompatFix", 0).is_none(), "the popup offers a fix for a warning that has none");

    // Save can be pressed, and keeps the HTML as the owner wrote it.
    hover(&mut h, AWAY);
    let (save, disabled) = save_button(&h);
    assert!(!disabled, "Save is disabled by a warning");
    click(&mut h, save.center());
    assert_eq!(h.state().session.mailbox.signature(NAME).and_then(|s| s.html()), Some(ROUNDED), "Save did not keep the HTML with its warning");
}

#[test]
fn keep_editing_is_what_escape_answers_and_enter_changes_nothing() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut h = window(opened(SIZED_BY_STYLE));
    let mailbox = h.state().session.mailbox.clone();
    let editor = h.state().session.signature_edit.clone();
    assert!(editor.as_ref().is_some_and(|e| e.dirty && e.signature.html() == Some(SIZED_BY_STYLE)), "the fixture has no unsaved HTML");

    leave_file(&mut h);
    assert!(h.query_by_label(CANT_SAVE).is_some(), "leaving the page asked nothing: {CANT_SAVE:?}");
    // Enter discards nothing and types nothing into the editor behind the question.
    key(&mut h, egui::Key::Enter);
    assert_eq!(h.state().session.signature_edit, editor, "Enter changed the signature open in the editor");
    assert!(h.state().session.mailbox == mailbox, "Enter changed the mailbox");

    key(&mut h, egui::Key::Escape);
    assert!(h.state().dialog.is_none(), "Escape did not answer the question");
    let s = &h.state().session;
    assert_eq!(s.signature_edit, editor, "Escape changed the signature open in the editor");
    assert!(s.mailbox == mailbox, "Escape changed the mailbox");
    let ui = &h.state().ui;
    assert!(ui.backstage && ui.backstage_page == "signatures", "Escape did not keep File › Signatures open");
}
