// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Email Calendar, Insert › Calendar, Tracking and Search Calendar in the window: the ribbon's
//! buttons, the dialogs they open, and what those do through real frames. Nothing is sent.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
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

/// The sample's meeting that the owner organizes today, with three people invited.
fn design_review(app: &MailApp) -> u64 {
    app.session.mailbox.events.values().find(|e| e.title == "Design review: homepage").map(|e| e.id).unwrap()
}

fn dialog_name(app: &MailApp) -> Option<&'static str> {
    app.dialog.as_ref().map(Dialog::name)
}

#[test]
fn the_calendar_ribbon_has_them_and_the_insert_tab_has_calendar() {
    let mut app = app();
    app.execute("view.calendar", json!({})).unwrap();
    let keys = |app: &MailApp| crate::ribbon::describe(app).items().map(|i| i.key).collect::<Vec<_>>();
    let shown = keys(&app);
    assert!(shown.contains(&"calendar.email") && shown.contains(&"calendar.search"), "{shown:?}");
    assert!(!shown.contains(&"calendar.tracking"), "nothing selected");
    let id = design_review(&app);
    app.execute("calendar.select", json!({"id": id})).unwrap();
    let shown = keys(&app);
    assert!(shown.contains(&"calendar.tracking") && shown.contains(&"calendar.contactAttendees"), "{shown:?}");
    app.execute("mail.new", json!({})).unwrap();
    app.ui.tab = "Insert".into();
    let item = crate::ribbon::describe(&app).items().find(|i| i.key == "insert.calendar").cloned().unwrap();
    assert!(!item.pending);
}

#[test]
fn email_calendar_asks_which_days_then_opens_a_message() {
    let mut app = app();
    app.run("calendar.email", json!({})).unwrap();
    assert_eq!(dialog_name(&app), Some("emailCalendar"));
    let mut h = window(app);
    h.get_by_label("Limited details: and the titles").click();
    h.run_steps(2);
    h.get_by_label("Create Message").click();
    h.run_steps(4);
    let app = h.state();
    assert!(app.dialog.is_none());
    let c = app.session.compose.as_ref().expect("a message is being written");
    assert!(c.subject.starts_with("Calendar: ") && c.to.is_empty(), "{}", c.subject);
    assert!(c.body.text().text().contains("Design review: homepage"));
    assert!(c.attachments.iter().any(|a| a.name == "calendar.ics"));
}

#[test]
fn tracking_lists_who_answered() {
    let mut app = app();
    let id = design_review(&app);
    app.execute("calendar.select", json!({"id": id})).unwrap();
    app.run("calendar.tracking", json!({})).unwrap();
    assert_eq!(dialog_name(&app), Some("tracking"));
    let h = window(app);
    h.get_by_label("Maya Okafor");
    h.get_by_label("1 accepted · 1 tentative · 0 declined · 1 not answered");
}

#[test]
fn search_calendar_finds_and_goes_to_the_appointment() {
    let mut app = app();
    app.run("calendar.search", json!({})).unwrap();
    assert_eq!(dialog_name(&app), Some("searchCalendar"));
    app.execute("ui.dialog", json!({"name": "searchCalendar", "query": "birch"})).unwrap();
    let mut h = window(app);
    h.get_by_label("Search").click();
    h.run_steps(3);
    h.get_by_label("Design review: homepage").click();
    h.run_steps(3);
    let app = h.state();
    assert!(app.dialog.is_none());
    assert_eq!(app.session.sel.event, Some(design_review(app)));
}
