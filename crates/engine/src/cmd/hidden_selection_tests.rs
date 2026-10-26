// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Delete and Move take only the messages the list shows: a message selected before the tab or the
//! search changed, which the list no longer shows, is not taken with a message Ctrl+clicked there.

use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn shown(s: &mut Session) -> Vec<Id> {
    s.rows().iter().map(|r| r.id).collect()
}

fn folder(s: &Session, kind: FolderKind) -> Id {
    s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, kind).unwrap()
}

/// The Inbox: a read message clicked in the All tab, then `narrow` (the Unread tab, a search), and
/// a Ctrl+click on a message the list shows. Gives the read one and the Ctrl+clicked one.
fn hidden_and_clicked(narrow: (&str, Value)) -> (Session, Id, Id) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let read = s.rows().iter().find(|r| r.unread == 0).map(|r| r.id).unwrap();
    run(&mut s, "mail.select", json!({"id": read}));
    run(&mut s, narrow.0, narrow.1);
    let listed = shown(&mut s);
    assert!(!listed.contains(&read), "the list doesn't show {read}: {listed:?}");
    let other = listed[0];
    run(&mut s, "mail.select", json!({"id": other, "extend": true}));
    assert!(!shown(&mut s).contains(&read));
    (s, read, other)
}

#[test]
fn delete_leaves_a_message_the_unread_tab_does_not_show() {
    let (mut s, read, other) = hidden_and_clicked(("view.filter", json!({"value": "unread"})));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(s.mailbox.message(other).unwrap().folder, folder(&s, FolderKind::Deleted));
    assert_eq!(s.mailbox.message(read).unwrap().folder, folder(&s, FolderKind::Inbox), "{read} wasn't shown, and stays in the Inbox");
}

#[test]
fn move_leaves_a_message_the_unread_tab_does_not_show() {
    let (mut s, read, other) = hidden_and_clicked(("view.filter", json!({"value": "unread"})));
    let archive = folder(&s, FolderKind::Archive);
    run(&mut s, "mail.move", json!({"folder": archive}));
    assert_eq!(s.mailbox.message(other).unwrap().folder, archive);
    assert_eq!(s.mailbox.message(read).unwrap().folder, folder(&s, FolderKind::Inbox), "{read} wasn't shown, and stays in the Inbox");
}

#[test]
fn delete_leaves_a_message_a_search_does_not_find() {
    let (mut s, read, other) = hidden_and_clicked(("view.search", json!({"query": "is:unread"})));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(s.mailbox.message(other).unwrap().folder, folder(&s, FolderKind::Deleted));
    assert_eq!(s.mailbox.message(read).unwrap().folder, folder(&s, FolderKind::Inbox), "{read} wasn't found, and stays in the Inbox");
}

#[test]
fn delete_leaves_a_message_opened_in_the_unread_tab_after_all_and_back() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let listed = shown(&mut s);
    let opened = listed[0];
    run(&mut s, "mail.select", json!({"id": opened}));
    run(&mut s, "view.filter", json!({"value": "all"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    assert!(!shown(&mut s).contains(&opened), "read now, and the list starts afresh");
    let other = shown(&mut s)[0];
    run(&mut s, "mail.select", json!({"id": other, "extend": true}));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(s.mailbox.message(other).unwrap().folder, folder(&s, FolderKind::Deleted));
    assert_eq!(s.mailbox.message(opened).unwrap().folder, folder(&s, FolderKind::Inbox), "{opened} wasn't shown, and stays in the Inbox");
}
