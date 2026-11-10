// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Find and Replace in the window: the Message tab's buttons open the dialog, which runs
//! `edit.find` and `edit.replace` on the message being written.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn writing(text: &str) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("mail.new", json!({})).unwrap();
    let len = app.session.compose.as_ref().unwrap().body.text().len();
    app.execute("compose.select", json!({"from": 0, "to": len})).unwrap();
    app.execute("compose.type", json!({"text": text})).unwrap();
    app.execute("compose.select", json!({"from": 0})).unwrap();
    app
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
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

fn body(app: &MailApp) -> String {
    app.session.compose.as_ref().unwrap().body.plain()
}

#[test]
fn the_message_tab_has_find_and_replace() {
    let mut app = writing("x");
    app.ui.tab = "Message".into();
    let tab = crate::ribbon::describe(&app);
    let commands: Vec<&str> = tab.items().filter_map(|i| i.command.as_ref().map(|c| c.0)).collect();
    assert!(commands.contains(&"edit.find") && commands.contains(&"edit.replace"));
    assert!(tab.items().all(|i| i.pending || crate::icons::NAMES.contains(&i.icon)));
}

#[test]
fn find_opens_with_the_selection_and_selects_each_place() {
    let mut app = writing("one two two");
    app.execute("compose.select", json!({"from": 4, "to": 7})).unwrap();
    let mut h = window(app);
    h.state_mut().run("edit.find", json!({})).unwrap();
    h.run_steps(3);
    match h.state().dialog.as_ref() {
        Some(Dialog::FindReplace(d)) => assert_eq!((d.text.as_str(), d.replace), ("two", false)),
        _ => panic!("Find isn't open"),
    }
    h.get_by_label("Find Next").click();
    h.run_steps(3);
    let sel = h.state().session.compose.as_ref().unwrap().body.selection();
    assert_eq!((sel.start(), sel.end()), (8, 11));
    let at = h.state().hit.rect("findClose", 0).unwrap().center();
    crate::people_view::testing::click(&mut h, at, egui::PointerButton::Primary, 1);
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
}

#[test]
fn replace_all_from_the_dialog_is_one_undo_step() {
    let mut h = window(writing("cat and cat"));
    h.state_mut().run("edit.replace", json!({})).unwrap();
    h.run_steps(3);
    if let Some(Dialog::FindReplace(d)) = h.state_mut().dialog.as_mut() {
        d.text = "cat".into();
        d.with = "dog".into();
    } else {
        panic!("Replace isn't open");
    }
    h.run_steps(2);
    h.get_by_label("Replace All").click();
    h.run_steps(3);
    assert_eq!(body(h.state()), "dog and dog");
    match h.state().dialog.as_ref() {
        Some(Dialog::FindReplace(d)) => assert_eq!(d.note, "2 replacements were made."),
        _ => panic!("Replace closed"),
    }
    h.state_mut().execute("compose.undo", json!({})).unwrap();
    assert_eq!(body(h.state()), "cat and cat");
}
