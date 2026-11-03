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

/// The sample's Inbox in the Unread tab: its oldest read message, which the tab doesn't list,
/// and an unread one, which it does.
fn unread_tab(w: &mut testing::Window) -> (Id, Id) {
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    let rows = w.state_mut().session.rows();
    let read = rows.iter().rev().find(|r| r.unread == 0).map(|r| r.id).unwrap();
    let unread = rows.iter().find(|r| r.unread > 0).map(|r| r.id).unwrap();
    w.state_mut().execute("view.filter", json!({"value": "unread"})).unwrap();
    w.run_steps(3);
    assert!(!drawn(w).contains(&read));
    (read, unread)
}

/// The message opened is in the list, selected, in its folder's All tab.
fn opened_in_the_all_tab(w: &mut testing::Window, id: Id) {
    w.run_steps(3);
    let s = &w.state().session;
    assert_eq!((s.view.module, s.view.filter), (mailcraft_engine::Module::Mail, mailcraft_engine::Filter::All));
    assert_eq!(s.sel.messages, vec![id]);
    assert!(drawn(w).contains(&id), "{id} is in the list");
}

/// A task's "Open the message", for a task made from `id`.
fn open_from_its_task(w: &mut testing::Window, id: Id) {
    let task = w.state_mut().execute("tasks.fromMessage", json!({"id": id})).unwrap()["id"].as_u64().unwrap();
    w.state_mut().execute("view.tasks", json!({})).unwrap();
    w.state_mut().execute("tasks.select", json!({"id": task})).unwrap();
    w.run_steps(3);
    let pos = w.state().hit.rect("openTaskMessage", id).unwrap().center();
    testing::click(w, pos, PointerButton::Primary, 1);
}

#[test]
fn a_message_opened_from_its_task_is_in_the_all_tab() {
    let mut w = testing::window("view.mail");
    let (read, unread) = unread_tab(&mut w);
    // The oldest read one: the list scrolls to it.
    open_from_its_task(&mut w, read);
    opened_in_the_all_tab(&mut w, read);
    // An unread one, which the Unread tab would list: the All tab all the same.
    w.state_mut().execute("view.filter", json!({"value": "unread"})).unwrap();
    open_from_its_task(&mut w, unread);
    opened_in_the_all_tab(&mut w, unread);
}

#[test]
fn a_message_opened_from_a_contact_is_in_the_all_tab() {
    let mut w = testing::window("view.mail");
    unread_tab(&mut w);
    w.state_mut().execute("view.people", json!({})).unwrap();
    let contacts: Vec<Id> = w.state().session.mailbox.contacts.keys().copied().collect();
    let mut found = None;
    for c in contacts {
        w.state_mut().execute("people.select", json!({"id": c})).unwrap();
        w.run_steps(3);
        let s = &w.state().session;
        let inbox = s.view.folder;
        found = s.mailbox.messages_in(inbox).filter(|m| !m.read).map(|m| m.id).find(|id| w.state().hit.rect("recentMail", *id).is_some());
        if found.is_some() {
            break;
        }
    }
    // An unread one, which the Unread tab would list: the All tab all the same.
    let unread = found.expect("a contact of the sample has unread mail in the Inbox among its recent mail");
    let pos = w.state().hit.rect("recentMail", unread).unwrap().center();
    testing::click(&mut w, pos, PointerButton::Primary, 1);
    opened_in_the_all_tab(&mut w, unread);
}

#[test]
fn a_message_opened_from_its_alert_is_in_the_all_tab() {
    let mut w = testing::window("view.mail");
    let (_, unread) = unread_tab(&mut w);
    let inbox = w.state().session.view.folder;
    w.state_mut().execute("view.calendar", json!({})).unwrap();
    let ctx = w.state().ctx.clone().unwrap();
    crate::alerts::open(w.state_mut(), &ctx, crate::alerts::Opens::Message { message: unread, folder: inbox });
    opened_in_the_all_tab(&mut w, unread);
}

/// Clicks the item `kind` `id` and lets the window draw what comes of it.
fn click_on(w: &mut testing::Window, kind: &str, id: u64) {
    let pos = w.state().hit.rect(kind, id).unwrap().center();
    testing::click(w, pos, PointerButton::Primary, 1);
    w.run_steps(3);
}

/// Another list that shows what is selected keeps it selected, in the list and in the reading pane.
fn still_selected(w: &mut testing::Window, id: Id) {
    assert_eq!(w.state().session.sel.messages, vec![id]);
    assert!(drawn(w).contains(&id), "{id} is in the list");
}

#[test]
fn closing_a_search_keeps_the_hit_selected() {
    let mut w = testing::window("view.mail");
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.state_mut().execute("folder.select", json!({"kind": "inbox"})).unwrap();
    w.state_mut().execute("view.search", json!({"query": "e"})).unwrap();
    w.run_steps(3);
    let hit = drawn(&mut w)[0];
    click_on(&mut w, "message", hit);
    still_selected(&mut w, hit);
    click_on(&mut w, "searchBarClose", 0);
    assert!(w.state().session.view.search.is_empty());
    still_selected(&mut w, hit);
}

#[test]
fn the_all_tab_keeps_what_was_selected_in_the_unread_tab() {
    let mut w = testing::window("view.mail");
    let (_, unread) = unread_tab(&mut w);
    click_on(&mut w, "message", unread);
    still_selected(&mut w, unread);
    click_on(&mut w, "listTab", 0);
    assert_eq!(w.state().session.view.filter, mailcraft_engine::Filter::All);
    still_selected(&mut w, unread);
}
