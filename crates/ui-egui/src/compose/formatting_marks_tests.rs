// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Format Text › Show/Hide ¶ (#1278): the marks are a view of the text being edited. Turning
//! them on does not change the message, and what is sent does not contain them.

use egui::{Key, Modifiers};
use egui_kittest::kittest::Queryable as _;
use serde_json::json;

use super::super::tests::{key, key_with, plain, type_in, writing};
use super::{layout_text, marked_space};
use crate::ribbon;
use mailcraft_model::rich::LINE_BREAK;

#[test]
fn spaces_are_marked_and_a_line_break_is_laid_out_as_one_character() {
    assert!(marked_space(' ') && marked_space('\u{00A0}'));
    assert!(!marked_space('\t') && !marked_space('\n') && !marked_space(LINE_BREAK) && !marked_space('a'));
    assert_eq!(layout_text("a b"), "a b");
    assert_eq!(layout_text(&format!("a{LINE_BREAK}b")), "a\nb");
    assert_eq!(format!("a{LINE_BREAK}b").chars().count(), "a\nb".chars().count());
}

#[test]
fn the_switch_does_not_change_the_message() {
    let mut h = writing();
    type_in(&mut h, "Hello  ");
    key(&mut h, Key::Tab);
    key_with(&mut h, Modifiers { shift: true, ..Modifiers::NONE }, Key::Enter);
    type_in(&mut h, "next");
    let body = h.state().session.compose.as_ref().unwrap().body.text().clone();
    assert_eq!(body.paragraphs().len(), 1, "Shift+Enter stays in the paragraph");
    assert!(body.text().contains(LINE_BREAK));
    let html = mailcraft_mime_model::rich::to_html(&body);
    assert!(html.contains("<br>"), "{html}");
    assert!(!marked(&html), "{html}");
    assert!(!marked(&body.to_plain()));

    assert_eq!(h.state_mut().execute("ui.showFormattingMarks", json!({})).unwrap()["value"], true);
    assert!(h.state().ui.show_formatting_marks);
    h.run_steps(2);
    let shown = h.state().session.compose.as_ref().unwrap().body.text().clone();
    assert_eq!(shown, body);
    assert_eq!(mailcraft_mime_model::rich::to_html(&shown), html);

    assert_eq!(h.state_mut().execute("ui.showFormattingMarks", json!({"value": false})).unwrap()["value"], false);
    assert!(!h.state().ui.show_formatting_marks);
    assert!(h.state_mut().execute("ui.showFormattingMarks", json!({"value": "yes"})).is_err());
    assert!(!h.state().ui.show_formatting_marks);

    // Enter still starts a paragraph. The marks stay out of it.
    key(&mut h, Key::Enter);
    type_in(&mut h, "para");
    let body = h.state().session.compose.as_ref().unwrap().body.text().clone();
    assert_eq!(body.paragraphs().len(), 2);
    // The editor's own text keeps the break as one character, so the caret's positions stay put.
    // What an agent reads, and what is sent, is a real new line, with none of the marks.
    assert!(body.text().contains(LINE_BREAK));
    assert_eq!(body.text().chars().count(), plain(&h).chars().count());
    assert_eq!(plain(&h), "Hello  \t\nnext\npara");
    assert_eq!(body.to_plain(), "Hello  \t\nnext\npara");
    let html = mailcraft_mime_model::rich::to_html(&body);
    assert!(!marked(&html), "{html}");
}

fn marked(text: &str) -> bool {
    text.chars().any(|c| matches!(c, '\u{00B6}' | '\u{00B7}' | '\u{2192}' | '\u{21B5}' | LINE_BREAK))
}

#[test]
fn the_format_text_tab_offers_the_switch_and_it_follows_the_setting() {
    let mut h = writing();
    h.state_mut().execute("ui.tab", json!({"tab": "Format Text"})).unwrap();
    let item = |h: &super::super::tests::Window| {
        ribbon::describe_tab(h.state(), "Format Text")
            .items()
            .find(|item| item.command.as_ref().is_some_and(|(id, _)| *id == "ui.showFormattingMarks"))
            .cloned()
            .expect("Format Text has no Show/Hide ¶")
    };
    let off = item(&h);
    assert_eq!((off.label, off.tip, off.icon, off.checked), ("Show/Hide ¶", "Show/Hide formatting marks", "formattingMarks", false));
    h.state_mut().execute("ui.showFormattingMarks", json!({"value": true})).unwrap();
    assert!(item(&h).checked);
}

/// The signature editor has the same switch, and using it leaves the signature as it was.
#[test]
fn the_signature_editor_has_the_same_switch() {
    let mut w = crate::people_view::testing::window("view.mail");
    let _ = w.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    w.state_mut().execute("signature.set", json!({"name": "Standard", "text": "Avery  Quinn"})).unwrap();
    w.state_mut().execute("signature.open", json!({"name": "Standard"})).unwrap();
    w.state_mut().execute("ui.backstage", json!({"value": true, "page": "signatures"})).unwrap();
    w.run_steps(3);
    let before = w.state().session.signature_edit.as_ref().unwrap().body.text().text();
    w.get_by_label("Show/Hide ¶").click();
    w.run_steps(2);
    assert!(w.state().ui.show_formatting_marks);
    assert_eq!(w.state().session.signature_edit.as_ref().unwrap().body.text().text(), before);
    w.get_by_label("Show/Hide ¶").click();
    w.run_steps(2);
    assert!(!w.state().ui.show_formatting_marks);
    assert_eq!(w.state().session.signature_edit.as_ref().unwrap().body.text().text(), before);
}

/// The switch is one of the window settings, so it is still on the next time the window opens.
#[test]
fn the_switch_is_kept_with_the_window_settings() {
    let mut h = writing();
    h.state_mut().execute("ui.showFormattingMarks", json!({"value": true})).unwrap();
    let saved = serde_json::to_value(h.state().prefs()).unwrap();
    assert_eq!(saved["showFormattingMarks"], true);
    let restored: crate::UiState = serde_json::from_value(saved).unwrap();
    assert!(restored.show_formatting_marks);
    let off = serde_json::json!({"tab": "Home"});
    let old: crate::UiState = serde_json::from_value(off).unwrap();
    assert!(!old.show_formatting_marks);
}

/// What the marks look like, when `MAILCRAFT_SHOTS_DIR` is set: spaces, a tab, a line break,
/// an empty paragraph and a paragraph mark, in light and in dark.
#[test]
fn shots_of_the_marks() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else { return };
    for theme in ["light", "dark"] {
        let mut h = writing();
        let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
        h.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
        h.state_mut().execute("compose.set", json!({"body": format!("A  B\tC{LINE_BREAK}D\n\nE")})).unwrap();
        h.state_mut().execute("ui.showFormattingMarks", json!({"value": true})).unwrap();
        h.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
        h.state_mut().execute("ui.tab", json!({"tab": "Format Text"})).unwrap();
        h.run_steps(4);
        h.render().unwrap().save(std::path::Path::new(&dir).join(format!("formatting-marks-{theme}.png"))).unwrap();
    }
}
