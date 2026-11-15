// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A folder deleted for good goes to the server with what the owner had of it, and a server that
//! holds more there keeps it (AU3-H7): the folder comes back and the owner is told how much is new.
//! Every name and id is invented.

use mailcraft_model::Message;
use mailcraft_model::sync::{FolderChangeDone, ServerMessage, Shown};
use serde_json::json;

use super::*;

const ACCOUNT_EMAIL: &str = "owner@example.org";

fn folder(id: &str, parent: &str, name: &str, kind: FolderKind) -> GraphFolder {
    GraphFolder { id: id.into(), parent: parent.into(), name: name.into(), kind, ..Default::default() }
}

/// Deleted Items with "Old projects" in it and "2025" in that.
fn server() -> Vec<GraphFolder> {
    vec![
        folder("in", "", "Inbox", FolderKind::Inbox),
        folder("de", "", "Deleted Items", FolderKind::Deleted),
        folder("old", "de", "Old projects", FolderKind::Custom),
        folder("ch", "old", "2025", FolderKind::Custom),
    ]
}

fn take(s: &mut Session, account: Id, folders: Vec<GraphFolder>, changes: Vec<FolderChangeDone>) -> Value {
    let result = GraphMailResult {
        account,
        email: ACCOUNT_EMAIL.into(),
        listing: Some(FolderListing { folders, complete: true }),
        changes,
        ..Default::default()
    };
    apply_graph_mail(s, result).unwrap()
}

/// A session whose Microsoft account has the server's folders, and a message the server has in
/// "2025"; the account.
fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ACCOUNT_EMAIL, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    take(&mut s, account, server(), Vec::new());
    let ch = s.mailbox.folder_by_server(account, "ch").unwrap();
    let server = ServerMessage { id: "m1".into(), folder: "ch".into(), ..Default::default() };
    s.mailbox.add_message(Message { folder: ch, subject: "Plan".into(), server: Some(server), ..Default::default() }).unwrap();
    (s, account)
}

#[test]
fn the_deletion_carries_what_the_owner_had_of_the_folder() {
    let (mut s, account) = session();
    let old = s.mailbox.folder_by_server(account, "old").unwrap();
    s.run("folder.delete", &json!({"id": old})).unwrap();
    let request = graph_mail_request(&s, account).unwrap();
    assert_eq!(request.changes, [FolderChange::Delete { id: "old".into(), shown: Shown::of("Old projects", ["old", "ch"], ["m1"]) }]);
}

#[test]
fn a_deletion_the_server_kept_for_what_is_new_there_brings_the_folder_back_and_says_how_much() {
    let (mut s, account) = session();
    let old = s.mailbox.folder_by_server(account, "old").unwrap();
    s.run("folder.delete", &json!({"id": old})).unwrap();
    assert!(s.mailbox.folder_by_server(account, "old").is_none());
    let change = graph_mail_request(&s, account).unwrap().changes.remove(0);
    let kept = FolderChangeDone { change, outcome: ChangeOutcome::NotShown { items: 3 } };
    // The server still has them: the listing brings them back.
    let answer = take(&mut s, account, server(), vec![kept]);
    let errors = answer["errors"].to_string();
    assert!(errors.contains("Old projects") && errors.contains("3 new items"), "{errors}");
    assert!(s.mailbox.folder_by_server(account, "old").is_some() && s.mailbox.folder_by_server(account, "ch").is_some());
    // Nothing waits to be deleted: deleting it again asks again.
    assert!(s.mailbox.queued.is_empty());
    assert!(graph_mail_request(&s, account).unwrap().changes.iter().all(|c| !matches!(c, FolderChange::Delete { .. })));
}

#[test]
fn one_new_item_is_said_as_one() {
    let (mut s, account) = session();
    let old = s.mailbox.folder_by_server(account, "old").unwrap();
    s.run("folder.delete", &json!({"id": old})).unwrap();
    let change = graph_mail_request(&s, account).unwrap().changes.remove(0);
    let answer = take(&mut s, account, server(), vec![FolderChangeDone { change, outcome: ChangeOutcome::NotShown { items: 1 } }]);
    assert!(answer["errors"].to_string().contains("1 new item in it"), "{answer}");
}

#[test]
fn an_imap_folder_deleted_for_good_asks_nothing_of_its_server() {
    // (An IMAP server is never asked to delete a folder: what it holds there stays, AU3-H7.)
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let old = s.mailbox.add_folder(account, Some(deleted), "Old projects").unwrap();
    s.run("folder.delete", &json!({"id": old})).unwrap();
    assert!(s.mailbox.folder(old).is_none() && s.mailbox.queued.is_empty());
}
