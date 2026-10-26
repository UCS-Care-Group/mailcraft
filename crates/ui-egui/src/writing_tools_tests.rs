// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Insert › Date & Time and Review › Word Count in the window (#507): the tabs while a message is
//! being written, the dialogs they open, and what those do.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A new message whose body is "Hello world" alone.
fn writing() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("mail.new", json!({})).unwrap();
    let len = app.session.compose.as_ref().unwrap().body.text().len();
    app.execute("compose.select", json!({"from": 0, "to": len})).unwrap();
    app.execute("compose.type", json!({"text": "Hello world"})).unwrap();
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
fn insert_and_review_are_tabs_while_writing() {
    let app = writing();
    assert_eq!(crate::ribbon::tabs(&app)[..4], ["File", "Message", "Insert", "Review"]);
    let mut app = writing();
    app.execute("compose.discard", json!({})).unwrap();
    assert!(!crate::ribbon::tabs(&app).contains(&"Insert"));
}

#[test]
fn date_and_time_lists_the_formats_and_ok_puts_one_in() {
    let mut h = window(writing());
    h.state_mut().run("insert.dateTime", json!({})).unwrap();
    h.run_steps(3);
    match h.state().dialog.as_ref() {
        Some(Dialog::DateTime { choices, selected: 0 }) => {
            assert_eq!(choices.len(), 8);
            assert_eq!(choices[0], ("long", "Wednesday, October 14, 2026".to_string()));
        }
        _ => panic!("Date and Time isn't open"),
    }
    // The second (Down), then OK.
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(Dialog::DateTime { selected: 1, .. })));
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(body(h.state()), "Hello worldOctober 14, 2026");
}

#[test]
fn word_count_shows_the_counts() {
    let mut h = window(writing());
    h.state_mut().run("review.wordCount", json!({})).unwrap();
    h.run_steps(3);
    match h.state().dialog.as_ref() {
        Some(Dialog::WordCount { counts, selection: false }) => assert_eq!(*counts, [2, 10, 11, 1]),
        _ => panic!("Word Count isn't open"),
    }
    assert!(h.query_by_label("Characters (with spaces)").is_some());
    // (The message being written has a Close of its own.)
    h.query_all_by_label("Close").last().unwrap().click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(body(h.state()), "Hello world");
}

#[test]
fn neither_opens_without_a_message() {
    let mut app = writing();
    app.execute("compose.discard", json!({})).unwrap();
    assert!(app.run("review.wordCount", json!({})).is_err());
    assert!(app.run("insert.dateTime", json!({})).is_err());
    assert!(app.dialog.is_none());
}
