// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A dialog keeps the keyboard on its field, but a dialog with two fields lets the other one be
//! clicked into and typed into: Subscribe to Internet Calendar (the name after the address) and
//! Add Reminder (the time after the date).

use egui::accesskit::Role;
use egui::{Event, Modifiers, PointerButton, Rect};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::MailApp;
use crate::dialogs::Dialog;
use crate::people_view::testing;

/// Clicks into the middle of `field`, then types `text`.
fn click_and_type(h: &mut Harness<'_, MailApp>, field: Rect, text: &str) {
    let at = field.center();
    h.event(Event::PointerMoved(at));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
    h.event(Event::Text(text.into()));
    h.run_steps(3);
}

#[test]
fn the_name_can_be_typed_into_after_the_address() {
    let mut h = testing::window_on(mailcraft_engine::sample::mailbox(testing::now(), 0), "view.calendar");
    h.state_mut().run("ui.dialog", json!({"name": "subscribe"})).unwrap();
    h.run_steps(3);
    h.event(Event::Text("webcal://calendar.example/f1.ics".into()));
    h.run_steps(3);
    // The name: the first text field below the address (which, holding a secret, is a password field).
    let address = h.get_by_role(Role::PasswordInput).rect();
    let name = h
        .query_all_by_role(Role::TextInput)
        .map(|f| f.rect())
        .filter(|r| r.top() >= address.bottom())
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("the name field");
    click_and_type(&mut h, name, "F1");
    match &h.state().dialog {
        Some(Dialog::Subscribe { address, name, .. }) => {
            assert_eq!(address, "webcal://calendar.example/f1.ics");
            assert_eq!(name, "F1", "the name took what was typed");
        }
        _ => panic!("the dialog is open"),
    }
}

/// The text field of the window whose value `is` says it is, with that value.
fn field(h: &Harness<'_, MailApp>, is: impl Fn(&str) -> bool) -> (Rect, String) {
    h.query_all_by_role(Role::TextInput).find_map(|f| f.value().filter(|v| is(v)).map(|v| (f.rect(), v))).expect("the field")
}

fn a_date(v: &str) -> bool {
    v.len() == 10 && v.as_bytes().get(4) == Some(&b'-')
}

/// A time, with what was typed into it taken out.
fn a_time(v: &str) -> bool {
    let v = v.replace('x', "");
    v.len() == 5 && v.as_bytes().get(2) == Some(&b':')
}

#[test]
fn the_time_of_a_reminder_can_be_typed_into_after_the_date() {
    let mut h = testing::window_on(mailcraft_engine::sample::mailbox(testing::now(), 0), "view.mail");
    let one = h.state().session.mailbox.messages.values().find(|m| !m.from.email.is_empty()).map(|m| m.id).unwrap();
    h.state_mut().run("mail.select", json!({"id": one})).unwrap();
    h.state_mut().run("ui.dialog", json!({"name": "addReminder"})).unwrap();
    h.run_steps(3);
    assert!(matches!(h.state().dialog, Some(Dialog::AddReminder(_))), "the dialog is open");
    let (_, date) = field(&h, a_date);
    let (at, time) = field(&h, a_time);
    click_and_type(&mut h, at, "x");
    // A few more frames: the date mustn't take the keyboard back in any of them.
    h.run_steps(4);
    h.event(Event::Text("x".into()));
    h.run_steps(3);
    assert_eq!(field(&h, a_date).1, date, "the date is as it was");
    let time_now = field(&h, a_time).1;
    // (Typed where the click put the cursor, in the middle.)
    assert_eq!((time_now.matches('x').count(), time_now.replace('x', "")), (2, time), "the time took both keys");
}

/// A field of the window behind (the search box) that is given the keyboard while a dialog is
/// open doesn't keep it from the dialog's field (`keys::gate` takes it from under the dialog). This
/// passed before the fix too: it guards what the fix relies on.
#[test]
fn a_dialog_takes_the_keyboard_back_from_a_field_behind_it() {
    let mut h = testing::window_on(mailcraft_engine::sample::mailbox(testing::now(), 0), "view.mail");
    h.state_mut().run("ui.dialog", json!({"name": "goToDate"})).unwrap();
    h.run_steps(3);
    let before = match &h.state().dialog {
        Some(Dialog::GoToDate { text, .. }) => text.clone(),
        _ => panic!("the dialog is open"),
    };
    h.ctx.memory_mut(|m| m.request_focus(crate::chrome::search_box_id()));
    h.run_steps(3);
    h.event(Event::Text("7".into()));
    h.run_steps(3);
    match &h.state().dialog {
        Some(Dialog::GoToDate { text, .. }) => {
            assert_eq!(text.matches('7').count(), before.matches('7').count() + 1, "the dialog took the key: {before} → {text}")
        }
        _ => panic!("the dialog is open"),
    }
}
