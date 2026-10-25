// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #428: a message opened in the Unread tab stays in the list, drawn as read, until the focus
//! moves; Down goes on to the one below it.

use egui::{Key, PointerButton};
use mailcraft_model::Id;
use serde_json::json;

use crate::people_view::testing;

/// The rows drawn, top to bottom: each message, and whether it is drawn as unread.
fn drawn(w: &mut testing::Window) -> Vec<(Id, bool)> {
    let rows = w.state_mut().session.rows();
    rows.iter().filter(|row| w.state().hit.rect("message", row.id).is_some()).map(|row| (row.id, row.unread > 0)).collect()
}

#[test]
fn an_opened_message_stays_drawn_as_read_until_down_moves_on() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("view.filter", json!({"value": "unread"})).unwrap();
    w.run_steps(3);
    let listed: Vec<Id> = drawn(&mut w).into_iter().map(|(id, unread)| if unread { id } else { panic!("{id} is read") }).collect();
    assert!(listed.len() >= 3, "{listed:?}");

    let pos = w.state().hit.rect("message", listed[1]).unwrap().center();
    testing::click(&mut w, pos, PointerButton::Primary, 1);
    w.run_steps(3);
    assert_eq!(w.state().session.sel.focus(), Some(listed[1]));
    let mut want: Vec<(Id, bool)> = listed.iter().map(|id| (*id, true)).collect();
    want[1].1 = false;
    assert_eq!(drawn(&mut w), want, "still listed, drawn as read");

    testing::key(&mut w, Key::ArrowDown);
    w.run_steps(3);
    assert_eq!(w.state().session.sel.focus(), Some(listed[2]));
    want.remove(1);
    want[1].1 = false;
    assert_eq!(drawn(&mut w), want, "it leaves once the focus moves; the next one stays");
}
