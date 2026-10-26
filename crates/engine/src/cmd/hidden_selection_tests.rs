// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Delete and Move take only the messages the list shows: a message selected before the tab or the
//! search changed, which the list no longer shows, is not taken with a message Ctrl+clicked there.

use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Filter, Session, sample};

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

/// The sample's Inbox in the All tab, with a read message selected.
fn read_one_selected() -> (Session, Id) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let read = s.rows().iter().find(|r| r.unread == 0).map(|r| r.id).unwrap();
    run(&mut s, "mail.select", json!({"id": read}));
    (s, read)
}

#[test]
fn another_tab_search_or_find_lets_the_selection_go_and_an_order_keeps_it() {
    // Another order or grouping is the same list: what is selected stays.
    let (mut s, read) = read_one_selected();
    run(&mut s, "view.sort", json!({"by": "from"}));
    run(&mut s, "view.conversations", json!({"value": true}));
    run(&mut s, "view.groups", json!({"value": false}));
    assert_eq!(s.sel.messages, [read]);
    // A search typed the same way again is the same list too.
    run(&mut s, "view.search", json!({"query": "e"}));
    let found = shown(&mut s)[0];
    run(&mut s, "mail.select", json!({"id": found}));
    run(&mut s, "view.search", json!({"query": "e "}));
    assert_eq!(s.sel.messages.len(), 1);

    // Each of these goes to another list, and nothing selected comes with, though it may be in it.
    let leaving: [(&str, Value); 6] = [
        ("view.filter", json!({"value": "flagged"})),
        ("view.search", json!({"query": "a"})),
        ("view.search", json!({"query": "a", "scope": "everywhere"})),
        ("mail.findRelated", json!({})),
        ("mail.findFromSender", json!({})),
        ("view.reset", json!({})),
    ];
    for (id, params) in leaving {
        let (mut s, _) = read_one_selected();
        if id == "view.reset" {
            run(&mut s, "view.filter", json!({"value": "unread"}));
            let unread = shown(&mut s)[0];
            run(&mut s, "mail.select", json!({"id": unread}));
        }
        run(&mut s, id, params.clone());
        assert!(s.sel.messages.is_empty(), "{id} {params}: {:?}", s.sel.messages);
        assert!(s.sel.stays.is_none() && s.sel.focus_row.is_none() && s.sel.anchor_row.is_none(), "{id} {params}");
    }
    // So does clearing a search.
    let (mut s, _) = read_one_selected();
    run(&mut s, "view.search", json!({"query": "e"}));
    let found = shown(&mut s)[0];
    run(&mut s, "mail.select", json!({"id": found}));
    run(&mut s, "view.clearSearch", json!({}));
    assert!(s.sel.messages.is_empty());
}

#[test]
fn a_message_opened_from_a_task_is_shown_in_its_folder_in_the_all_tab() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let inbox = s.view.folder;
    let read = s.rows().iter().find(|r| r.unread == 0).map(|r| r.id).unwrap();
    let unread = s.rows().iter().find(|r| r.unread > 0).map(|r| r.id).unwrap();
    // From the Unread tab of another folder, with a search typed: a task's "Open the message".
    run(&mut s, "folder.select", json!({"kind": "sent"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    run(&mut s, "view.search", json!({"query": "x"}));
    run(&mut s, "view.tasks", json!({}));
    run(&mut s, "mail.select", json!({"id": read, "elsewhere": true}));
    assert_eq!((s.view.folder, s.view.filter, s.view.search.as_str()), (inbox, Filter::All, ""));
    assert_eq!(s.sel.messages, [read]);
    assert!(shown(&mut s).contains(&read));
    // Always the All tab, though the Unread tab would list it.
    run(&mut s, "view.filter", json!({"value": "unread"}));
    run(&mut s, "mail.select", json!({"id": unread, "elsewhere": true}));
    assert_eq!(s.view.filter, Filter::All);
}

#[test]
fn a_message_opened_from_outside_the_list_is_shown_in_it() {
    // A conversation member in another folder, clicked in the reading pane while a search of
    // this folder is on: its folder, with no search.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let sent = folder(&s, FolderKind::Sent);
    let elsewhere = s.mailbox.messages_in(sent).next().map(|m| m.id).unwrap();
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.search", json!({"query": "e"}));
    run(&mut s, "mail.select", json!({"id": elsewhere}));
    assert_eq!((s.view.folder, s.view.search.as_str()), (sent, ""));
    assert!(shown(&mut s).contains(&elsewhere));

    // An agent's two messages, one of which the Unread tab doesn't list: the All tab.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let read = s.rows().iter().find(|r| r.unread == 0).map(|r| r.id).unwrap();
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let unread = shown(&mut s)[0];
    run(&mut s, "mail.select", json!({"ids": [read, unread]}));
    assert_eq!(s.view.filter, Filter::All);
    let listed = shown(&mut s);
    assert!(s.sel.messages.iter().all(|id| listed.contains(id)), "{:?} {listed:?}", s.sel.messages);
}

#[test]
fn ctrl_or_shift_click_adds_only_to_what_the_list_shows() {
    // Selected by the host though the list doesn't show it (a server search result opened, which
    // the search here may not find), then a Ctrl+click on a row: only the row.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let read = s.rows().iter().find(|r| r.unread == 0).map(|r| r.id).unwrap();
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let listed = shown(&mut s);
    s.sel.messages = vec![read];
    run(&mut s, "mail.select", json!({"id": listed[0], "extend": true}));
    assert_eq!(s.sel.messages, [listed[0]]);
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(s.mailbox.message(read).unwrap().folder, folder(&s, FolderKind::Inbox));

    // A Shift+click from it: a range from no row the list shows is the row clicked.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let listed = shown(&mut s);
    s.sel.messages = vec![read];
    s.sel.anchor_row = Some((read, 0));
    run(&mut s, "mail.select", json!({"id": listed[2], "range": true}));
    assert_eq!(s.sel.messages, [listed[2]]);
}

#[test]
fn undo_shows_what_it_selects_again() {
    let (mut s, read) = read_one_selected();
    run(&mut s, "mail.delete", json!({}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    // Back in the Inbox and selected again: listed, in the All tab.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.sel.messages, [read]);
    assert_eq!(s.view.filter, Filter::All);
    assert!(shown(&mut s).contains(&read));
}

#[test]
fn an_imported_message_is_shown_where_it_is_selected() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.filter", json!({"value": "flagged"}));
    let eml = "From: a@example.org\r\nTo: b@example.org\r\nSubject: Brought in\r\nMessage-ID: <in-1@example.org>\r\n\r\nBody\r\n";
    let r = run(&mut s, "file.import", json!({"name": "in.eml", "data": crate::cmd::base64_encode(eml.as_bytes())}));
    let id = r["ids"][0].as_u64().unwrap();
    assert_eq!(s.sel.messages, [id]);
    assert_eq!(s.view.filter, Filter::All);
    assert!(shown(&mut s).contains(&id));
}

#[test]
fn a_selected_message_a_run_takes_to_another_folder_is_let_go_of() {
    let (mut s, read) = read_one_selected();
    let other = shown(&mut s).into_iter().find(|id| *id != read).unwrap();
    run(&mut s, "mail.select", json!({"id": other, "extend": true}));
    // Moved on the server, as a run brings it in (`Session::mailbox_changed`, which Send/Receive
    // ends with in the window and the command line).
    let archive = folder(&s, FolderKind::Archive);
    s.mailbox.message_mut(read).unwrap().folder = archive;
    s.mailbox_changed();
    assert_eq!(s.sel.messages, [other]);
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(s.mailbox.message(read).unwrap().folder, archive, "{read} wasn't shown, and stays where the run put it");
}

#[test]
fn importing_a_tree_of_folders_lets_the_old_folders_selection_go() {
    let (mut s, read) = read_one_selected();
    let mbox = "From a@example.org Mon Oct 12 10:00:00 2026\nFrom: a@example.org\nTo: b@example.org\nSubject: Old\nMessage-ID: <old-1@example.org>\n\nBody\n\n";
    let entries = json!([{"path": "Mail/Old", "data": crate::cmd::base64_encode(mbox.as_bytes())}]);
    run(&mut s, "file.importFolderTree", json!({"entries": entries, "name": "Old mail"}));
    assert_ne!(s.mailbox.message(read).unwrap().folder, s.view.folder, "the view went to the folders brought in");
    assert!(s.sel.messages.is_empty());
}
