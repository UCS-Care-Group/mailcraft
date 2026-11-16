// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shift+Delete (#1438): on Windows the window layer turns the press into a cut (the
//! system's cut key), so the permanent delete's shortcut arrives as a cut rather than a
//! key; and a Delete in the ribbon pressed with Shift held runs as Delete Permanently,
//! as in Outlook.

use egui::{Event, Key, Modifiers, PointerButton};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::people_view::testing;

/// The window on the sample's Inbox in list view, `id` selected, nothing having the keyboard.
fn selected() -> (testing::Window, Id) {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.run_steps(3);
    let id = w.state_mut().session.rows().iter().find(|r| r.under.is_none()).map(|r| r.id).unwrap();
    click(&mut w, "message", id);
    assert!(!w.ctx.egui_wants_keyboard_input(), "nothing has the keyboard");
    (w, id)
}

fn click(w: &mut testing::Window, kind: &str, id: u64) {
    let pos = w.state().hit.rect(kind, id).unwrap_or_else(|| panic!("no {kind} {id} drawn")).center();
    testing::click(w, pos, PointerButton::Primary, 1);
    w.run_steps(3);
}

/// What the window layer sends on Windows when Shift+Delete is pressed: Shift goes down,
/// and the press itself comes as a cut, not a key (#1438).
fn shift_delete(w: &mut testing::Window) {
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::SHIFT));
    w.step();
    w.input_mut().events.push(Event::Cut);
    w.step();
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    w.step();
}

/// The key as the window layer would deliver it on any platform but Windows.
fn shift_delete_key(w: &mut testing::Window) {
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::SHIFT));
    w.step();
    for pressed in [true, false] {
        w.input_mut().events.push(Event::Key { key: Key::Delete, physical_key: None, pressed, repeat: false, modifiers: Modifiers::SHIFT });
    }
    w.run_steps(2);
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    w.step();
}

#[test]
fn the_shift_delete_cut_raises_the_question_as_the_key_would() {
    let (mut w, id) = selected();
    shift_delete(&mut w);
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else {
        panic!("no question, only {:#?}", testing::dialog(&w));
    };
    let asked = &ask.text;
    assert!(asked.contains("permanently deleted"), "the question was not the permanent delete's: {asked}");
    click(&mut w, "questionAction", 0);
    assert!(w.state().session.mailbox.message(id).is_none(), "the yes takes it for good");
}

#[test]
fn a_shift_delete_key_raises_the_question_where_the_cut_does() {
    let (mut w, id) = selected();
    shift_delete_key(&mut w);
    assert_eq!(testing::dialog(&w), Some("confirm"));
    assert!(w.state().session.mailbox.message(id).is_some(), "the question is still open");
}

#[test]
fn a_cut_without_shift_is_the_text_fields_own_not_the_lists() {
    let (mut w, id) = selected();
    // Ctrl+X arrives as a cut as well, with no shift.
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::CTRL));
    w.step();
    w.input_mut().events.push(Event::Cut);
    w.step();
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    w.step();
    assert_eq!(testing::dialog(&w), None, "the cut did not ask for a delete");
    assert!(w.state().session.mailbox.message(id).is_some(), "nothing was taken");
}

#[test]
fn a_text_field_keeps_the_shift_delete_cut_for_itself() {
    let (mut w, _) = selected();
    click(&mut w, "searchBox", 0);
    w.input_mut().events.push(Event::Text("a search".to_string()));
    w.run_steps(2);
    // The field has the keyboard: Shift+Home selects its text, as the field's own keys do, and the
    // shift delete's cut then goes to the field, not the window.
    for pressed in [true, false] {
        w.input_mut().events.push(Event::Key { key: Key::Home, physical_key: None, pressed, repeat: false, modifiers: Modifiers::SHIFT });
    }
    w.run_steps(2);
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    w.step();
    shift_delete(&mut w);
    assert_eq!(testing::dialog(&w), None, "the cut went to the field, not the window");
    assert_eq!(w.state().search_text, "", "the field's selection was cut");
}

#[test]
fn a_delete_pressed_with_shift_held_is_the_permanent_one() {
    let (mut w, id) = selected();
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::SHIFT));
    w.step();
    click(&mut w, "ribbon", crate::ribbon::hit_id("mail.delete"));
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    w.step();
    let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &w.state().dialog else {
        panic!("no question, only {:#?}", testing::dialog(&w));
    };
    let asked = &ask.text;
    assert!(asked.contains("permanently deleted"), "the question was not the permanent delete's: {asked}");
    assert!(w.state().session.mailbox.message(id).is_some(), "the yes has not been given yet");
}

#[test]
fn a_delete_pressed_plain_runs_the_ordinary_delete() {
    let (mut w, id) = selected();
    click(&mut w, "ribbon", crate::ribbon::hit_id("mail.delete"));
    assert_eq!(testing::dialog(&w), None, "an ordinary delete from the Inbox asks nothing");
    assert_eq!(
        w.state().session.mailbox.message(id).and_then(|m| w.state().session.mailbox.folder(m.folder)).map(|f| f.kind),
        Some(FolderKind::Deleted),
        "it went to Deleted Items"
    );
}
