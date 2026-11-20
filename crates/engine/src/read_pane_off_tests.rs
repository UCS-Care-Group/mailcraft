// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! With the reading pane off nothing is shown, so selecting a message doesn't mark it read
//! (#1734); opening it in a window of its own (`"open": true`) does.
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::session::Pane;
use crate::*;

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// A session with the pane as given, and an unread message in the list.
fn with(pane: Pane) -> (Session, mailcraft_model::Id) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    s.view.reading_pane = pane;
    let id = s.rows().iter().map(|r| r.id).find(|id| s.mailbox.message(*id).is_some_and(|m| !m.read && !m.draft)).unwrap();
    (s, id)
}

fn read(s: &Session, id: mailcraft_model::Id) -> bool {
    s.mailbox.message(id).unwrap().read
}

#[test]
fn pane_off_selecting_leaves_it_unread() {
    let (mut s, id) = with(Pane::Off);
    run(&mut s, "mail.select", json!({"id": id}));
    assert!(!read(&s, id));
}

#[test]
fn pane_off_opening_in_a_window_marks_it_read() {
    let (mut s, id) = with(Pane::Off);
    run(&mut s, "mail.select", json!({"id": id}));
    run(&mut s, "mail.select", json!({"id": id, "open": true}));
    assert!(read(&s, id));
}

#[test]
fn pane_off_opening_in_a_window_respects_the_option() {
    let (mut s, id) = with(Pane::Off);
    s.prefs.mark_read_on_open = false;
    run(&mut s, "mail.select", json!({"id": id, "open": true}));
    assert!(!read(&s, id));
}

#[test]
fn pane_off_arrowing_leaves_it_unread() {
    let (mut s, _) = with(Pane::Off);
    // An unread message with a row above it, arrowed down to from that row.
    let rows: Vec<_> = s.rows().iter().map(|r| r.id).collect();
    let i = (1..rows.len()).find(|i| s.mailbox.message(rows[*i]).is_some_and(|m| !m.read && !m.draft)).unwrap();
    run(&mut s, "mail.select", json!({"id": rows[i - 1]}));
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.messages, [rows[i]]);
    assert!(!read(&s, rows[i]));
}

#[test]
fn pane_turned_on_marks_the_selected_one_read() {
    for value in ["right", "bottom"] {
        let (mut s, id) = with(Pane::Off);
        run(&mut s, "mail.select", json!({"id": id}));
        run(&mut s, "view.readingPane", json!({"value": value}));
        assert!(read(&s, id), "{value}");
    }
    // Out of the Preview view, whose pane is off, too.
    let (mut s, id) = with(Pane::Right);
    run(&mut s, "view.changeView", json!({"value": "preview"}));
    run(&mut s, "mail.select", json!({"id": id}));
    assert!(!read(&s, id));
    run(&mut s, "view.changeView", json!({"value": "compact"}));
    assert!(read(&s, id));
}

#[test]
fn pane_turned_on_respects_the_option() {
    let (mut s, id) = with(Pane::Off);
    s.prefs.mark_read_on_open = false;
    run(&mut s, "mail.select", json!({"id": id}));
    run(&mut s, "view.readingPane", json!({"value": "right"}));
    assert!(!read(&s, id));
}

#[test]
fn pane_showing_selecting_marks_it_read() {
    for pane in [Pane::Right, Pane::Bottom] {
        let (mut s, id) = with(pane);
        run(&mut s, "mail.select", json!({"id": id}));
        assert!(read(&s, id), "{pane:?}");
    }
}
