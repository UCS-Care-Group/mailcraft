// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The dimmed window behind a dialog: a click on it reaches nothing, and the dialog stays on top
//! of it, so its buttons still answer afterwards (#489).

use egui::{Event, Modifiers, PointerButton, Pos2};
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::dialogs::{Dialog, Question};
use crate::people_view::testing::{self, Window};

fn click(h: &mut Window, pos: Pos2) {
    h.event(Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
    h.run_steps(3);
}

fn ask(h: &mut Window) {
    let ask = Question::new("Keep Tasks in Step", "Allow it?".to_string(), "Continue", serde_json::Value::Null);
    h.state_mut().dialog = Some(Dialog::Confirm { command: "ui.allowTasks".to_string(), params: json!({"account": 1}), ask });
    h.run_steps(3);
}

fn cancel(h: &mut Window) {
    let at = h.get_by_label("Cancel").rect().center();
    click(h, at);
}

/// A click on the dimmed window doesn't lift the dim over the dialog: Cancel, clicked where it is
/// drawn, still closes it. Nor over the next dialog: egui keeps the order of what was on top
/// while the window is open, so a dialog opened later, with nothing clicked behind it, came up
/// under the dim too.
#[test]
fn a_click_behind_a_dialog_leaves_its_buttons_working() {
    let mut h = testing::window("view.mail");
    let _ = h.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    ask(&mut h);
    click(&mut h, Pos2::new(10.0, 10.0));
    assert!(h.state().dialog.is_some(), "a click behind the dialog closed it");
    cancel(&mut h);
    assert!(h.state().dialog.is_none(), "Cancel didn't answer after a click behind the dialog");
    ask(&mut h);
    cancel(&mut h);
    assert!(h.state().dialog.is_none(), "Cancel didn't answer in the next dialog");
}
