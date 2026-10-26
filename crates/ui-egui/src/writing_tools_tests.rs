// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Insert › Date & Time and Review › Word Count in the window (#507): the tabs while a message is
//! being written, the dialogs they open, and what those do.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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
        Some(Dialog::DateTime { choices, selected: 0, .. }) => {
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

/// OK puts in what the dialog showed, though the clock has gone on to another day since.
#[test]
fn date_and_time_puts_in_what_it_showed() {
    let mut h = window(writing());
    h.state_mut().run("insert.dateTime", json!({})).unwrap();
    h.run_steps(3);
    h.state_mut().session.clock.now = Some(NOW + 86_400);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(body(h.state()), "Hello worldWednesday, October 14, 2026");
}

/// Ctrl+Shift+G (Cmd+Shift+G on a Mac) is Word Count while a message is being written, and
/// nothing else has it.
#[test]
fn ctrl_shift_g_counts_the_words() {
    let app = writing();
    assert_eq!(crate::keys::resolve(&app, "Mod+Shift+G"), Some("review.wordCount"));
    let bound: Vec<&str> =
        app.session.registry.all().iter().filter(|c| c.shortcut.split(" / ").any(|k| k.eq_ignore_ascii_case("Mod+Shift+G"))).map(|c| c.id).collect();
    assert_eq!(bound, ["review.wordCount"]);
    let mut app = writing();
    app.execute("compose.discard", json!({})).unwrap();
    assert_eq!(crate::keys::resolve(&app, "Mod+Shift+G"), None);
}

/// The rest of the two tabs is there, greyed, and runs nothing.
#[test]
fn the_rest_of_insert_and_review_is_pending() {
    let mut app = writing();
    for (tab, pending) in
        [("Insert", ["Item", "Table", "Diagram", "Saved Text"]), ("Review", ["Spelling & Grammar", "Thesaurus", "Translate", "Language"])]
    {
        app.execute("ui.tab", json!({"tab": tab})).unwrap();
        let described = crate::ribbon::describe(&app);
        for label in pending {
            let item = described.items().find(|item| item.label == label).unwrap_or_else(|| panic!("{tab}: no {label}"));
            assert!(item.pending && item.command.is_none() && item.menu.is_none(), "{tab}: {label}");
        }
    }
    let mut h = window(app);
    h.state_mut().execute("ui.ribbonMode", json!({"value": "classic"})).unwrap();
    h.run_steps(3);
    // Named for a screen reader, and disabled.
    let pending = h.get_by_label("Thesaurus, pending development");
    assert!(pending.accesskit_node().is_disabled());
    let before = body(h.state());
    let rect = h.state().hit.rect("ribbon", crate::ribbon::hit_id("review.thesaurus")).unwrap();
    let pos = rect.center();
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(body(h.state()), before);
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
