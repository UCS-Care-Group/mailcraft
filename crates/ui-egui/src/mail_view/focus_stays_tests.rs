// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #428: a message opened in the Unread tab, by a click or a key, stays in the list, drawn as read,
//! until the focus moves; Down goes on to the one below it, and a range made with Shift stays.

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

#[test]
fn a_message_end_opens_stays_drawn_as_read() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("view.filter", json!({"value": "unread"})).unwrap();
    w.run_steps(3);
    let listed: Vec<Id> = drawn(&mut w).into_iter().map(|(id, _)| id).collect();
    assert!(listed.len() >= 3, "{listed:?}");

    let pos = w.state().hit.rect("message", listed[0]).unwrap().center();
    testing::click(&mut w, pos, PointerButton::Primary, 1);
    w.run_steps(3);
    testing::key(&mut w, Key::End);
    w.run_steps(3);
    let end = *listed.last().unwrap();
    assert_eq!(w.state().session.sel.focus(), Some(end));
    let mut want: Vec<(Id, bool)> = listed[1..].iter().map(|id| (*id, *id != end)).collect();
    assert_eq!(drawn(&mut w), want, "the first leaves; the last stays, drawn as read");
    // Shift+Up from it: both selected, and both stay.
    for pressed in [true, false] {
        w.input_mut().events.push(egui::Event::Key {
            key: Key::ArrowUp,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        });
    }
    w.run_steps(3);
    assert_eq!(w.state().session.sel.messages.len(), 2);
    w.state_mut().execute("mail.markRead", json!({})).unwrap();
    w.run_steps(3);
    want.iter_mut().rev().take(2).for_each(|row| row.1 = false);
    assert_eq!(drawn(&mut w), want, "marked read, the range stays");
}
