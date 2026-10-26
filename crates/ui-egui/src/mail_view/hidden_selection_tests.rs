// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! In the window: a read message clicked in the All tab, the Unread tab, a Ctrl+click on a message
//! there and Delete take only the message the list shows.

use egui::{Event, Key, Modifiers, PointerButton};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::people_view::testing;

fn drawn(w: &mut testing::Window) -> Vec<Id> {
    let rows = w.state_mut().session.rows();
    rows.iter().filter(|row| w.state().hit.rect("message", row.id).is_some()).map(|row| row.id).collect()
}

#[test]
fn delete_after_the_unread_tab_takes_only_what_the_list_shows() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.run_steps(3);
    let read = w.state_mut().session.rows().iter().find(|r| r.unread == 0).map(|r| r.id).unwrap();
    let pos = w.state().hit.rect("message", read).unwrap().center();
    testing::click(&mut w, pos, PointerButton::Primary, 1);
    w.run_steps(3);
    assert_eq!(w.state().session.sel.messages, vec![read]);

    let tab = w.state().hit.rect("listTab", 1).unwrap().center();
    testing::click(&mut w, tab, PointerButton::Primary, 1);
    w.run_steps(3);
    let listed = drawn(&mut w);
    assert!(!listed.contains(&read), "the Unread tab doesn't show {read}: {listed:?}");

    let other = listed[0];
    let pos = w.state().hit.rect("message", other).unwrap().center();
    w.run_steps(45);
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::COMMAND));
    w.input_mut().events.push(Event::PointerMoved(pos));
    w.step();
    for pressed in [true, false] {
        w.input_mut().events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::COMMAND });
        w.step();
    }
    w.input_mut().events.push(Event::ModifiersChanged(Modifiers::NONE));
    w.run_steps(3);

    testing::key(&mut w, Key::Delete);
    w.run_steps(3);
    let s = &w.state().session;
    let kind = |id: Id| s.mailbox.message(id).and_then(|m| s.mailbox.folder(m.folder)).map(|f| f.kind);
    assert_eq!(kind(other), Some(FolderKind::Deleted));
    assert_eq!(kind(read), Some(FolderKind::Inbox), "{read} wasn't shown, and stays in the Inbox");
}
