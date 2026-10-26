// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar after the forms' date fields (#329): a pick is written as typing would write it,
//! and None is offered only where the field may be empty.

use egui::Key;
use egui_kittest::kittest::Queryable as _;
use mailcraft_time::Date;
use serde_json::json;

use crate::people_view::testing::{self, Window};

fn window(module: &str, dialog: serde_json::Value) -> Window {
    let mut h = testing::window(module);
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.state_mut().execute("ui.dialog", dialog).unwrap();
    h.run_steps(3);
    h
}

fn today(h: &Window) -> Date {
    h.state().session.today()
}

/// Opens the `n`th calendar of the form.
fn calendar(h: &mut Window, n: usize) {
    h.get_all_by_label(tl!("Pick a date")).nth(n).unwrap().click();
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx), "calendar {n} opened");
}

/// Today or None in the open calendar (the form may have its own, laid out before it).
fn pick(h: &mut Window, label: &str) {
    h.get_all_by_role_and_label(egui::accesskit::Role::Button, label).last().unwrap().click();
}

/// What OK would send: the open form's fields.
fn fields(h: &Window) -> serde_json::Value {
    use crate::forms::Form;
    match &h.state().dialog {
        Some(crate::dialogs::Dialog::Form(form)) => match form {
            Form::Event(f) => f.command().1,
            Form::Task(f) => f.command().1,
            Form::Contact(f) => f.command().1,
            _ => panic!("not a form with dates"),
        },
        other => panic!("no form: {:?}", other.as_ref().map(crate::dialogs::Dialog::name)),
    }
}

fn field(h: &Window, key: &str) -> String {
    fields(h)[key].as_str().unwrap_or_default().to_string()
}

#[test]
fn the_appointments_start_day_moves_the_end_as_typing_does() {
    let mut h = window("view.calendar", json!({"name": "event", "title": "Planning"}));
    let next = today(&h).add_days(1).iso();
    calendar(&mut h, 0);
    assert!(h.query_by_label(tl!("None")).is_none(), "an appointment needs its days");
    testing::key(&mut h, Key::ArrowRight);
    testing::key(&mut h, Key::Enter);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert!(field(&h, "start").starts_with(&next), "{}", fields(&h));
    assert!(field(&h, "end").starts_with(&next), "the end follows: {}", fields(&h));
}

#[test]
fn a_tasks_due_day_is_picked_and_none_empties_it() {
    let mut h = window("view.tasks", json!({"name": "task"}));
    let day = today(&h).add_days(7).iso();
    calendar(&mut h, 0);
    testing::key(&mut h, Key::ArrowDown);
    testing::key(&mut h, Key::Enter);
    assert_eq!(field(&h, "due"), day);
    calendar(&mut h, 0);
    pick(&mut h, tl!("None"));
    h.run_steps(3);
    assert_eq!(field(&h, "due"), "");
}

#[test]
fn a_reminder_day_picked_brings_a_time() {
    let mut h = window("view.tasks", json!({"name": "task"}));
    // Due, start, reminder.
    calendar(&mut h, 2);
    pick(&mut h, tl!("Today"));
    h.run_steps(3);
    let reminder = field(&h, "reminder");
    assert!(reminder.starts_with(&format!("{}T", today(&h).iso())), "a day and a time: {reminder}");
    // With a time, the day may not be emptied from the calendar; the row's None link still does.
    let before = h.query_all_by_label(tl!("None")).count();
    calendar(&mut h, 2);
    assert_eq!(h.query_all_by_label(tl!("None")).count(), before, "no None in the calendar");
}

#[test]
fn a_contacts_birthday_has_its_calendar() {
    let mut h = window("view.people", json!({"name": "contact"}));
    calendar(&mut h, 0);
    pick(&mut h, tl!("Today"));
    h.run_steps(3);
    assert_eq!(field(&h, "birthday"), today(&h).iso());
    calendar(&mut h, 0);
    pick(&mut h, tl!("None"));
    h.run_steps(3);
    assert_eq!(field(&h, "birthday"), "");
}
