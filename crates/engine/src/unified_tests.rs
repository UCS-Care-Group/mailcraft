// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! All Accounts (#625): every account's Inbox in one list, a view over each account's own
//! folder. What is done to a message from it is done in the message's own account, and each
//! account's next Send/Receive asks its own server for it, as from that account's folder.

use mailcraft_model::sync::{FolderDelta, RemoteOp, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Id, Message};
use serde_json::json;

use crate::Session;
use crate::cmd::account::{apply_sync, sync_request};

fn from_server(uid: u32, subject: &str, when: i64) -> Message {
    Message {
        uid,
        subject: subject.into(),
        message_id: format!("{}@mail.example", subject.to_lowercase().replace(' ', ".")),
        from: Address::new("", "someone@example.org"),
        date: when,
        received: when,
        body: Body { text: format!("{subject}."), html: None },
        ..Default::default()
    }
}

/// An IMAP account with an Inbox ("INBOX", holding `mail`), a Trash and an Archive on its server.
fn imap(s: &mut Session, email: &str, mail: Vec<Message>) -> Id {
    let host = email.split('@').nth(1).unwrap();
    let servers = json!({"email": email, "incoming": {"host": format!("imap.{host}")}, "outgoing": {"host": format!("smtp.{host}")}});
    let a = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let delta = |remote: &str, kind, uid_validity, new: Vec<Message>| {
        let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(0);
        FolderDelta { remote: remote.into(), kind, uid_validity, uid_seen, new, ..Default::default() }
    };
    let folders = vec![
        delta("INBOX", FolderKind::Inbox, 7, mail),
        delta("Trash", FolderKind::Deleted, 8, vec![]),
        delta("Archive", FolderKind::Archive, 9, vec![]),
    ];
    apply_sync(s, SyncResult { account: a, folders, ..Default::default() }).unwrap();
    a
}

/// Two IMAP accounts, each with two messages in its Inbox, the newest alternating between them.
fn two() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let a = imap(&mut s, "avery@one.example", vec![from_server(1, "A old", 100), from_server(2, "A new", 300)]);
    let b = imap(&mut s, "avery@two.example", vec![from_server(1, "B old", 200), from_server(2, "B new", 400)]);
    (s, a, b)
}

fn titled(s: &Session, subject: &str) -> Id {
    s.mailbox.messages.values().find(|m| m.subject == subject).unwrap().id
}

fn subjects(s: &mut Session) -> Vec<String> {
    let rows = s.rows();
    rows.iter().map(|r| s.mailbox.message(r.id).unwrap().subject.clone()).collect()
}

fn inbox(s: &Session, account: Id) -> Id {
    s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap()
}

#[test]
fn all_inboxes_lists_every_accounts_inbox_newest_first_without_copying() {
    let (mut s, a, b) = two();
    let before = s.mailbox.messages.len();
    let answer = s.run("view.allAccounts", &json!({})).unwrap();
    assert_eq!(answer["kind"], "inbox");
    assert_eq!(answer["folders"], json!([inbox(&s, a), inbox(&s, b)]));
    assert_eq!(answer["messages"], 4);
    assert_eq!(subjects(&mut s), ["B new", "A new", "B old", "A old"]);
    assert_eq!(s.mailbox.messages.len(), before, "nothing copied");
    // Each message is still in its own account's Inbox.
    assert_eq!(s.mailbox.message(titled(&s, "B new")).unwrap().folder, inbox(&s, b));
}

#[test]
fn delete_from_all_inboxes_goes_to_the_messages_own_trash_and_own_server() {
    let (mut s, a, b) = two();
    s.run("view.allAccounts", &json!({"kind": "inbox"})).unwrap();
    let id = titled(&s, "B new");
    s.run("mail.delete", &json!({"ids": [id]})).unwrap();
    let trash_b = s.mailbox.folder_of_kind(b, FolderKind::Deleted).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().folder, trash_b);
    assert_eq!(s.mailbox.message(id).unwrap().account, b);
    // Account B's server is asked to move it from its INBOX to its Trash; account A's is asked nothing.
    let ops = sync_request(&s, b).unwrap().ops;
    assert!(
        ops.iter()
            .any(|op| matches!(op, RemoteOp::Move { from, uid: 2, to, to_folder, .. } if from == "INBOX" && to == "Trash" && *to_folder == trash_b)),
        "{ops:?}"
    );
    assert!(sync_request(&s, a).unwrap().ops.is_empty());
    // And it leaves the list.
    assert_eq!(subjects(&mut s), ["A new", "B old", "A old"]);
}

#[test]
fn archive_and_move_from_all_inboxes_stay_in_each_messages_account() {
    let (mut s, a, b) = two();
    s.run("view.allAccounts", &json!({})).unwrap();
    let (an, bn) = (titled(&s, "A new"), titled(&s, "B new"));
    s.run("mail.archive", &json!({"ids": [an, bn]})).unwrap();
    assert_eq!(s.mailbox.message(an).unwrap().folder, s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap());
    assert_eq!(s.mailbox.message(bn).unwrap().folder, s.mailbox.folder_of_kind(b, FolderKind::Archive).unwrap());
    for account in [a, b] {
        let ops = sync_request(&s, account).unwrap().ops;
        assert!(matches!(ops.as_slice(), [RemoteOp::Move { from, uid: 2, to, .. }] if from == "INBOX" && to == "Archive"), "{ops:?}");
    }
    // Flag and read act on the message itself, in its own account's folder.
    let bo = titled(&s, "B old");
    s.run("mail.markRead", &json!({"ids": [bo]})).unwrap();
    let ops = sync_request(&s, b).unwrap().ops;
    assert!(ops.iter().any(|op| matches!(op, RemoteOp::Flags { folder, uid: 1, read: Some(true), .. } if folder == "INBOX")), "{ops:?}");
}

#[test]
fn mark_all_read_in_all_inboxes_reads_every_accounts_inbox() {
    let (mut s, a, b) = two();
    s.run("view.allAccounts", &json!({})).unwrap();
    let answer = s.run("mail.markAllRead", &json!({})).unwrap();
    assert_eq!(answer["changed"], 4);
    assert_eq!(s.mailbox.unread_count(inbox(&s, a)) + s.mailbox.unread_count(inbox(&s, b)), 0);
}

#[test]
fn a_search_in_all_inboxes_looks_in_every_accounts_inbox() {
    let (mut s, _, _) = two();
    s.run("view.allAccounts", &json!({})).unwrap();
    s.run("view.search", &json!({"query": "new"})).unwrap();
    assert_eq!(subjects(&mut s), ["B new", "A new"]);
}

#[test]
fn opening_one_accounts_folder_leaves_all_inboxes() {
    let (mut s, a, b) = two();
    s.run("view.allAccounts", &json!({})).unwrap();
    // The view stands on the default account's Inbox: selecting it is one account's again.
    s.run("folder.select", &json!({"id": inbox(&s, a)})).unwrap();
    assert_eq!(crate::unified::of(&s), None);
    assert_eq!(subjects(&mut s), ["A new", "A old"]);
    s.run("view.allAccounts", &json!({})).unwrap();
    s.run("folder.select", &json!({"id": inbox(&s, b)})).unwrap();
    assert_eq!(subjects(&mut s), ["B new", "B old"]);
}

#[test]
fn other_kinds_and_refusals() {
    let (mut s, a, b) = two();
    let answer = s.run("view.allAccounts", &json!({"kind": "deleted"})).unwrap();
    let trash = |acc| s.mailbox.folder_of_kind(acc, FolderKind::Deleted).unwrap();
    assert_eq!(answer["folders"], json!([trash(a), trash(b)]));
    // Empty Folder doesn't guess which account's Deleted Items is meant.
    assert!(s.run("folder.empty", &json!({})).is_err());
    for kind in ["junk", "outbox", "custom", "nonsense"] {
        assert!(s.run("view.allAccounts", &json!({"kind": kind})).is_err(), "{kind}");
    }
    // One account alone: nothing to put together.
    let mut one = Session::empty();
    imap(&mut one, "avery@one.example", vec![]);
    assert!(crate::cmd::registry().get("view.allAccounts").is_some());
    assert!(!crate::unified::offered(&one.mailbox));
}

/// A shared mailbox's folder is listed with the owner's own (Martin's call): its mail is mail the
/// owner works through too. Each message still acts in its own mailbox.
#[test]
fn a_shared_mailbox_is_listed_too() {
    let (mut s, a, b) = two();
    let shared = s.mailbox.accounts.iter().position(|x| x.id == b).unwrap();
    s.mailbox.accounts[shared].shared_of = Some(a);
    assert_eq!(crate::unified::folders(&s.mailbox, FolderKind::Inbox), vec![inbox(&s, a), inbox(&s, b)]);
    // One account of the owner's and a shared mailbox are two mailboxes to put together.
    assert!(crate::unified::offered(&s.mailbox));
    s.run("view.allAccounts", &json!({})).unwrap();
    assert_eq!(subjects(&mut s), ["B new", "A new", "B old", "A old"]);
}

#[test]
fn mail_list_of_a_named_inbox_is_that_inbox_alone() {
    let (mut s, a, b) = two();
    s.run("view.allAccounts", &json!({})).unwrap();
    let listed = s.run("mail.list", &json!({"folder": inbox(&s, a)})).unwrap();
    assert_eq!(listed["total"], 2);
    // Without a folder named, the list in view: every account's Inbox.
    assert_eq!(s.run("mail.list", &json!({})).unwrap()["total"], 4);
    let _ = b;
}

/// Empty Folder that names its folder empties that one, All Deleted Items in view or not; one that
/// names none refuses rather than guess which account's.
#[test]
fn empty_folder_named_in_all_deleted_items_empties_that_one() {
    let (mut s, a, b) = two();
    let trash = |s: &Session, acc| s.mailbox.folder_of_kind(acc, FolderKind::Deleted).unwrap();
    for id in [titled(&s, "A old"), titled(&s, "B old")] {
        s.run("mail.delete", &json!({"ids": [id]})).unwrap();
    }
    s.run("view.allAccounts", &json!({"kind": "deleted"})).unwrap();
    assert!(s.run("folder.empty", &json!({})).is_err());
    assert_eq!(s.mailbox.count(trash(&s, a)) + s.mailbox.count(trash(&s, b)), 2);
    s.run("folder.empty", &json!({"id": trash(&s, b)})).unwrap();
    assert_eq!((s.mailbox.count(trash(&s, a)), s.mailbox.count(trash(&s, b))), (1, 0));
}

/// An open conversation of All Inboxes is closed in a list of one Inbox named by `mail.list`,
/// though that Inbox is the one the view stands on.
#[test]
fn mail_list_of_the_anchor_inbox_closes_all_inboxes_conversations() {
    let (mut s, a, _) = two();
    // A reply to "A new", filed in account A's Archive: part of its conversation.
    let parent = s.mailbox.message(titled(&s, "A new")).unwrap().message_id.clone();
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    let reply = Message { in_reply_to: parent.clone(), references: vec![parent], ..from_server(0, "RE: A new", 350) };
    s.mailbox.add_message(Message { folder: archive, ..reply }).unwrap();
    s.run("view.conversations", &json!({"value": true})).unwrap();
    s.run("view.allAccounts", &json!({})).unwrap();
    s.view.open.push((a, s.threads().key(titled(&s, "A new"))));
    // Open in All Inboxes: the reply is listed under it.
    let here = s.run("mail.list", &json!({})).unwrap();
    assert!(here["messages"].as_array().unwrap().iter().any(|m| m.get("under").is_some()), "{here}");
    let listed = s.run("mail.list", &json!({"folder": inbox(&s, a)})).unwrap();
    assert!(listed["messages"].as_array().unwrap().iter().all(|m| m.get("open").is_none() && m.get("under").is_none()), "{listed}");
    // The window's own view is as it was.
    assert_eq!(s.view.open.len(), 1);
}

/// A search of the current mailbox from All Inboxes looks in every listed account's mailbox, not
/// only the one the view stands on.
#[test]
fn a_mailbox_search_in_all_inboxes_covers_every_account() {
    let (mut s, a, b) = two();
    s.run("view.allAccounts", &json!({})).unwrap();
    // One message of each account in its Archive: out of the Inbox, still in its mailbox.
    for id in [titled(&s, "A old"), titled(&s, "B old")] {
        s.run("mail.archive", &json!({"ids": [id]})).unwrap();
    }
    s.run("view.search", &json!({"query": "old", "scope": "mailbox"})).unwrap();
    assert_eq!(subjects(&mut s), ["B old", "A old"]);
    let _ = (a, b);
}

/// A message of a shared mailbox selected in All Inboxes and deleted comes back selected and listed
/// with Undo, the view still every account's Inbox.
#[test]
fn undo_in_all_inboxes_brings_a_shared_mailboxs_message_back_into_the_list() {
    let (mut s, a, b) = two();
    let shared = s.mailbox.accounts.iter().position(|x| x.id == b).unwrap();
    s.mailbox.accounts[shared].shared_of = Some(a);
    s.run("view.allAccounts", &json!({})).unwrap();
    let id = titled(&s, "B new");
    s.run("mail.select", &json!({"id": id})).unwrap();
    s.run("mail.delete", &json!({})).unwrap();
    assert!(!subjects(&mut s).contains(&"B new".to_string()));
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(crate::unified::of(&s), Some(FolderKind::Inbox));
    assert_eq!(s.sel.messages, vec![id]);
    assert!(subjects(&mut s).contains(&"B new".to_string()));
}

/// With one mailbox, `view.allAccounts` from a script, an agent or the control port is refused
/// where commands are run (its `when`), with the reason the window gives, and the view stays as
/// it was: never an All Inboxes the folder pane has no row for.
#[test]
fn one_mailbox_refuses_all_accounts_at_dispatch() {
    let mut s = Session::empty();
    imap(&mut s, "avery@one.example", vec![from_server(1, "Only", 100)]);
    let before = s.view.clone();
    let refused = s.run("view.allAccounts", &json!({"kind": "inbox"})).unwrap_err();
    assert!(matches!(refused, crate::CmdError::Disabled(ref why) if why.contains("two mailboxes")), "{refused:?}");
    assert_eq!(s.view, before);
    assert_eq!(crate::unified::of(&s), None);
}
