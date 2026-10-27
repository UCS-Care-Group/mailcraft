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
fn another_tab_or_search_keeps_selected_what_it_shows_and_an_order_keeps_it() {
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

    // Another tab, search or find that shows what is selected keeps it selected: Close Search,
    // the hit being in the folder.
    let (mut s, _) = read_one_selected();
    run(&mut s, "view.search", json!({"query": "e"}));
    let found = shown(&mut s)[0];
    run(&mut s, "mail.select", json!({"id": found}));
    run(&mut s, "view.clearSearch", json!({}));
    assert_eq!(s.sel.messages, [found]);
    assert!(shown(&mut s).contains(&found));
    // One more letter, the search finding it still.
    let word: String = s.mailbox.message(found).unwrap().subject.split_whitespace().next().unwrap().to_string();
    let less: String = word.chars().take(word.chars().count() - 1).collect();
    run(&mut s, "view.search", json!({"query": less}));
    run(&mut s, "view.search", json!({"query": word}));
    assert_eq!(s.sel.messages, [found]);
    assert!(shown(&mut s).contains(&found));
    // The Unread tab, then All: read now, and listed there.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let unread = shown(&mut s)[0];
    run(&mut s, "mail.select", json!({"id": unread}));
    run(&mut s, "view.filter", json!({"value": "all"}));
    assert_eq!(s.sel.messages, [unread]);
    // Find Related, Messages from Sender and Reset View find it too.
    for id in ["mail.findRelated", "mail.findFromSender", "view.reset"] {
        let (mut s, read) = read_one_selected();
        run(&mut s, id, json!({}));
        assert_eq!(s.sel.messages, [read], "{id}");
        assert!(shown(&mut s).contains(&read), "{id}");
    }

    // One that doesn't show it lets it go, with its rows and what kept it.
    let leaving: [(&str, Value); 4] = [
        ("view.filter", json!({"value": "unread"})),
        ("view.filter", json!({"value": "flagged"})),
        ("view.search", json!({"query": "zzzz"})),
        ("view.search", json!({"query": "zzzz", "scope": "everywhere"})),
    ];
    for (id, params) in leaving {
        let (mut s, read) = read_one_selected();
        run(&mut s, id, params.clone());
        assert!(s.sel.messages.is_empty(), "{id} {params}: {:?}", s.sel.messages);
        assert!(s.sel.stays.is_none() && s.sel.focus_row.is_none() && s.sel.anchor_row.is_none(), "{id} {params}");
        assert!(!shown(&mut s).contains(&read), "{id} {params}");
    }

    // Of several selected, those it shows stay, and Delete takes only them.
    let (mut s, read) = read_one_selected();
    let unread = s.rows().iter().find(|r| r.unread > 0).map(|r| r.id).unwrap();
    run(&mut s, "mail.select", json!({"ids": [read, unread]}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    assert_eq!(s.sel.messages, [unread]);
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(s.mailbox.message(unread).unwrap().folder, folder(&s, FolderKind::Deleted));
    assert_eq!(s.mailbox.message(read).unwrap().folder, folder(&s, FolderKind::Inbox));
}

#[test]
fn an_older_message_of_a_conversation_stays_selected_where_its_row_is_shown() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.conversations", json!({"value": true}));
    let row = s.rows().iter().find(|r| r.members.len() > 1).cloned().unwrap();
    let older = row.members[1];
    run(&mut s, "mail.select", json!({"id": older}));
    // A search that finds the conversation: its row shows the newer message, and the older one,
    // which it stands for too, stays selected.
    let word: String = s.mailbox.message(older).unwrap().topic().split_whitespace().last().unwrap().to_string();
    run(&mut s, "view.search", json!({"query": word}));
    let rows = s.rows();
    let found = rows.iter().find(|r| r.members.contains(&older)).unwrap();
    assert_eq!(found.id, row.id);
    assert_eq!(s.sel.messages, [older]);
    // And Close Search.
    run(&mut s, "view.clearSearch", json!({}));
    assert_eq!(s.sel.messages, [older]);
}

#[test]
fn undo_of_a_step_that_changed_no_mail_leaves_the_message_list_as_it_is() {
    let (mut s, read) = read_one_selected();
    let task = run(&mut s, "tasks.new", json!({"title": "Pay the invoice"}))["id"].as_u64().unwrap();
    // The Flagged tab and a search of another folder, with nothing selected: undoing the task
    // takes it back, and the list and its selection stay as they are.
    run(&mut s, "folder.select", json!({"kind": "sent"}));
    run(&mut s, "view.filter", json!({"value": "flagged"}));
    run(&mut s, "view.search", json!({"query": "e"}));
    assert!(s.sel.messages.is_empty());
    let view = (s.view.folder, s.view.filter, s.view.search.clone());
    run(&mut s, "edit.undo", json!({}));
    assert!(s.mailbox.task(task).is_none());
    assert_eq!((s.view.folder, s.view.filter, s.view.search.clone()), view);
    assert!(s.sel.messages.is_empty(), "{read} isn't selected again");
    run(&mut s, "edit.redo", json!({}));
    assert!(s.mailbox.task(task).is_some());
    assert_eq!((s.view.folder, s.view.filter, s.view.search.clone()), view);
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

#[test]
fn a_message_selected_already_and_opened_from_its_task_stays_selected() {
    // Opened in the Unread tab, which keeps it, then from its task: the same selection, in the
    // All tab.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let opened = shown(&mut s)[0];
    run(&mut s, "mail.select", json!({"id": opened}));
    run(&mut s, "view.tasks", json!({}));
    run(&mut s, "mail.select", json!({"id": opened, "elsewhere": true}));
    assert_eq!(s.view.filter, Filter::All);
    assert_eq!(s.sel.messages, [opened]);
}

#[test]
fn undo_that_selects_again_what_is_selected_shows_it() {
    let (mut s, read) = read_one_selected();
    run(&mut s, "mail.markUnread", json!({}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    run(&mut s, "mail.select", json!({"id": read}));
    // Read again by Undo, which the Unread tab doesn't list: the All tab, with it selected.
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.view.filter, Filter::All);
    assert_eq!(s.sel.messages, [read]);
    assert!(shown(&mut s).contains(&read));
}

#[test]
fn a_message_selected_twice_over_is_shown_where_it_is() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let unread = shown(&mut s)[0];
    run(&mut s, "mail.select", json!({"ids": [unread, unread]}));
    assert_eq!(s.view.filter, Filter::Unread);
}

#[test]
fn an_imported_message_is_shown_in_the_folder_it_went_to() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let archive = folder(&s, FolderKind::Archive);
    let eml = "From: a@example.org\r\nTo: b@example.org\r\nSubject: Kept\r\nMessage-ID: <in-2@example.org>\r\n\r\nBody\r\n";
    let r = run(&mut s, "file.import", json!({"name": "in.eml", "data": crate::cmd::base64_encode(eml.as_bytes()), "folder": archive}));
    let id = r["ids"][0].as_u64().unwrap();
    assert_eq!((s.view.folder, s.sel.messages.as_slice()), (archive, [id].as_slice()));
    assert!(shown(&mut s).contains(&id));
}

#[test]
fn an_imported_message_read_where_it_is_selected_stays_in_the_list() {
    // In the Unread tab, read with no list built in between (an agent, the command line): it
    // stays, as one clicked does (#428).
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.filter", json!({"value": "unread"}));
    let eml = "From: a@example.org\r\nTo: b@example.org\r\nSubject: Read\r\nMessage-ID: <in-3@example.org>\r\n\r\nBody\r\n";
    let r = run(&mut s, "file.import", json!({"name": "in.eml", "data": crate::cmd::base64_encode(eml.as_bytes())}));
    let id = r["ids"][0].as_u64().unwrap();
    run(&mut s, "mail.markRead", json!({}));
    assert!(s.mailbox.message(id).unwrap().read);
    assert_eq!((s.view.filter, s.sel.messages.as_slice()), (Filter::Unread, [id].as_slice()));
    assert!(shown(&mut s).contains(&id));
}

#[test]
fn undo_shows_what_it_selects_again_in_its_folder() {
    // Deleted from a search of every mailbox, in Sent Items, and the search cleared: Undo selects
    // it again in Sent Items.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let sent = folder(&s, FolderKind::Sent);
    run(&mut s, "view.search", json!({"query": "e", "scope": "everywhere"}));
    let found = shown(&mut s).into_iter().find(|id| s.mailbox.message(*id).is_some_and(|m| m.folder == sent)).unwrap();
    run(&mut s, "mail.select", json!({"id": found}));
    run(&mut s, "mail.delete", json!({}));
    run(&mut s, "view.clearSearch", json!({}));
    run(&mut s, "edit.undo", json!({}));
    assert_eq!((s.view.folder, s.sel.messages.as_slice()), (sent, [found].as_slice()));
    assert!(shown(&mut s).contains(&found));
}

#[test]
fn the_message_being_read_is_kept_when_the_rest_is_not_found() {
    // A search of every mailbox finds one in Sent Items, not one in the Inbox: both selected by an
    // agent, the one being read is shown in its folder.
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let inbox = s.view.folder;
    let sent = folder(&s, FolderKind::Sent);
    run(&mut s, "view.search", json!({"query": "Lisbon", "scope": "everywhere"}));
    let listed = shown(&mut s);
    let found = listed.iter().copied().find(|id| s.mailbox.message(*id).is_some_and(|m| m.folder == sent)).unwrap();
    let not_found = s.mailbox.messages_in(inbox).map(|m| m.id).find(|id| !listed.contains(id)).unwrap();
    run(&mut s, "mail.select", json!({"ids": [not_found, found]}));
    assert_eq!((s.view.folder, s.sel.messages.as_slice()), (sent, [found].as_slice()));
}

#[test]
fn an_import_a_search_finds_is_selected_in_the_search() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.search", json!({"query": "Brought"}));
    shown(&mut s);
    let eml = "From: a@example.org\r\nTo: b@example.org\r\nSubject: Brought in\r\nMessage-ID: <in-4@example.org>\r\n\r\nBody\r\n";
    let r = run(&mut s, "file.import", json!({"name": "in.eml", "data": crate::cmd::base64_encode(eml.as_bytes())}));
    let id = r["ids"][0].as_u64().unwrap();
    assert_eq!((s.view.search.as_str(), s.sel.messages.as_slice()), ("Brought", [id].as_slice()));
    assert!(shown(&mut s).contains(&id));
}

#[test]
fn redo_shows_what_it_selects_again() {
    let (mut s, read) = read_one_selected();
    run(&mut s, "mail.delete", json!({}));
    let next = s.sel.messages.clone();
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(s.sel.messages, [read]);
    run(&mut s, "view.filter", json!({"value": "flagged"}));
    // Deleted again, with what was selected after it: listed, in the All tab.
    run(&mut s, "edit.redo", json!({}));
    assert_eq!((s.view.filter, &s.sel.messages), (Filter::All, &next));
    let listed = shown(&mut s);
    assert!(!next.is_empty() && next.iter().all(|id| listed.contains(id)), "{next:?} {listed:?}");
}

/// What every command leaves (#485): from a selection the list shows, in each of a few lists, the
/// selection is in the folder in view (or where its search looks), the list shows each of its
/// messages, and what a command says it selected is still selected. A command that breaks that,
/// now or later, fails here.
#[test]
fn every_command_leaves_only_what_the_list_shows_selected() {
    use std::collections::HashSet;
    // A server search result opened is put into its folder and selected as it is: what the
    // server found it by (its body) may not be what the search here finds it by, and the
    // window lists the server's results apart. A Ctrl+click lets it go
    // (`ctrl_or_shift_click_adds_only_to_what_the_list_shows`).
    const SELECTS_AS_IT_IS: [&str; 1] = ["search.openServerResult"];
    type Setup = fn(&mut Session) -> Option<()>;
    let setups: [(&str, Setup); 5] = [
        ("a read message and an unread one, in the All tab", |s| {
            s.run("folder.select", &json!({"kind": "inbox"})).ok()?;
            let rows = s.rows();
            let read = rows.iter().find(|r| r.unread == 0)?.id;
            let unread = rows.iter().find(|r| r.unread > 0)?.id;
            s.run("mail.select", &json!({"id": read})).ok()?;
            s.run("mail.select", &json!({"id": unread, "extend": true})).ok().map(|_| ())
        }),
        ("one opened in the Unread tab, which keeps it", |s| {
            s.run("folder.select", &json!({"kind": "inbox"})).ok()?;
            s.run("view.filter", &json!({"value": "unread"})).ok()?;
            let id = s.rows().first()?.id;
            s.run("mail.select", &json!({"id": id})).ok().map(|_| ())
        }),
        ("one found by a search of the folder", |s| {
            s.run("folder.select", &json!({"kind": "inbox"})).ok()?;
            s.run("view.search", &json!({"query": "e"})).ok()?;
            let id = s.rows().get(1)?.id;
            s.run("mail.select", &json!({"id": id})).ok().map(|_| ())
        }),
        ("one in Sent Items found by a search of every mailbox", |s| {
            s.run("folder.select", &json!({"kind": "inbox"})).ok()?;
            s.run("view.search", &json!({"query": "e", "scope": "everywhere"})).ok()?;
            let sent = s.mailbox.folder_of_kind(s.mailbox.accounts.first()?.id, FolderKind::Sent)?;
            let id = s.rows().iter().map(|r| r.id).find(|id| s.mailbox.message(*id).is_some_and(|m| m.folder == sent))?;
            s.run("mail.select", &json!({"id": id})).ok().map(|_| ())
        }),
        ("nothing, after a read message was deleted in the All tab and the Unread tab shown", |s| {
            s.run("folder.select", &json!({"kind": "inbox"})).ok()?;
            s.run("view.filter", &json!({"value": "all"})).ok()?;
            let read = s.rows().iter().find(|r| r.unread == 0)?.id;
            s.run("mail.select", &json!({"id": read})).ok()?;
            s.run("mail.delete", &json!({})).ok()?;
            s.run("view.filter", &json!({"value": "unread"})).ok().map(|_| ())
        }),
    ];
    let fresh = || Session::at(sample::mailbox(now(), 0), now(), 0);
    let eml = crate::cmd::base64_encode(b"From: a@example.org\r\nSubject: In\r\nMessage-ID: <sweep@example.org>\r\n\r\nBody\r\n");
    let ids: Vec<&'static str> = fresh().registry.all().iter().map(|c| c.id).collect();
    let mut checked = 0;
    for id in &ids {
        for (setup_name, setup) in &setups {
            let mut s = fresh();
            if setup(&mut s).is_none() {
                panic!("{setup_name}: can't be set up on the sample");
            }
            // What the list shows and what it doesn't, for the params.
            let shown: Vec<Id> = s.rows().iter().map(|r| r.id).collect();
            let in_folder = s.mailbox.messages_in(s.view.folder).map(|m| m.id).find(|m| !shown.contains(m)).unwrap_or(0);
            let archive = folder(&s, FolderKind::Archive);
            let first = shown.first().copied().unwrap_or(0);
            let second = shown.get(1).copied().unwrap_or(first);
            let all_params = [
                json!(null),
                json!({}),
                json!({"id": first, "ids": [first, second], "folder": archive, "value": "unread", "query": "is:unread", "by": "from", "kind": "sent"}),
                json!({"id": in_folder, "extend": true}),
                json!({"id": in_folder, "range": true}),
                json!({"ids": [in_folder, first]}),
                json!({"id": second, "elsewhere": true}),
                json!({"name": "in.eml", "data": eml}),
                json!({"name": "in.eml", "data": eml, "opened": true}),
                json!({"value": "flagged", "query": "a", "scope": "everywhere"}),
            ];
            for params in &all_params {
                // The setup again, for each try, on what the last one left.
                if setup(&mut s).is_none() {
                    s = fresh();
                    setup(&mut s);
                }
                let Ok(answer) = s.run(id, params) else { continue };
                if SELECTS_AS_IT_IS.contains(id) {
                    continue;
                }
                checked += 1;
                // What a command says it selected is what is selected once it has run.
                if let Some(said) = answer.get("selected").filter(|said| said.is_array()) {
                    assert_eq!(*said, json!(s.sel.messages), "{id} {params}, from {setup_name}: {:?}", s.view);
                }
                let listed: HashSet<Id> = s.rows().iter().flat_map(|r| r.members.iter()).copied().collect();
                let in_view = crate::list::in_view(&s);
                for m in &s.sel.messages {
                    let place = s.mailbox.message(*m).map(|m| (m.folder, m.read));
                    assert!(
                        s.mailbox.message(*m).is_some_and(&in_view),
                        "{id} {params}, from {setup_name}: {m} {place:?} is selected outside the view {:?}",
                        s.view
                    );
                    assert!(
                        listed.contains(m),
                        "{id} {params}, from {setup_name}: {m} {place:?} is selected, and the list doesn't show it ({:?})",
                        s.view
                    );
                }
            }
        }
    }
    assert!(checked > 1000, "the commands did something: {checked}");
}
