// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Escape and Enter go only to the layer on top (#406): a list open in a dialog, the recurrence
//! editor of an appointment in a dialog, the front one of several windows, a window's question
//! about its unsaved changes, Send/Receive Progress over a window. And the repeats of a held key
//! that closed a dialog don't reach what is under it (a contact, a task, an appointment, a search,
//! a picked slot). Where both apply, each test is run with one press and with the key held down.

use egui::Key;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::layers::{Layer, Open, top};
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

/// The order of the layers, from the top.
#[test]
fn what_is_on_top() {
    let everything = Open {
        agent: true,
        unsaved: Some(4),
        menu: true,
        dialog: true,
        dialog_inner: true,
        progress: true,
        window: Some(3),
        window_inner: Some(3),
        window_waits: true,
        file: true,
        search: true,
    };
    let mut open = everything;
    let mut seen = Vec::new();
    for peel in 0..12 {
        seen.push(top(&open));
        match peel {
            0 => open.agent = false,
            1 => open.unsaved = None,
            2 => open.menu = false,
            3 => open.dialog_inner = false,
            4 => open.dialog = false,
            5 => open.progress = false,
            6 => open.window_inner = None,
            7 => open.window_waits = false,
            8 => open.window = None,
            9 => open.file = false,
            10 => open.search = false,
            _ => {}
        }
    }
    assert_eq!(
        seen,
        [
            Layer::Agent,
            Layer::Unsaved(4),
            Layer::Menu,
            Layer::Inner(0),
            Layer::Dialog,
            Layer::Progress,
            Layer::Inner(3),
            Layer::Nobody,
            Layer::Window(3),
            Layer::File,
            Layer::Search,
            Layer::Main,
        ]
    );
    assert_eq!(top(&Open::default()), Layer::Main);
}
