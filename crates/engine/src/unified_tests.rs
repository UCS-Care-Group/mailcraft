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

#[test]
fn a_shared_mailbox_stays_out() {
    let (mut s, a, b) = two();
    let shared = s.mailbox.accounts.iter().position(|x| x.id == b).unwrap();
    s.mailbox.accounts[shared].shared_of = Some(a);
    assert_eq!(crate::unified::folders(&s.mailbox, FolderKind::Inbox), vec![inbox(&s, a)]);
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
