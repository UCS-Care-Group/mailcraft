// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Undo for a Microsoft account, as the engine merges it. (Against the Graph stand-in, end to
//! end: `apps/mailcraft-cli/tests/graph_undo.rs`.)

use mailcraft_model::sync::MessageChange;
use mailcraft_model::{Account, ServerFolder};
use serde_json::json;

use super::*;
use crate::Session;
use crate::cmd::graph_up::message_changes;

/// Friday 2027-01-15 08:00 UTC.
const NOW: i64 = 1_800_000_000;

/// A session with a Microsoft account whose special folders the server has, a message in its
/// Inbox the server has, and a local account's folder: the session, the account, the message and
/// the local folder.
fn setup() -> (Session, Id, Id, Id) {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let kinds = [FolderKind::Inbox, FolderKind::Archive, FolderKind::Deleted, FolderKind::Drafts, FolderKind::Outbox];
    for (n, kind) in kinds.into_iter().enumerate() {
        let folder = s.mailbox.folder_of_kind(account, kind).unwrap();
        s.mailbox.folder_mut(folder).unwrap().server =
            Some(ServerFolder { id: format!("F-{n}"), name: kind.default_name().into(), ..Default::default() });
    }
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let server = ServerMessage { id: "M-1".into(), folder: "F-0".into(), ..Default::default() };
    let m = s.mailbox.add_message(Message { folder: inbox, subject: "Invoice 42".into(), server: Some(server), ..Default::default() }).unwrap();
    let local = s.mailbox.add_account(Account { name: "On this computer".into(), ..Default::default() });
    let kept = s.mailbox.add_folder(local, None, "Kept").unwrap();
    (s, account, m, kept)
}

fn undo(s: &mut Session) -> Vec<String> {
    let v = s.run("edit.undo", &json!({})).unwrap();
    serde_json::from_value(v["notUndone"].clone()).unwrap()
}

#[test]
fn a_mailbox_without_a_microsoft_account_is_undone_as_ever() {
    let mut s = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let inbox = s.mailbox.inbox().unwrap();
    let m = s.mailbox.add_message(Message { folder: inbox, subject: "Local".into(), ..Default::default() }).unwrap();
    let before = s.mailbox.clone();
    s.run("mail.flag", &json!({"ids": [m]})).unwrap();
    assert!(touched(&before, &s.mailbox).messages.is_empty());
    s.run("mail.deletePermanently", &json!({"ids": [m]})).unwrap();
    assert!(undo(&mut s).is_empty());
    assert!(undo(&mut s).is_empty());
    assert_eq!(s.mailbox.messages, before.messages);
}

#[test]
fn a_change_taken_in_from_the_server_keeps_the_history_and_undo_reverses_it() {
    let (mut s, account, m, _) = setup();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": archive})).unwrap();
    // As if the move went up.
    s.mailbox.message_mut(m).unwrap().server.as_mut().unwrap().folder = "F-1".into();
    assert!(message_changes(&s, account, 10).0.is_empty());
    assert!(undo(&mut s).is_empty());
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().folder, inbox);
    // What the server last said stays: the difference is the move back.
    let changes = message_changes(&s, account, 10).0;
    assert!(matches!(changes.as_slice(), [MessageChange::Move { to, .. }] if to == "F-0"), "{changes:?}");
    // Redo: back in Archive, which is where the server has it: nothing to send.
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().folder, archive);
    assert!(message_changes(&s, account, 10).0.is_empty());
}

#[test]
fn a_folder_deleted_for_good_stays_deleted_and_one_never_on_the_server_comes_back() {
    let (mut s, account, _, _) = setup();
    let made = s.run("folder.new", &json!({"name": "Receipts", "account": account, "parent": null})).unwrap()["id"].as_u64().unwrap();
    let local_only = s.run("folder.new", &json!({"name": "Scratch", "account": account, "parent": null})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.folder_mut(made).unwrap().server = Some(ServerFolder { id: "F-r".into(), name: "Receipts".into(), ..Default::default() });
    // Into Deleted Items, then out of the mailbox for good.
    for folder in [made, made, local_only, local_only] {
        s.run("folder.delete", &json!({"id": folder})).unwrap();
    }
    assert!(s.mailbox.folder(made).is_none() && s.mailbox.folder(local_only).is_none());
    let mut said = Vec::new();
    for _ in 0..4 {
        said.extend(undo(&mut s));
    }
    assert!(s.mailbox.folder(local_only).is_some_and(|f| f.parent.is_none()), "never on the server: back as ever");
    assert_eq!(said, ["The folder 'Receipts' was permanently deleted: that can't be undone here."]);
    assert!(s.mailbox.folder(made).is_none());
}

#[test]
fn a_folder_the_change_made_that_the_server_has_goes_into_deleted_items() {
    let (mut s, account, _, _) = setup();
    let made = s.run("folder.new", &json!({"name": "Receipts", "account": account, "parent": null})).unwrap()["id"].as_u64().unwrap();
    // As if it went up.
    s.mailbox.folder_mut(made).unwrap().server = Some(ServerFolder { id: "F-r".into(), name: "Receipts".into(), ..Default::default() });
    assert!(undo(&mut s).is_empty());
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    assert_eq!(s.mailbox.folder(made).unwrap().parent, Some(deleted));
}

#[test]
fn a_move_out_of_the_account_is_undone_only_while_the_servers_copy_is_there() {
    let (mut s, account, m, kept) = setup();
    s.run("mail.move", &json!({"ids": [m], "folder": kept})).unwrap();
    assert!(s.mailbox.message(m).unwrap().moving_out.is_some());
    assert!(undo(&mut s).is_empty());
    let back = s.mailbox.message(m).unwrap();
    assert_eq!((back.account, back.moving_out.as_ref(), back.server.as_ref().map(|s| s.id.as_str())), (account, None, Some("M-1")));
    assert!(message_changes(&s, account, 10).0.is_empty());

    s.run("edit.redo", &json!({})).unwrap();
    assert!(s.mailbox.message(m).unwrap().moving_out.is_some());
    // The server's copy removed: it stays where it was moved.
    s.mailbox.message_mut(m).unwrap().moving_out = None;
    let said = undo(&mut s);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(s.mailbox.message(m).unwrap().folder, kept);
}

#[test]
fn undo_with_the_account_gone_or_the_folders_gone_never_panics() {
    let (mut s, account, m, _) = setup();
    let archive = s.mailbox.folder_of_kind(account, FolderKind::Archive).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": archive})).unwrap();
    s.run("mail.flag", &json!({"ids": [m]})).unwrap();
    // The server took Archive away meanwhile, with the message in it.
    s.mailbox.folders.retain(|f| f.id != archive);
    s.mailbox.messages.remove(&m);
    s.mailbox_changed();
    undo(&mut s);
    undo(&mut s);
    assert!(s.mailbox.message(m).is_none(), "gone from the server: not brought back");
    s.run("edit.redo", &json!({})).unwrap();
    s.run("account.remove", &json!({"id": account})).unwrap();
    while s.can_undo() {
        s.run("edit.undo", &json!({})).unwrap();
    }
    while s.can_redo() {
        s.run("edit.redo", &json!({})).unwrap();
    }
}
