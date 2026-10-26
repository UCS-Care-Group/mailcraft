// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Escape and Enter go to one place (#406): a modal layer on top (a dialog, the recurrence editor
//! of an appointment, a window's question about its unsaved changes, an agent's question), a list
//! open, Send/Receive Progress, the widget with the keyboard, or the window last clicked or typed
//! in. Nothing under a dialog gets keys or typing, and the keyboard goes back once it closes. The
//! repeats of a held key that closed a dialog don't reach what is under it (a contact, a task, an
//! appointment, a search, a picked slot), nor press OK in what its press opened; they type new
//! lines where Enter does. Where both apply, a test is run with one press and with the key held.

use egui::Key;
use egui_kittest::kittest::{NodeT as _, Queryable};
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::people_view::testing::{self, Window};

/// One press, and the key held down.
const PRESSES: [(&str, fn(&mut Window, Key)); 2] = [("pressed", testing::key), ("held", testing::held)];

/// The sample mailbox with `module` in front, and no reminder over it.
fn window(module: &str) -> Window {
    let mut h = testing::window(module);
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    h.run_steps(3);
    h
}

fn run(h: &mut Window, command: &str, params: Value) -> Value {
    let answer = h.state_mut().execute(command, params).unwrap_or_else(|e| panic!("{command}: {e}"));
    h.run_steps(3);
    answer
}

fn dialog(h: &Window) -> Option<&'static str> {
    h.state().dialog.as_ref().map(Dialog::name)
}

/// A message box over the window, which one press of Escape or Enter closes.
fn refused(h: &mut Window) {
    h.state_mut().dialog = Some(Dialog::Refused { text: "Not now".into() });
    h.run_steps(3);
}

/// The open windows as `ui.inspect` has them.
fn windows(h: &Window) -> Vec<Value> {
    crate::windows::inspect(h.state()).as_array().cloned().unwrap_or_default()
}

/// A list is open in the New Contact dialog (Work, Home, Mobile beside an address): Escape closes
/// the list, and the dialog stays, with what was typed in it.
#[test]
fn escape_on_a_list_in_a_dialog_leaves_the_dialog() {
    for (how, press) in PRESSES {
        let mut h = window("view.people");
        run(&mut h, "ui.dialog", json!({"name": "contact"}));
        testing::text(&mut h, "Ada");
        h.get_all_by_role(egui::accesskit::Role::ComboBox).next().unwrap().click();
        h.run_steps(3);
        assert!(egui::Popup::is_any_open(&h.ctx), "{how}: the list is open");
        press(&mut h, Key::Escape);
        assert!(!egui::Popup::is_any_open(&h.ctx), "{how}: the list closed");
        assert_eq!(dialog(&h), Some("contact"), "{how}");
        assert!(h.query_all_by_value("Ada").next().is_some(), "{how}: what was typed is there");
    }
}

/// An appointment opened as a dialog, with its recurrence editor open: Escape closes the editor,
/// and the appointment stays.
#[test]
fn escape_on_the_recurrence_editor_leaves_the_dialog() {
    for (how, press) in PRESSES {
        let mut h = window("view.calendar");
        run(&mut h, "ui.dialog", json!({"name": "event", "title": "Planning"}));
        if let Some(Dialog::Form(form)) = &mut h.state_mut().dialog {
            form.event_mut().unwrap().open_recurrence();
        }
        h.run_steps(3);
        let editing = |h: &Window| matches!(&h.state().dialog, Some(Dialog::Form(form)) if form.modal());
        assert!(editing(&h), "{how}: the editor is open");
        press(&mut h, Key::Escape);
        assert!(!editing(&h), "{how}: the editor closed");
        assert_eq!(dialog(&h), Some("event"), "{how}");
    }
}

/// An appointment window has its recurrence editor open, and an appointment opened as a dialog
/// over it has its own open too: Escape closes the dialog's, and the window's stays.
#[test]
fn escape_on_the_recurrence_editor_of_a_dialog_leaves_the_one_of_a_window() {
    for (how, press) in PRESSES {
        let mut h = window("view.calendar");
        run(&mut h, "ui.openWindow", json!({"new": "appointment"}));
        let id = windows(&h)[0]["id"].as_u64().unwrap();
        h.state_mut().windows.form_mut(id).and_then(Form::event_mut).unwrap().open_recurrence();
        run(&mut h, "ui.dialog", json!({"name": "event", "title": "Planning"}));
        if let Some(Dialog::Form(form)) = &mut h.state_mut().dialog {
            form.event_mut().unwrap().open_recurrence();
        }
        h.run_steps(3);
        let in_dialog = |h: &Window| matches!(&h.state().dialog, Some(Dialog::Form(form)) if form.modal());
        let in_window = |h: &mut Window| h.state_mut().windows.form_mut(id).is_some_and(|form| form.modal());
        assert!(in_dialog(&h) && in_window(&mut h), "{how}: both editors are open");
        press(&mut h, Key::Escape);
        assert!(!in_dialog(&h), "{how}: the dialog's editor closed");
        assert_eq!(dialog(&h), Some("event"), "{how}");
        assert!(in_window(&mut h), "{how}: the window's editor stays");
    }
}

/// Two messages each in a window of their own: a held Escape closes the front one only, as one
/// press does.
#[test]
fn a_held_escape_closes_one_window() {
    for (how, press) in PRESSES {
        let mut h = window("view.mail");
        let ids: Vec<u64> = h.state().session.mailbox.messages.keys().take(2).copied().collect();
        for id in &ids {
            run(&mut h, "ui.openWindow", json!({"message": id}));
        }
        assert_eq!(windows(&h).len(), 2);
        press(&mut h, Key::Escape);
        assert_eq!(windows(&h).len(), 1, "{how}");
    }
}

/// `key` held down with `modifiers`, as [`testing::held`] has it.
fn held_with(h: &mut Window, key: Key, modifiers: egui::Modifiers) {
    for repeat in [false, true, true, true] {
        h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat, modifiers });
        h.run_steps(2);
    }
    h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    h.run_steps(2);
}

/// Two messages each in a window of their own: a held Ctrl+W closes the front one only, and its
/// repeats don't go on to the one behind it.
#[test]
fn a_held_ctrl_w_closes_one_window() {
    let mut h = window("view.mail");
    let ids: Vec<u64> = h.state().session.mailbox.messages.keys().take(2).copied().collect();
    for id in &ids {
        run(&mut h, "ui.openWindow", json!({"message": id}));
    }
    held_with(&mut h, Key::W, egui::Modifiers::COMMAND);
    assert_eq!(windows(&h).len(), 1);
}

/// Two new appointments each in a window of their own: a held Ctrl+Enter saves the front one
/// only, and the one behind it stays open, not saved.
#[test]
fn a_held_ctrl_enter_saves_one_appointment() {
    let mut h = window("view.calendar");
    let before = h.state().session.mailbox.events.len();
    for title in ["Lunch", "Dinner"] {
        run(&mut h, "ui.openWindow", json!({"new": "appointment"}));
        testing::text(&mut h, title);
    }
    held_with(&mut h, Key::Enter, egui::Modifiers::COMMAND);
    assert_eq!(windows(&h).len(), 1);
    assert_eq!(h.state().session.mailbox.events.len(), before + 1);
}

/// An appointment window with a change in it: Escape asks whether to save the change, and the
/// repeats of the held Escape don't answer the question before it has been read.
#[test]
fn a_held_escape_leaves_the_unsaved_question() {
    let mut h = window("view.calendar");
    run(&mut h, "ui.openWindow", json!({"new": "appointment"}));
    testing::text(&mut h, "Lunch");
    assert_eq!(windows(&h)[0]["dirty"], true);
    testing::held(&mut h, Key::Escape);
    let open = windows(&h);
    assert_eq!(open.len(), 1);
    assert_eq!(open[0]["asking"], true, "the question waits for an answer");
    // A new press answers it: stay.
    testing::key(&mut h, Key::Escape);
    assert_eq!(windows(&h)[0]["asking"], false);
    assert_eq!(windows(&h).len(), 1);
}

/// Send/Receive Progress shows over a message window: Escape closes Progress, and the window stays.
#[test]
fn escape_on_the_progress_leaves_the_window() {
    let mut h = window("view.mail");
    let id = *h.state().session.mailbox.messages.keys().next().unwrap();
    run(&mut h, "ui.openWindow", json!({"message": id}));
    run(&mut h, "ui.dialog", json!({"name": "syncProgress"}));
    testing::key(&mut h, Key::Escape);
    assert!(h.state().sync_panel.is_none());
    assert_eq!(windows(&h).len(), 1);
}

/// Send/Receive Progress takes only Escape: with it open, Enter opens the selected contact, as
/// it does without it.
#[test]
fn enter_goes_under_the_progress() {
    let mut h = window("view.people");
    let id = *h.state().session.mailbox.contacts.keys().next().unwrap();
    run(&mut h, "people.select", json!({"id": id}));
    run(&mut h, "ui.dialog", json!({"name": "syncProgress"}));
    assert!(h.state().sync_panel.is_some());
    testing::key(&mut h, Key::Enter);
    assert_eq!(dialog(&h), Some("editContact"));
}

/// A contact is selected, and Enter is held down to close a message box: its repeats don't open
/// the contact.
#[test]
fn a_held_enter_that_closes_a_dialog_leaves_the_contact() {
    let mut h = window("view.people");
    let id = *h.state().session.mailbox.contacts.keys().next().unwrap();
    run(&mut h, "people.select", json!({"id": id}));
    refused(&mut h);
    testing::held(&mut h, Key::Enter);
    assert_eq!(dialog(&h), None);
    // A new press opens it.
    testing::key(&mut h, Key::Enter);
    assert_eq!(dialog(&h), Some("editContact"));
}

/// The same with a task selected.
#[test]
fn a_held_enter_that_closes_a_dialog_leaves_the_task() {
    let mut h = window("view.tasks");
    let id = *h.state().session.mailbox.tasks.keys().next().unwrap();
    run(&mut h, "tasks.select", json!({"id": id}));
    refused(&mut h);
    testing::held(&mut h, Key::Enter);
    assert_eq!(dialog(&h), None);
    testing::key(&mut h, Key::Enter);
    assert!(dialog(&h).is_some());
}

/// The same with an appointment selected in the calendar.
#[test]
fn a_held_enter_that_closes_a_dialog_leaves_the_appointment() {
    let mut h = window("view.calendar");
    let today = h.state().session.today();
    let id = h.state().session.mailbox.events.iter().find(|(_, e)| e.start.date == today && e.recurrence.is_none()).map(|(id, _)| *id).unwrap();
    run(&mut h, "calendar.select", json!({"id": id, "day": today.iso()}));
    refused(&mut h);
    testing::held(&mut h, Key::Enter);
    assert_eq!((windows(&h).len(), dialog(&h)), (0, None));
    testing::key(&mut h, Key::Enter);
    assert_eq!(windows(&h).len() + usize::from(dialog(&h).is_some()), 1);
}

/// A search is going in Mail, and Escape is held down to close a message box: its repeats leave
/// the search.
#[test]
fn a_held_escape_that_closes_a_dialog_leaves_the_search() {
    let mut h = window("view.mail");
    run(&mut h, "view.search", json!({"query": "invoice"}));
    refused(&mut h);
    testing::held(&mut h, Key::Escape);
    assert_eq!(dialog(&h), None);
    assert_eq!(h.state().session.view.search, "invoice");
    // A new press ends it.
    testing::key(&mut h, Key::Escape);
    assert_eq!(h.state().session.view.search, "");
}

/// A stretch of the calendar is picked out, and Escape is held down to close a message box: its
/// repeats leave the pick.
#[test]
fn a_held_escape_that_closes_a_dialog_leaves_the_pick() {
    let mut h = window("view.calendar");
    run(&mut h, "view.day", json!({}));
    let day = u64::try_from(h.state().session.view.date.to_days()).unwrap();
    testing::key(&mut h, Key::ArrowDown);
    assert!(h.state().hit.rect("slot", day).is_some(), "a slot is picked");
    refused(&mut h);
    testing::held(&mut h, Key::Escape);
    assert_eq!(dialog(&h), None);
    assert!(h.state().hit.rect("slot", day).is_some(), "the pick stays");
    testing::key(&mut h, Key::Escape);
    assert!(h.state().hit.rect("slot", day).is_none());
}

/// A message window is open over File: Escape closes the window, and File stays until the next
/// press.
#[test]
fn escape_on_a_window_over_file_leaves_file() {
    for (how, press) in PRESSES {
        let mut h = window("view.mail");
        let id = *h.state().session.mailbox.messages.keys().next().unwrap();
        h.state_mut().ui.backstage = true;
        run(&mut h, "ui.openWindow", json!({"message": id}));
        press(&mut h, Key::Escape);
        assert_eq!(windows(&h).len(), 0, "{how}");
        assert!(h.state().ui.backstage, "{how}: File stays");
        press(&mut h, Key::Escape);
        assert!(!h.state().ui.backstage, "{how}");
    }
}

/// Escape and Enter count only when pressed alone: Shift+Escape doesn't close a message box,
/// and Shift+Enter doesn't press its button.
#[test]
fn only_the_key_alone_counts() {
    let mut h = window("view.mail");
    refused(&mut h);
    for key in [Key::Escape, Key::Enter] {
        for pressed in [true, false] {
            h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::SHIFT });
        }
        h.run_steps(2);
        assert_eq!(dialog(&h), Some("refused"), "Shift+{key:?}");
    }
    testing::key(&mut h, Key::Escape);
    assert_eq!(dialog(&h), None);
}

/// The pointer pressed at `from`, moved to `to` and let go there.
fn drag(h: &mut Window, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    h.input_mut().events.extend([egui::Event::PointerMoved(from), button(from, true)]);
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::PointerMoved(to));
    h.run_steps(2);
    h.input_mut().events.push(button(to, false));
    h.run_steps(2);
}

/// Text picked out in a label of the main window (a contact's notes): Escape lets go of it, as
/// egui has it do.
#[test]
fn escape_lets_go_of_text_picked_out_in_the_main_window() {
    let mut h = window("view.people");
    let id = *h.state().session.mailbox.contacts.keys().next().unwrap();
    run(&mut h, "people.select", json!({"id": id}));
    let notes = h
        .get_all_by_value("Prefers feedback as comments on the file, not in mail.")
        .find(|n| n.accesskit_node().role() == egui::accesskit::Role::Label)
        .unwrap()
        .rect();
    drag(&mut h, egui::pos2(notes.min.x + 2.0, notes.center().y), egui::pos2(notes.max.x - 2.0, notes.center().y));
    let picked = |h: &Window| h.ctx.with_plugin(|labels: &mut egui::text_selection::LabelSelectionState| labels.has_selection()).unwrap_or(false);
    assert!(picked(&h), "the notes are picked out");
    testing::key(&mut h, Key::Escape);
    assert!(!picked(&h));
}

fn focus(h: &Window) -> Option<egui::Id> {
    h.ctx.memory(|m| m.focused())
}

/// A new appointment in a window inside the main one, `Lunch` typed in its title; how many
/// appointments there were before it.
fn appointment(h: &mut Window) -> usize {
    let before = h.state().session.mailbox.events.len();
    run(h, "ui.openWindow", json!({"new": "appointment"}));
    testing::text(h, "Lunch");
    assert!(focus(h).is_some(), "the title has the keyboard");
    before
}

/// The title of a new appointment had the keyboard, or nothing had it, and a message box comes
/// up over the window: what is typed doesn't go into the title under it, and Enter, one press or
/// held, closes the message box and saves nothing.
#[test]
fn a_message_box_over_an_appointment_window_has_the_keys() {
    for keep in [true, false] {
        for (how, press) in PRESSES {
            let mut h = window("view.calendar");
            let before = appointment(&mut h);
            if !keep {
                let id = focus(&h).unwrap();
                h.ctx.memory_mut(|m| m.surrender_focus(id));
                h.run_steps(2);
            }
            refused(&mut h);
            testing::text(&mut h, "X");
            assert!(h.query_all_by_value("LunchX").next().is_none(), "{how}, {keep}: nothing typed under the box");
            press(&mut h, Key::Enter);
            assert_eq!(dialog(&h), None, "{how}, {keep}");
            assert_eq!((windows(&h).len(), h.state().session.mailbox.events.len()), (1, before), "{how}, {keep}: nothing saved");
        }
    }
}

/// Enter in the pass after a message box came up over a form is the box's: the form under it
/// doesn't save.
#[test]
fn enter_right_after_a_message_box_came_up_is_its() {
    let mut h = window("view.calendar");
    let before = appointment(&mut h);
    h.state_mut().dialog = Some(Dialog::Refused { text: "Not now".into() });
    h.step();
    testing::key(&mut h, Key::Enter);
    assert_eq!(dialog(&h), None);
    assert_eq!((windows(&h).len(), h.state().session.mailbox.events.len()), (1, before));
}

/// Window A has its recurrence editor open, and window B, with a title typed, is in front with
/// nothing focused: Enter is the editor's OK, and B isn't saved.
#[test]
fn enter_under_another_windows_editor_is_the_editors() {
    let mut h = window("view.calendar");
    let before = h.state().session.mailbox.events.len();
    run(&mut h, "ui.openWindow", json!({"new": "appointment"}));
    let a = windows(&h)[0]["id"].as_u64().unwrap();
    run(&mut h, "ui.openWindow", json!({"new": "appointment"}));
    testing::text(&mut h, "Dinner");
    let id = focus(&h).unwrap();
    h.ctx.memory_mut(|m| m.surrender_focus(id));
    h.state_mut().windows.form_mut(a).and_then(Form::event_mut).unwrap().open_recurrence();
    h.run_steps(3);
    testing::key(&mut h, Key::Enter);
    assert!(!h.state_mut().windows.form_mut(a).is_some_and(|f| f.modal()), "the editor took OK");
    assert_eq!((windows(&h).len(), h.state().session.mailbox.events.len()), (2, before), "B isn't saved");
}

/// A held Enter in New Contact's notes, a box of several lines, types four new lines: the press
/// and each repeat.
#[test]
fn a_held_enter_types_new_lines_in_notes() {
    let mut h = window("view.people");
    run(&mut h, "ui.dialog", json!({"name": "contact"}));
    let notes = crate::forms::kit::field_id("notes");
    h.ctx.memory_mut(|m| m.request_focus(notes));
    h.run_steps(2);
    testing::text(&mut h, "one");
    testing::held(&mut h, Key::Enter);
    testing::text(&mut h, "two");
    assert!(h.query_all_by_value("one\n\n\n\ntwo").next().is_some());
    assert_eq!(dialog(&h), Some("contact"));
}

/// A held Enter in the body of a message being written types four new lines.
#[test]
fn a_held_enter_types_new_lines_in_a_message() {
    let mut h = window("view.mail");
    run(&mut h, "mail.new", json!({"to": "maya@lumen.example"}));
    run(&mut h, "compose.set", json!({"body": ""}));
    h.state_mut().body_focus_wanted = true;
    h.run_steps(3);
    assert!(crate::compose::editor::has_focus(&h.ctx));
    testing::text(&mut h, "Hi");
    testing::held(&mut h, Key::Enter);
    testing::text(&mut h, "there");
    assert_eq!(h.state().session.compose.as_ref().map(|c| c.body.plain()).unwrap_or_default(), "Hi\n\n\n\nthere");
}

/// A held Enter in New Contact's name saves one contact and closes the dialog, and its repeats
/// open nothing after it.
#[test]
fn a_held_enter_in_a_one_line_field_saves_once() {
    let mut h = window("view.people");
    let before = h.state().session.mailbox.contacts.len();
    run(&mut h, "ui.dialog", json!({"name": "contact"}));
    testing::text(&mut h, "Ada Probe");
    testing::held(&mut h, Key::Enter);
    assert_eq!(dialog(&h), None);
    assert_eq!(h.state().session.mailbox.contacts.len(), before + 1);
}

/// A held Enter that opens the selected contact, task or appointment: its repeats don't press
/// OK in what opened.
#[test]
fn a_held_enter_that_opens_something_leaves_it_open() {
    for (module, select) in [("view.people", "people.select"), ("view.tasks", "tasks.select")] {
        let mut h = window(module);
        let mb = &h.state().session.mailbox;
        let id = if module == "view.people" { *mb.contacts.keys().next().unwrap() } else { *mb.tasks.keys().next().unwrap() };
        run(&mut h, select, json!({"id": id}));
        testing::held(&mut h, Key::Enter);
        assert!(dialog(&h).is_some(), "{module}: it stays open");
    }
    let mut h = window("view.calendar");
    let today = h.state().session.today();
    let id = h.state().session.mailbox.events.iter().find(|(_, e)| e.start.date == today && e.recurrence.is_none()).map(|(id, _)| *id).unwrap();
    run(&mut h, "calendar.select", json!({"id": id, "day": today.iso()}));
    testing::held(&mut h, Key::Enter);
    assert_eq!(windows(&h).len() + usize::from(dialog(&h).is_some()), 1, "the appointment stays open");
}

/// A held Enter in the entry of a picked stretch of the calendar makes one appointment.
#[test]
fn a_held_enter_in_a_calendar_slot_makes_one_appointment() {
    let mut h = window("view.calendar");
    run(&mut h, "view.day", json!({}));
    let before = h.state().session.mailbox.events.len();
    testing::key(&mut h, Key::ArrowDown);
    testing::text(&mut h, "Standup");
    h.run_steps(2);
    testing::held(&mut h, Key::Enter);
    assert_eq!(h.state().session.mailbox.events.len(), before + 1);
    assert_eq!((windows(&h).len(), dialog(&h)), (0, None));
}

/// Two presses of Enter in the pass a message box closes in: the second doesn't open the contact
/// under it.
#[test]
fn two_presses_in_the_pass_a_dialog_closes_in() {
    let mut h = window("view.people");
    let id = *h.state().session.mailbox.contacts.keys().next().unwrap();
    run(&mut h, "people.select", json!({"id": id}));
    refused(&mut h);
    for pressed in [true, false, true, false] {
        h.input_mut().events.push(egui::Event::Key { key: Key::Enter, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
    }
    h.step();
    h.run_steps(2);
    assert_eq!(dialog(&h), None);
}

/// The search box has the keyboard, and a message box comes up: the box gives it up, and has it
/// again once the message box closes, so that the next Escape is the search box's.
#[test]
fn the_keyboard_goes_back_after_a_message_box() {
    let mut h = window("view.mail");
    run(&mut h, "mail.new", json!({"subject": "Half written"}));
    run(&mut h, "ui.focusSearch", json!({}));
    testing::text(&mut h, "budget");
    let search = crate::chrome::search_box_id();
    assert_eq!(focus(&h), Some(search));
    refused(&mut h);
    assert_eq!(focus(&h), None, "not under the message box");
    testing::key(&mut h, Key::Escape);
    assert_eq!((dialog(&h), focus(&h)), (None, Some(search)));
    // (egui lets a widget keep the keyboard on Escape from its second pass with it.)
    h.step();
    testing::key(&mut h, Key::Escape);
    assert_eq!(h.state().session.view.search, "");
    assert!(h.state().session.compose.is_some(), "the message being written stays");
}

/// A window that opens while a message box is up stays under it, out of reach: Escape closes the
/// message box, and then the window, which came up after the main window was last used.
#[test]
fn a_window_that_opens_under_a_dialog_waits() {
    let mut h = window("view.mail");
    refused(&mut h);
    let message = *h.state().session.mailbox.messages.keys().next().unwrap();
    run(&mut h, "ui.openWindow", json!({"message": message}));
    let id = windows(&h)[0]["id"].as_u64().unwrap();
    assert!(!h.ctx.memory(|m| m.allows_interaction(crate::windows::embedded_layer(id))));
    testing::key(&mut h, Key::Escape);
    assert_eq!((dialog(&h), windows(&h).len()), (None, 1));
    testing::key(&mut h, Key::Escape);
    assert_eq!(windows(&h).len(), 0);
}

/// A list is open in New Contact, and an agent's question comes up over both: Escape is the
/// question's, and the list stays open for the next one.
#[test]
fn a_list_under_an_agents_question_waits() {
    let mut h = window("view.people");
    run(&mut h, "ui.dialog", json!({"name": "contact"}));
    h.get_all_by_role(egui::accesskit::Role::ComboBox).next().unwrap().click();
    h.run_steps(3);
    assert!(egui::Popup::is_any_open(&h.ctx));
    h.state_mut().execute("mail.new", json!({"to": "x@attacker.example", "subject": "Re: Invoice"})).unwrap();
    let (req, rx) = crate::control::ControlRequest::new("engine.execute", json!({"command": "compose.send"}));
    let ctx = h.ctx.clone();
    let _ = crate::control::handle(h.state_mut(), &ctx, &req.from_agent(vec![]));
    h.run_steps(3);
    assert_eq!(h.state().asking.len(), 1);
    testing::key(&mut h, Key::Escape);
    assert!(h.state().asking.is_empty() && rx.try_recv().is_ok_and(|v| v["ok"] == false), "the question said no");
    assert!(egui::Popup::is_any_open(&h.ctx), "the list stays");
    testing::key(&mut h, Key::Escape);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert_eq!(dialog(&h), Some("contact"));
}

/// In a dialog, Enter on the button that has the keyboard presses that button: Cancel, not OK.
#[test]
fn enter_on_a_button_with_the_keyboard_presses_it() {
    let mut h = window("view.people");
    let before = h.state().session.mailbox.contacts.len();
    run(&mut h, "ui.dialog", json!({"name": "contact"}));
    testing::text(&mut h, "Ada Probe");
    h.get_by_label("Cancel").focus();
    h.run_steps(2);
    testing::key(&mut h, Key::Enter);
    assert_eq!(dialog(&h), None);
    assert_eq!(h.state().session.mailbox.contacts.len(), before, "nothing saved");
}

/// A message window is open, and the owner clicks into the search box of the main window and
/// types: the main window is the active one now, so Escape is the search box's, and the window
/// stays.
#[test]
fn escape_in_a_field_of_the_main_window_is_its_with_a_window_open() {
    let mut h = window("view.mail");
    let message = *h.state().session.mailbox.messages.keys().next().unwrap();
    run(&mut h, "ui.openWindow", json!({"message": message}));
    let r = h.ctx.read_response(crate::chrome::search_box_id()).unwrap().rect;
    testing::click(&mut h, r.center(), egui::PointerButton::Primary, 1);
    testing::text(&mut h, "budget");
    assert_eq!(h.state().session.view.search, "budget");
    testing::key(&mut h, Key::Escape);
    assert_eq!(h.state().session.view.search, "");
    assert_eq!(windows(&h).len(), 1);
}

/// A new appointment is open in a window inside the main one, with a title typed; the owner
/// clicks a row of the main window's list: Enter is the main window's now (it opens what was
/// clicked, but a message), and the appointment isn't saved.
#[test]
fn enter_after_a_click_on_the_main_list_is_the_main_windows() {
    for (module, kind) in [("view.mail", "message"), ("view.people", "contact"), ("view.tasks", "task"), ("view.calendar", "event")] {
        let mut h = window(module);
        let before = appointment(&mut h);
        let id = windows(&h)[0]["id"].as_u64().unwrap();
        let area = h.ctx.memory(|m| m.area_rect(crate::windows::embedded_layer(id).id)).unwrap();
        let mb = &h.state().session.mailbox;
        let ids: Vec<u64> = match kind {
            "message" => mb.messages.keys().copied().collect(),
            "contact" => mb.contacts.keys().copied().collect(),
            "task" => mb.tasks.keys().copied().collect(),
            _ => mb.events.keys().copied().collect(),
        };
        let row = ids.iter().find_map(|id| h.state().hit.rect(kind, *id).filter(|r| !area.expand(8.0).contains(r.center()))).unwrap();
        testing::click(&mut h, row.center(), egui::PointerButton::Primary, 1);
        testing::key(&mut h, Key::Enter);
        assert_eq!(h.state().session.mailbox.events.len(), before, "{module}: the appointment isn't saved");
        assert!(windows(&h).iter().any(|w| w["id"].as_u64() == Some(id)), "{module}: its window stays");
        if kind != "message" {
            let opened = windows(&h).len() - 1 + usize::from(dialog(&h).is_some());
            assert_eq!(opened, 1, "{module}: what was clicked opened");
        }
    }
}

/// The source of the crate without its tests: the files of their own, and the modules of tests
/// inside a file.
fn sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name =
                path.strip_prefix(root).unwrap().components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
            if path.is_dir() {
                if !name.ends_with("tests") {
                    walk(&path, root, out);
                }
            } else if name.ends_with(".rs") && !name.ends_with("tests.rs") {
                out.push((name, without_tests(&std::fs::read_to_string(&path).unwrap())));
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

/// `src` without its comments and its inline modules of tests.
fn without_tests(src: &str) -> String {
    let mut out = String::new();
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        let inline = |l: &&str| {
            let l = l.trim();
            (l.starts_with("mod ") || l.starts_with("pub(crate) mod ")) && l.ends_with('{')
        };
        if line.trim() == "#[cfg(test)]" && lines.peek().is_some_and(inline) {
            let mut depth = 0i64;
            for l in lines.by_ref() {
                depth += l.matches('{').count() as i64 - l.matches('}').count() as i64;
                if depth <= 0 {
                    break;
                }
            }
        } else if !line.trim_start().starts_with("//") {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Escape and Enter are read through [`crate::keys`] only, which gives each press to one place
/// (#406): by the gate's own functions, named here, and nothing else of keys.rs. The few reads of
/// the keys themselves left are by a widget that has the keyboard, in a list that is open, with a
/// modifier held, or no reads at all: each is named here by its line, so that a new read is
/// caught, also where it takes the place of one of these.
#[test]
fn escape_and_enter_are_read_through_the_gate_only() {
    let allowed: &[(&str, &str)] = &[
        // The names of the keys the control channel presses.
        ("control.rs", r#""enter" | "return" => Some(egui::Key::Enter),"#),
        ("control.rs", r#""esc" | "escape" => Some(egui::Key::Escape),"#),
        // The search box, with the list under it open: Enter takes the one an arrow chose.
        ("search_suggest.rs", "if self.selected.is_some() && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {"),
        // The search box has the keyboard: a held Enter is one press in it.
        ("chrome.rs", "ui.input_mut(|i| crate::keys::drop_repeats(i, &[egui::Key::Enter, egui::Key::Tab]));"),
        // The body of a message being written has the keyboard: Enter is a new line.
        ("compose/editor.rs", r#"Key::Enter if !m.command && !m.alt => ask("compose.type", json!({"text": "\n"})),"#),
        // The search box of the Move menu, a list that is open: Escape clears it, Enter moves.
        ("ribbon.rs", "i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),"),
        ("ribbon.rs", "searching && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),"),
        // Ctrl+Enter.
        ("windows.rs", "if take_key(i, Modifiers::COMMAND, Key::Enter) | take_key(i, Modifiers::ALT, Key::S) {"),
        // The names of the keys in the shortcuts, and the key loop, which leaves Escape alone to
        // what the gate hands it to.
        ("keys.rs", r#"Key::Enter => "Enter","#),
        ("keys.rs", r#"Key::Escape => "Escape","#),
        ("keys.rs", "if key == Key::Escape && modifiers.is_none() {"),
    ];
    let reads = |line: &str| {
        ["Key::Escape", "Key::Enter"]
            .iter()
            .any(|key| line.match_indices(key).any(|(at, _)| !line[at + key.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')))
    };
    // The gate is these functions of keys.rs; the rest of keys.rs (the shortcuts, the key loop)
    // reads through them like the other files.
    let gate = [
        "alone",
        "give_back",
        "gate",
        "ask",
        "escape",
        "take_escape",
        "take_escape_in",
        "take_escape_any",
        "give_back_on_escape",
        "enter",
        "take_enter",
        "take_enter_out",
        "enter_alone",
        "take_enter_alone",
    ];
    let mut gate_found = Vec::new();
    let mut found = Vec::new();
    let mut wrong = Vec::new();
    for (name, src) in sources() {
        // The function a line is in, when it is one at the top of the file.
        let mut within = "";
        for line in src.lines() {
            if !line.starts_with([' ', '}']) && !line.is_empty() {
                within = ["fn ", "pub fn ", "pub(crate) fn "]
                    .iter()
                    .find_map(|p| line.strip_prefix(p))
                    .and_then(|f| f.split(['(', '<']).next())
                    .unwrap_or("");
                if name == "keys.rs"
                    && let Some(f) = gate.iter().find(|f| **f == within)
                {
                    gate_found.push(*f);
                }
            }
            let line = line.trim();
            if !reads(line) || name == "keys.rs" && gate.contains(&within) {
                continue;
            }
            if allowed.contains(&(name.as_str(), line)) {
                found.push((name.clone(), line.to_string()));
            } else {
                wrong.push(format!("{name}: {line}"));
            }
        }
    }
    gate_found.sort_unstable();
    let mut named = gate.to_vec();
    named.sort_unstable();
    assert_eq!(gate_found, named, "each function of the gate is in keys.rs, once");
    assert!(
        wrong.is_empty(),
        "Escape or Enter read outside the gate; ask crate::keys (escape, enter…) instead, or add the line to the list: {wrong:?}"
    );
    for (file, line) in allowed {
        let times = found.iter().filter(|(f, l)| f == file && l == line).count();
        assert_eq!(times, 1, "{file}: {line}");
    }
}

/// One press of `key` in the next pass alone.
fn press_now(h: &mut Window, key: Key) {
    for pressed in [true, false] {
        h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE });
    }
    h.step();
}

/// Two message windows, and Escape in two passes one after the other (an agent's `ui.key`, a
/// quick double press): each closes one, though egui still shows the first in the pass after.
#[test]
fn escape_in_the_pass_after_a_window_closed_closes_the_next() {
    let mut h = window("view.mail");
    let ids: Vec<u64> = h.state().session.mailbox.messages.keys().take(2).copied().collect();
    for id in &ids {
        run(&mut h, "ui.openWindow", json!({"message": id}));
    }
    press_now(&mut h, Key::Escape);
    press_now(&mut h, Key::Escape);
    h.run_steps(3);
    assert_eq!(windows(&h).len(), 0);
}

/// The same with the recurrence editor of an appointment in a dialog: the first Escape closes the
/// editor, the next, in the pass after, the dialog.
#[test]
fn escape_in_the_pass_after_the_editor_closed_closes_the_dialog() {
    let mut h = window("view.calendar");
    run(&mut h, "ui.dialog", json!({"name": "event", "title": "Planning"}));
    if let Some(Dialog::Form(form)) = &mut h.state_mut().dialog {
        form.event_mut().unwrap().open_recurrence();
    }
    h.run_steps(3);
    press_now(&mut h, Key::Escape);
    assert!(matches!(&h.state().dialog, Some(Dialog::Form(form)) if !form.modal()), "the editor closed");
    press_now(&mut h, Key::Escape);
    h.run_steps(3);
    assert_eq!(dialog(&h), None);
}

/// The search box had the keyboard, and the message box over it is closed with Enter on its OK
/// button, reached with the keyboard: the search box has the keyboard again.
#[test]
fn the_keyboard_goes_back_after_ok_pressed_with_the_keyboard() {
    let mut h = window("view.mail");
    run(&mut h, "mail.new", json!({"subject": "Half written"}));
    run(&mut h, "ui.focusSearch", json!({}));
    testing::text(&mut h, "budget");
    refused(&mut h);
    h.get_by_label("OK").focus();
    h.run_steps(2);
    assert!(focus(&h).is_some(), "OK has the keyboard");
    testing::key(&mut h, Key::Enter);
    assert_eq!((dialog(&h), focus(&h)), (None, Some(crate::chrome::search_box_id())));
}

/// A message window is open, a search is going in Mail, and the owner clicks a message of the
/// list: the main window is the active one, so Escape ends the search, and the window in front
/// stays open.
#[test]
fn escape_after_a_click_on_the_main_list_is_the_main_windows() {
    let mut h = window("view.mail");
    run(&mut h, "view.search", json!({"query": "the"}));
    let ids: Vec<u64> = h.state().session.mailbox.messages.keys().copied().collect();
    run(&mut h, "ui.openWindow", json!({"message": ids[0]}));
    let id = windows(&h)[0]["id"].as_u64().unwrap();
    let area = h.ctx.memory(|m| m.area_rect(crate::windows::embedded_layer(id).id)).unwrap();
    let row = ids.iter().find_map(|id| h.state().hit.rect("message", *id).filter(|r| !area.expand(8.0).contains(r.center()))).unwrap();
    testing::click(&mut h, row.center(), egui::PointerButton::Primary, 1);
    assert_eq!(focus(&h), None);
    testing::key(&mut h, Key::Escape);
    assert_eq!(h.state().session.view.search, "");
    assert_eq!(windows(&h).len(), 1);
}

/// In a browser Reminders is a window inside the main one, which reads neither Escape nor Enter:
/// a click on it leaves them with the message window that had them, and Escape closes it.
#[test]
fn a_click_on_reminders_leaves_the_keys_where_they_were() {
    let mut h = testing::window("view.mail");
    h.run_steps(4);
    let reminders = crate::reminders::embedded_layer();
    assert!(h.ctx.memory(|m| m.areas().is_visible(&reminders)), "a reminder is showing");
    let message = *h.state().session.mailbox.messages.keys().next().unwrap();
    run(&mut h, "ui.openWindow", json!({"message": message}));
    let id = windows(&h)[0]["id"].as_u64().unwrap();
    let rect = h.ctx.memory(|m| m.area_rect(reminders.id)).unwrap();
    testing::click(&mut h, egui::pos2(rect.center().x, rect.min.y + 8.0), egui::PointerButton::Primary, 1);
    assert_eq!(h.ctx.layer_id_at(egui::pos2(rect.center().x, rect.min.y + 8.0)), Some(reminders), "the click was on Reminders");
    assert_eq!(crate::keys::active(&h.ctx), crate::windows::embedded_layer(id));
    testing::key(&mut h, Key::Escape);
    assert_eq!(windows(&h).len(), 0);
}

/// The title of a new appointment had the keyboard, and the message box over it is closed with
/// a click on its OK: Enter in the very next frame is the title's, and saves the appointment, as
/// it does a frame later. In its description, a box of several lines, it types a new line.
#[test]
fn enter_right_after_ok_was_clicked_is_the_fields() {
    for description in [false, true] {
        let mut h = window("view.calendar");
        let before = appointment(&mut h);
        if description {
            let id = windows(&h)[0]["id"].as_u64().unwrap();
            let field = crate::forms::kit::hosted(crate::forms::kit::Host { id, fill: false }, || crate::forms::kit::field_id("description"));
            h.ctx.memory_mut(|m| m.request_focus(field));
            h.run_steps(2);
            testing::text(&mut h, "Bring");
            assert_eq!(focus(&h), Some(field));
        }
        refused(&mut h);
        let ok = h.get_by_label("OK").rect().center();
        let button =
            |pressed| egui::Event::PointerButton { pos: ok, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        h.input_mut().events.extend([egui::Event::PointerMoved(ok), button(true)]);
        h.step();
        h.input_mut().events.push(button(false));
        h.step();
        assert_eq!(dialog(&h), None, "the click closed the message box");
        for pressed in [true, false] {
            h.input_mut().events.push(egui::Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
        }
        h.step();
        h.run_steps(4);
        if description {
            assert_eq!((windows(&h).len(), h.state().session.mailbox.events.len()), (1, before), "nothing saved");
            assert!(h.query_all_by_value("Bring\n").next().is_some(), "a new line in the description");
        } else {
            assert_eq!((windows(&h).len(), h.state().session.mailbox.events.len()), (0, before + 1), "Enter saved the appointment");
        }
    }
}

/// The same with Escape, in the very next frame and the one after: it takes the keyboard from
/// the title, as it does later, and the window asks about the change in it. The search box keeps
/// the keyboard on Escape: there it ends the search, and a message being written stays.
#[test]
fn escape_right_after_ok_was_clicked_is_as_a_frame_later() {
    for search in [false, true] {
        for wait in [0, 1] {
            let mut h = window(if search { "view.mail" } else { "view.calendar" });
            if search {
                run(&mut h, "mail.new", json!({"subject": "Half written"}));
                run(&mut h, "ui.focusSearch", json!({}));
                testing::text(&mut h, "budget");
            } else {
                appointment(&mut h);
            }
            refused(&mut h);
            let ok = h.get_by_label("OK").rect().center();
            let button =
                |pressed| egui::Event::PointerButton { pos: ok, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
            h.input_mut().events.extend([egui::Event::PointerMoved(ok), button(true)]);
            h.step();
            h.input_mut().events.push(button(false));
            h.step();
            assert_eq!(dialog(&h), None, "the click closed the message box");
            h.run_steps(wait);
            for pressed in [true, false] {
                h.input_mut().events.push(egui::Event::Key {
                    key: Key::Escape,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            h.step();
            h.run_steps(4);
            if search {
                let state = h.state();
                assert_eq!((state.session.view.search.as_str(), state.session.compose.is_some()), ("", true), "{wait}: the search ended");
            } else {
                assert_eq!(windows(&h)[0]["asking"], json!(true), "{wait}: the window asks about its change");
            }
        }
    }
}

/// A message being written, `Half a thought` typed in its body, which has the keyboard: a change
/// that isn't saved, which Escape, the ✕ and New Email ask about first (#442). The body, with the
/// signature under what was typed.
fn writing(h: &mut Window) -> String {
    run(h, "mail.new", json!({"to": "ada@example.org", "subject": "Mine"}));
    h.state_mut().body_focus_wanted = true;
    h.run_steps(3);
    assert!(crate::compose::editor::has_focus(&h.ctx), "the body has the keyboard");
    testing::text(h, "Half a thought");
    let compose = h.state().session.compose.as_ref().unwrap();
    assert!(compose.dirty && compose.body.plain().starts_with("Half a thought"));
    compose.body.plain()
}

/// The message is still being written with `body` in it, and nothing of it is in Drafts: none of
/// the question's answers ran.
fn untouched(h: &Window, body: &str, how: &str) {
    let compose = h.state().session.compose.as_ref().map(|c| (c.subject.clone(), c.body.plain()));
    assert_eq!(compose, Some(("Mine".to_string(), body.to_string())), "{how}");
    let mb = &h.state().session.mailbox;
    let drafts = mb.folder_of_kind(mb.default_account().unwrap().id, mailcraft_model::FolderKind::Drafts).unwrap();
    assert!(!mb.messages.values().any(|m| m.folder == drafts && m.subject == "Mine"), "{how}: nothing went to Drafts");
}

/// `key` pressed with `modifiers` and then repeated `repeats` times, a frame at a time, as
/// [`held_with`] has it, and the dialog that is open after each frame.
fn seen_while_held(h: &mut Window, key: Key, modifiers: egui::Modifiers, repeats: usize) -> Vec<Option<&'static str>> {
    let mut seen = Vec::new();
    for repeat in std::iter::once(false).chain(std::iter::repeat_n(true, repeats)) {
        h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat, modifiers });
        for _ in 0..2 {
            h.step();
            seen.push(dialog(h));
        }
    }
    h.input_mut().events.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
    for _ in 0..2 {
        h.step();
        seen.push(dialog(h));
    }
    seen
}

/// How many times Unsaved Message came up in what was `seen`.
fn times_asked(seen: &[Option<&'static str>]) -> usize {
    std::iter::once(None)
        .chain(seen.iter().copied())
        .zip(seen)
        .filter(|(before, now)| *before != Some("discardDraft") && **now == Some("discardDraft"))
        .count()
}

/// Escape in a message being written that has a change, from its body or its Subject line: the
/// question comes up once, and the repeats of a held Escape don't answer it (Cancel, Discard and
/// Save Draft each leave a mark). The press is the main window's, so the question, a layer of its
/// own, isn't given it in the pass it comes up in. (That the main window takes the press only
/// keeps it from what else of the main window asks later in the pass.)
#[test]
fn escape_in_a_changed_message_asks_once() {
    for subject in [false, true] {
        for repeats in [0, 3] {
            let how = format!("{} with {repeats} repeats", if subject { "Subject" } else { "body" });
            let mut h = window("view.mail");
            let body = writing(&mut h);
            if subject {
                h.query_all_by_value("Mine").find(|n| n.accesskit_node().role() == egui::accesskit::Role::TextInput).unwrap().click();
                h.run_steps(2);
                assert!(focus(&h).is_some() && !crate::compose::editor::has_focus(&h.ctx), "the Subject line has the keyboard");
            }
            let seen = seen_while_held(&mut h, Key::Escape, egui::Modifiers::NONE, repeats);
            assert_eq!(times_asked(&seen), 1, "{how}: {seen:?}");
            assert_eq!(dialog(&h), Some("discardDraft"), "{how}: still asking");
            untouched(&h, &body, &how);
        }
    }
}

/// Escape on the question is its Cancel: the message stays, as it was. Also in the very next
/// frame after the Escape that asked.
#[test]
fn escape_on_the_question_is_its_cancel() {
    for (how, press) in PRESSES {
        let mut h = window("view.mail");
        let body = writing(&mut h);
        testing::key(&mut h, Key::Escape);
        assert_eq!(dialog(&h), Some("discardDraft"), "{how}");
        press(&mut h, Key::Escape);
        assert_eq!(dialog(&h), None, "{how}: cancelled, and not asked again");
        untouched(&h, &body, how);
    }
    let mut h = window("view.mail");
    let body = writing(&mut h);
    press_now(&mut h, Key::Escape);
    assert_eq!(dialog(&h), Some("discardDraft"));
    press_now(&mut h, Key::Escape);
    h.run_steps(3);
    assert_eq!(dialog(&h), None, "a frame apart: cancelled");
    untouched(&h, &body, "a frame apart");
}

/// A held Enter on the question presses none of its buttons (Save Draft is the one in colour).
/// The question reads no Enter, and none of its buttons has the keyboard: this holds by how it is
/// made, and keeps it so.
#[test]
fn a_held_enter_leaves_the_question() {
    let mut h = window("view.mail");
    let body = writing(&mut h);
    testing::key(&mut h, Key::Escape);
    testing::held(&mut h, Key::Enter);
    assert_eq!(dialog(&h), Some("discardDraft"));
    untouched(&h, &body, "held Enter");
}

/// Enter is held in the body when the question comes up (the ✕ clicked meanwhile): once the
/// question is up, the repeats type no more new lines under it (egui takes the keyboard from what
/// is under a modal layer), and answer nothing.
#[test]
fn a_held_enter_in_the_body_stops_at_the_question() {
    let mut h = window("view.mail");
    let body = writing(&mut h);
    let enter = |pressed, repeat| egui::Event::Key { key: Key::Enter, physical_key: None, pressed, repeat, modifiers: egui::Modifiers::NONE };
    h.input_mut().events.push(enter(true, false));
    h.run_steps(2);
    // (Drawn once before the repeats go on: a repeat in the pass the ✕ is clicked in may still be
    // the body's, as in any pass before the question is up.)
    let _ = h.state_mut().run("compose.close", json!({}));
    h.step();
    for _ in 0..3 {
        h.input_mut().events.push(enter(true, true));
        h.run_steps(2);
    }
    h.input_mut().events.push(enter(false, false));
    h.run_steps(2);
    assert_eq!(dialog(&h), Some("discardDraft"));
    untouched(&h, &body.replacen("Half a thought", "Half a thought\n", 1), "the one new line of the press");
}

/// The ✕ on the message's bar and New Email, from the ribbon or with Ctrl+N (held too), ask as
/// Escape does: once, a held Enter answers nothing, and Escape is the question's Cancel.
#[test]
fn the_close_button_and_new_email_ask_as_escape_does() {
    // What a click on the ✕ and on the ribbon's New Email runs, as the owner.
    for command in ["compose.close", "mail.new"] {
        let mut h = window("view.mail");
        let body = writing(&mut h);
        let _ = h.state_mut().run(command, json!({}));
        h.run_steps(3);
        assert_eq!(dialog(&h), Some("discardDraft"), "{command}");
        testing::held(&mut h, Key::Enter);
        assert_eq!(dialog(&h), Some("discardDraft"), "{command}: Enter answers nothing");
        testing::key(&mut h, Key::Escape);
        assert_eq!(dialog(&h), None, "{command}: Escape is Cancel");
        untouched(&h, &body, command);
    }
    for repeats in [0, 3] {
        let mut h = window("view.mail");
        let body = writing(&mut h);
        let seen = seen_while_held(&mut h, Key::N, egui::Modifiers::COMMAND, repeats);
        assert_eq!(times_asked(&seen), 1, "Ctrl+N, {repeats} repeats: {seen:?}");
        assert_eq!(dialog(&h), Some("discardDraft"));
        untouched(&h, &body, "Ctrl+N");
        testing::key(&mut h, Key::Escape);
        assert_eq!(dialog(&h), None, "Ctrl+N, {repeats} repeats: Escape is Cancel");
        untouched(&h, &body, "Ctrl+N, then Escape");
    }
}

// ---- The lists under a field: egui's own, which the gate sees ----

/// Where the list under a field is, as egui last drew it, when one is open.
fn list_under_field(h: &Window) -> Option<egui::Rect> {
    h.ctx.memory(|m| m.areas().visible_layer_ids().into_iter().filter(|l| l.order == egui::Order::Foreground).find_map(|l| m.area_rect(l.id)))
}

/// What the line of recipients being typed in says: a new message's To, or a meeting's Required.
fn recipients_typed(h: &Window) -> String {
    match &h.state().dialog {
        Some(Dialog::Form(Form::Event(form))) => form.command().1["attendees"].as_str().unwrap_or_default().to_string(),
        _ => h.state().session.compose.as_ref().map(|c| c.to.clone()).unwrap_or_default(),
    }
}

/// The two lines of recipients, each with "pri" typed in it and the suggestions (Priya Nair)
/// open under it: a new message's To line, and a new meeting's Required line (Tab from the
/// title).
const RECIPIENT_LINES: [(&str, fn(&mut Window)); 2] = [("To", to_line), ("Required", required_line)];

/// The same two lines with the keyboard in them, nothing typed yet.
const EMPTY_RECIPIENT_LINES: [(&str, fn(&mut Window)); 2] = [("To", to_line_empty), ("Required", required_line_empty)];

fn to_line(h: &mut Window) {
    to_line_empty(h);
    typed_in_line(h);
}

fn required_line(h: &mut Window) {
    required_line_empty(h);
    typed_in_line(h);
}

fn to_line_empty(h: &mut Window) {
    run(h, "mail.new", json!({}));
}

fn required_line_empty(h: &mut Window) {
    run(h, "ui.dialog", json!({"name": "meeting"}));
    testing::key(h, Key::Tab);
}

fn typed_in_line(h: &mut Window) {
    assert!(focus(h).is_some(), "a line has the keyboard");
    testing::text(h, "pri");
    // (The list keeps its keys from the frame after it came up.)
    h.run_steps(2);
    assert_eq!(recipients_typed(h), "pri");
    assert!(list_under_field(h).is_some(), "recipients are suggested");
}

/// Escape in the recipient suggestions closes them alone: the message being written, or the
/// meeting, stays, nothing is asked, and the keyboard stays in the line with what was typed. Once
/// the list is closed, the next Escape is the window's: the message asks about its change, the
/// meeting closes.
#[test]
fn escape_in_recipient_suggestions_closes_them_alone() {
    for (line, open) in RECIPIENT_LINES {
        for (how, press) in PRESSES {
            let mut h = window("view.mail");
            open(&mut h);
            let (before, field) = (dialog(&h), focus(&h));
            press(&mut h, Key::Escape);
            assert!(list_under_field(&h).is_none(), "{line}, {how}: the list closed");
            assert_eq!(dialog(&h), before, "{line}, {how}: nothing asked, nothing closed");
            assert!(h.state().session.compose.is_some() || before.is_some(), "{line}, {how}: the message is still open");
            assert_eq!((focus(&h), recipients_typed(&h)), (field, "pri".to_string()), "{line}, {how}: still typing in the line");
            testing::key(&mut h, Key::Escape);
            let after = if before.is_none() { Some("discardDraft") } else { None };
            assert_eq!(dialog(&h), after, "{line}, {how}: the second Escape is the window's");
        }
    }
}

/// A click away from the recipient suggestions closes them.
#[test]
fn a_click_away_closes_recipient_suggestions() {
    for (line, open) in RECIPIENT_LINES {
        let mut h = window("view.mail");
        open(&mut h);
        let before = dialog(&h);
        // (The Subject line, the meeting's title: a field of the same window.)
        let other = h.get_all_by_role(egui::accesskit::Role::TextInput).find(|n| n.accesskit_node().value().is_none_or(|v| v != "pri")).unwrap();
        other.click();
        h.run_steps(3);
        assert!(list_under_field(&h).is_none(), "{line}: the list closed");
        assert_eq!(dialog(&h), before, "{line}");
    }
}

/// Typing goes on in the line while the suggestions show, and they follow it.
#[test]
fn typing_goes_on_under_recipient_suggestions() {
    for (line, open) in RECIPIENT_LINES {
        let mut h = window("view.mail");
        open(&mut h);
        let field = focus(&h);
        testing::text(&mut h, "ya");
        h.run_steps(2);
        assert_eq!((focus(&h), recipients_typed(&h)), (field, "priya".to_string()), "{line}");
        assert!(list_under_field(&h).is_some(), "{line}: still suggested");
    }
}

/// Enter, Tab or a click on the first suggestion takes it, as `Name <address>; `, and the
/// keyboard stays in the line, for the next one. A held Enter takes it once: its repeats were
/// the list's, not the line's, and leave the line as it is.
#[test]
fn enter_tab_or_a_click_takes_a_suggested_recipient() {
    for (line, open) in RECIPIENT_LINES {
        for how in ["Enter", "held Enter", "Tab", "click"] {
            let mut h = window("view.mail");
            open(&mut h);
            let field = focus(&h);
            match how {
                "Enter" => testing::key(&mut h, Key::Enter),
                "held Enter" => testing::held(&mut h, Key::Enter),
                "Tab" => testing::key(&mut h, Key::Tab),
                _ => {
                    let list = list_under_field(&h).unwrap();
                    testing::click(&mut h, egui::pos2(list.min.x + 30.0, list.min.y + 14.0), egui::PointerButton::Primary, 1);
                }
            }
            h.run_steps(2);
            assert_eq!(recipients_typed(&h), "Priya Nair <priya@lumen.example>; ", "{line}, {how}");
            assert_eq!(focus(&h), field, "{line}, {how}: the line keeps the keyboard");
            assert!(list_under_field(&h).is_none(), "{line}, {how}: nothing more to suggest");
            testing::text(&mut h, "may");
            h.run_steps(2);
            assert!(list_under_field(&h).is_some(), "{line}, {how}: the next name is suggested");
        }
    }
}

/// Escape in the pass right after the recipient suggestions came up (an agent's `ui.key`, a key
/// pressed before the last letter is let go) closes them alone too.
#[test]
fn escape_as_soon_as_recipients_are_suggested_closes_them_alone() {
    for (line, open) in EMPTY_RECIPIENT_LINES {
        let mut h = window("view.mail");
        open(&mut h);
        let (before, field) = (dialog(&h), focus(&h));
        h.input_mut().events.push(egui::Event::Text("pri".to_string()));
        for _ in 0..6 {
            h.step();
            if list_under_field(&h).is_some() {
                break;
            }
        }
        assert!(list_under_field(&h).is_some(), "{line}: recipients are suggested");
        press_now(&mut h, Key::Escape);
        h.run_steps(2);
        assert!(list_under_field(&h).is_none(), "{line}: the list closed");
        assert_eq!(dialog(&h), before, "{line}: nothing asked, nothing closed");
        assert_eq!((focus(&h), recipients_typed(&h)), (field, "pri".to_string()), "{line}: still typing in the line");
    }
}

/// Escape in two passes one after the other under the recipient suggestions (an agent's
/// `ui.key`, a quick double press): the first closes them, the second is the window's, as a
/// later one is.
#[test]
fn escape_in_the_pass_after_recipient_suggestions_closed_is_the_windows() {
    for (line, open) in RECIPIENT_LINES {
        let mut h = window("view.mail");
        open(&mut h);
        let before = dialog(&h);
        press_now(&mut h, Key::Escape);
        assert!(list_under_field(&h).is_none(), "{line}: the list closed");
        press_now(&mut h, Key::Escape);
        h.run_steps(3);
        let after = if before.is_none() { Some("discardDraft") } else { None };
        assert_eq!(dialog(&h), after, "{line}: the second Escape is the window's");
    }
}

/// Tab in the pass right after Enter took a suggestion (an agent's `ui.key`) goes on to the next
/// field, as a later one does: the list is closed, and Tab is the line's again.
#[test]
fn tab_in_the_pass_after_a_suggestion_was_taken_moves_on() {
    for (line, open) in RECIPIENT_LINES {
        let mut h = window("view.mail");
        open(&mut h);
        let field = focus(&h);
        press_now(&mut h, Key::Enter);
        press_now(&mut h, Key::Tab);
        h.run_steps(2);
        assert_eq!(recipients_typed(&h), "Priya Nair <priya@lumen.example>; ", "{line}");
        assert!(focus(&h).is_some() && focus(&h) != field, "{line}: the keyboard went on to the next field");
    }
}

/// The search box with `text` typed in it, and its list open.
fn searching(h: &mut Window, text: &str, offered: &str) {
    run(h, "ui.focusSearch", json!({}));
    testing::text(h, text);
    h.run_steps(2);
    assert!(h.query_by_label(offered).is_some(), "{offered} is offered");
}

fn search(h: &Window) -> String {
    h.state().session.view.search.clone()
}

/// The list under the search box: Enter with no row chosen leaves the box with the search as it
/// is typed; after an arrow, Enter takes the row and searches for it. Escape closes the list and
/// keeps the search; the next Escape clears it.
#[test]
fn the_search_list_keeps_its_enter_and_escape() {
    let mut h = window("view.mail");
    searching(&mut h, "is:", "flagged");
    testing::key(&mut h, Key::Enter);
    assert_eq!(search(&h), "is:");
    assert!(!h.ctx.memory(|m| m.has_focus(crate::chrome::search_box_id())), "Enter left the box");
    assert!(h.query_by_label("flagged").is_none(), "and the list closed");

    let mut h = window("view.mail");
    searching(&mut h, "is:", "flagged");
    testing::key(&mut h, Key::ArrowDown);
    testing::key(&mut h, Key::Enter);
    let first = search(&h);
    assert!(first.starts_with("is:") && first.len() > 4, "a row was taken: {first}");

    for (how, press) in PRESSES {
        let mut h = window("view.mail");
        searching(&mut h, "has:", "attachment");
        press(&mut h, Key::Escape);
        assert!(h.query_by_label("attachment").is_none(), "{how}: the list closed");
        assert_eq!(search(&h), "has:", "{how}: the search stays");
        testing::key(&mut h, Key::Escape);
        assert_eq!(search(&h), "", "{how}: the next Escape clears it");
    }
}
