// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Escape and Enter go only to what is on top (#393): an agent's question, an open menu, or a
//! dialog over File. Each test is run with one press and with the key held down: a held key is
//! one press (#391), so its repeats don't reach what is under the top layer either.

use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

use crate::people_view::testing::{self, Window};

/// One press, and the key held down.
const PRESSES: [(&str, fn(&mut Window, Key)); 2] = [("pressed", testing::key), ("held", testing::held)];

/// The sample mailbox with a message being written.
fn window() -> Window {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.execute("mail.new", json!({"subject": "Half written"})).unwrap();
    h.run_steps(3);
    h
}

fn writing(h: &Window) -> bool {
    h.state().session.compose.as_ref().is_some_and(|c| c.subject == "Half written")
}

/// An agent asks to send the message, which waits for the owner's yes.
fn agent_asks(h: &mut Window) -> std::sync::mpsc::Receiver<Value> {
    let (req, rx) = crate::control::ControlRequest::new("engine.execute", json!({"command": "compose.send"}));
    let _ = crate::control::handle(h.state_mut(), &egui::Context::default(), &req.from_agent(vec![]));
    h.run_steps(3);
    assert_eq!(h.state().asking.len(), 1);
    rx
}

fn said_no(rx: &std::sync::mpsc::Receiver<Value>) -> bool {
    rx.try_recv().is_ok_and(|v| v["ok"] == false)
}

fn refused(h: &mut Window) {
    h.state_mut().dialog = Some(crate::dialogs::Dialog::Refused { text: "Not now".into() });
    h.run_steps(3);
}

/// Opens the New menu on the Home tab.
fn open_a_menu(h: &mut Window) {
    h.state_mut().ui.tab = "Home".into();
    h.run_steps(3);
    let at = h.state().hit.rect("ribbonMenu", crate::ribbon::hit_id("mail.new")).unwrap().center();
    testing::click(h, at, PointerButton::Primary, 1);
    assert!(egui::Popup::is_any_open(&h.ctx));
}

/// Escape on an agent's question says no, and the message being written stays open.
#[test]
fn escape_on_an_agents_question_leaves_the_message_open() {
    for (how, press) in PRESSES {
        let mut h = window();
        let rx = agent_asks(&mut h);
        press(&mut h, Key::Escape);
        assert!(said_no(&rx) && h.state().asking.is_empty(), "{how}");
        assert!(writing(&h), "{how}");
    }
}

/// Escape and Enter on an agent's question say no, and the dialog under it stays open: Enter
/// doesn't press its OK.
#[test]
fn the_keys_of_an_agents_question_leave_the_dialog_under_it() {
    for (how, press) in PRESSES {
        let mut h = window();
        refused(&mut h);
        for key in [Key::Escape, Key::Enter] {
            let rx = agent_asks(&mut h);
            press(&mut h, key);
            assert!(said_no(&rx) && h.state().asking.is_empty(), "{how} {key:?}");
            assert!(h.state().dialog.is_some(), "{how} {key:?}: the dialog is still open");
        }
        assert!(writing(&h), "{how}");
    }
}

/// Escape on an agent's question over File says no, and File stays open.
#[test]
fn escape_on_an_agents_question_leaves_file_open() {
    for (how, press) in PRESSES {
        let mut h = window();
        let rx = agent_asks(&mut h);
        h.state_mut().ui.backstage = true;
        h.run_steps(3);
        press(&mut h, Key::Escape);
        assert!(said_no(&rx) && h.state().asking.is_empty(), "{how}");
        assert!(h.state().ui.backstage, "{how}");
    }
}

/// Escape on an agent's question over Send/Receive Progress says no, and the panel stays open.
#[test]
fn escape_on_an_agents_question_leaves_the_progress_open() {
    for (how, press) in PRESSES {
        let mut h = window();
        h.state_mut().execute("ui.dialog", json!({"name": "syncProgress"})).unwrap();
        h.run_steps(3);
        let rx = agent_asks(&mut h);
        press(&mut h, Key::Escape);
        assert!(said_no(&rx) && h.state().asking.is_empty(), "{how}");
        assert!(h.state().sync_panel.is_some(), "{how}");
    }
}

/// A key is held down to close a dialog, and the agent asks while it is still down. Its repeats
/// don't answer the question, which nobody has read yet. A new press does.
#[test]
fn a_key_held_from_before_leaves_an_agents_question() {
    for key in [Key::Escape, Key::Enter] {
        let mut h = window();
        refused(&mut h);
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE });
        h.run_steps(2);
        assert!(h.state().dialog.is_none(), "{key:?}: the press closed the dialog");
        let rx = agent_asks(&mut h);
        for _ in 0..3 {
            h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: true, repeat: true, modifiers: Modifiers::NONE });
            h.run_steps(2);
        }
        assert_eq!(h.state().asking.len(), 1, "{key:?}: the question waits");
        assert!(rx.try_recv().is_err(), "{key:?}: no answer yet");
        h.input_mut().events.push(Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE });
        h.run_steps(2);
        testing::key(&mut h, key);
        assert!(said_no(&rx) && h.state().asking.is_empty(), "{key:?}");
        assert!(writing(&h), "{key:?}");
    }
}

/// Escape with a menu open closes the menu, and the message being written stays open.
#[test]
fn escape_on_a_menu_leaves_the_message_open() {
    for (how, press) in PRESSES {
        let mut h = window();
        open_a_menu(&mut h);
        press(&mut h, Key::Escape);
        assert!(!egui::Popup::is_any_open(&h.ctx), "{how}");
        assert!(writing(&h), "{how}");
    }
}

/// A menu is open while Send/Receive Progress shows: Escape is the menu's, and the panel stays
/// open. (A click to open a menu closes the panel first, so the panel comes up after the menu.)
#[test]
fn escape_on_a_menu_leaves_the_progress_open() {
    for (how, press) in PRESSES {
        let mut h = window();
        open_a_menu(&mut h);
        h.state_mut().execute("ui.dialog", json!({"name": "syncProgress"})).unwrap();
        h.run_steps(3);
        assert!(egui::Popup::is_any_open(&h.ctx) && h.state().sync_panel.is_some(), "{how}");
        press(&mut h, Key::Escape);
        assert!(!egui::Popup::is_any_open(&h.ctx), "{how}: the menu closed");
        assert!(h.state().sync_panel.is_some(), "{how}");
    }
}

/// Escape with a list open in File › Options closes the list, and File stays open.
#[test]
fn escape_on_a_list_in_file_leaves_file_open() {
    for (how, press) in PRESSES {
        let mut h = window();
        h.state_mut().ui.backstage = true;
        h.state_mut().ui.backstage_page = "options".into();
        h.run_steps(3);
        h.get_all_by_role(egui::accesskit::Role::ComboBox).next().unwrap().click();
        h.run_steps(3);
        assert!(egui::Popup::is_any_open(&h.ctx), "{how}: the list is open");
        press(&mut h, Key::Escape);
        assert!(!egui::Popup::is_any_open(&h.ctx), "{how}");
        assert!(h.state().ui.backstage, "{how}");
    }
}

/// Escape with a dialog over File closes the dialog, and File stays open.
#[test]
fn escape_on_a_dialog_over_file_leaves_file_open() {
    for (how, press) in PRESSES {
        let mut h = window();
        h.state_mut().ui.backstage = true;
        h.run_steps(3);
        refused(&mut h);
        press(&mut h, Key::Escape);
        assert!(h.state().dialog.is_none(), "{how}");
        assert!(h.state().ui.backstage, "{how}");
    }
}
