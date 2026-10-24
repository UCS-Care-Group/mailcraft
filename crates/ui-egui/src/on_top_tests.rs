// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Escape and Enter go only to what is on top (#393): an agent's question, an open menu, or a
//! dialog over File.

use egui::{Key, PointerButton};
use serde_json::{Value, json};

use crate::people_view::testing::{self, Window};

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

/// Escape on an agent's question says no, and the message being written stays open.
#[test]
fn escape_on_an_agents_question_leaves_the_message_open() {
    let mut h = window();
    let rx = agent_asks(&mut h);
    testing::key(&mut h, Key::Escape);
    assert!(said_no(&rx) && h.state().asking.is_empty());
    assert!(writing(&h));
}

/// Escape and Enter on an agent's question say no, and the dialog under it stays open.
#[test]
fn the_keys_of_an_agents_question_leave_the_dialog_under_it() {
    let mut h = window();
    refused(&mut h);
    for key in [Key::Escape, Key::Enter] {
        let rx = agent_asks(&mut h);
        testing::key(&mut h, key);
        assert!(said_no(&rx) && h.state().asking.is_empty(), "{key:?}");
        assert!(h.state().dialog.is_some(), "{key:?}: the dialog is still open");
    }
    assert!(writing(&h));
}

/// Escape on an agent's question over File says no, and File stays open.
#[test]
fn escape_on_an_agents_question_leaves_file_open() {
    let mut h = window();
    let rx = agent_asks(&mut h);
    h.state_mut().ui.backstage = true;
    h.run_steps(3);
    testing::key(&mut h, Key::Escape);
    assert!(said_no(&rx) && h.state().asking.is_empty());
    assert!(h.state().ui.backstage);
}

/// Escape on an agent's question over Send/Receive Progress says no, and the panel stays open.
#[test]
fn escape_on_an_agents_question_leaves_the_progress_open() {
    let mut h = window();
    h.state_mut().execute("ui.dialog", json!({"name": "syncProgress"})).unwrap();
    h.run_steps(3);
    let rx = agent_asks(&mut h);
    testing::key(&mut h, Key::Escape);
    assert!(said_no(&rx) && h.state().asking.is_empty());
    assert!(h.state().sync_panel.is_some());
}

/// Escape with a menu open closes the menu, and the message being written stays open.
#[test]
fn escape_on_a_menu_leaves_the_message_open() {
    let mut h = window();
    h.state_mut().ui.tab = "Home".into();
    h.run_steps(3);
    let at = h.state().hit.rect("ribbonMenu", crate::ribbon::hit_id("mail.new")).unwrap().center();
    testing::click(&mut h, at, PointerButton::Primary, 1);
    assert!(egui::Popup::is_any_open(&h.ctx));
    testing::key(&mut h, Key::Escape);
    assert!(!egui::Popup::is_any_open(&h.ctx));
    assert!(writing(&h));
}

/// Escape with a dialog over File closes the dialog, and File stays open.
#[test]
fn escape_on_a_dialog_over_file_leaves_file_open() {
    let mut h = window();
    h.state_mut().ui.backstage = true;
    h.run_steps(3);
    refused(&mut h);
    testing::key(&mut h, Key::Escape);
    assert!(h.state().dialog.is_none());
    assert!(h.state().ui.backstage);
}
