// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Empty Folder, Delete and Move of a folder's worth of mail (#1037): done in time that grows with
//! the number of messages (a folder of 200,000 took an hour, one message at a time), one step to
//! undo, and the same changes waiting for the server as before.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use mailcraft_model::sync::{RemoteFlags, ServerMessage};
use mailcraft_model::{Account, AccountKind, Address, Directory, FolderKind, Id, Mailbox, Message, OAuth, OAuthProvider, Pending, Queued};
use serde_json::json;

use crate::Session;

/// Messages in the big folder: enough that one at a time, the work's square, takes minutes.
const N: u32 = 100_000;
/// Generous enough for a slow machine running the tests unoptimised.
const SOON: Duration = Duration::from_secs(20);

/// A session whose account's Inbox, Archive and Deleted Items follow IMAP mailboxes, with `n`
/// messages the server has in the folder of `kind`.
fn session(kind: FolderKind, n: u32) -> (Session, Id) {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let account = mb.accounts[0].id;
    for (k, remote) in [(FolderKind::Inbox, "INBOX"), (FolderKind::Archive, "Archive"), (FolderKind::Deleted, "Trash")] {
        let id = mb.folder_of_kind(account, k).unwrap();
        let f = mb.folder_mut(id).unwrap();
        (f.remote, f.uid_validity) = (remote.into(), 7);
    }
    let folder = mb.folder_of_kind(account, kind).unwrap();
    for uid in 1..=n {
        let m = Message {
            folder,
            uid,
            synced: Some(RemoteFlags { uid, ..Default::default() }),
            subject: format!("Message {uid}"),
            message_id: format!("<{uid}@example.org>"),
            date: i64::from(uid),
            from: Address::new("Bob", "bob@example.org"),
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    let mut s = Session::new(mb);
    s.run("folder.select", &json!({"id": folder})).unwrap();
    (s, folder)
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let out = f();
    (out, t.elapsed())
}

#[test]
fn emptying_a_big_deleted_items_is_quick_tells_the_server_and_undoes_in_one_step() {
    let (mut s, deleted) = session(FolderKind::Deleted, N);
    let (answer, took) = timed(|| s.run("folder.empty", &json!({"id": deleted})).unwrap());
    assert!(took < SOON, "emptying {N} took {took:?}");
    assert_eq!(answer["removed"], N);
    assert_eq!(s.mailbox.count(deleted), 0);
    // Each one is deleted where the server has it.
    assert_eq!(s.mailbox.pending.len(), N as usize);
    assert!(s.mailbox.pending.iter().all(|p| matches!(p, Pending::Delete { from, .. } if from == "Trash")));
    // One Undo brings the whole folder back, and nothing waits for the server.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.count(deleted), N as usize);
    assert!(s.mailbox.pending.is_empty());
}

#[test]
fn emptying_a_big_folder_into_deleted_items_and_then_deleted_items_is_quick() {
    let (mut s, inbox) = session(FolderKind::Inbox, N);
    let deleted = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Deleted).unwrap();
    let (_, took) = timed(|| s.run("folder.empty", &json!({"id": inbox})).unwrap());
    assert!(took < SOON, "emptying {N} into Deleted Items took {took:?}");
    assert_eq!(s.mailbox.count(deleted), N as usize);
    assert!(s.mailbox.pending.iter().all(|p| matches!(p, Pending::Move { to, .. } if *to == deleted)));
    // Emptied again before the server heard of the moves: each becomes a deletion from the Inbox.
    s.run("folder.select", &json!({"id": deleted})).unwrap();
    let (_, took) = timed(|| s.run("folder.empty", &json!({"id": deleted})).unwrap());
    assert!(took < SOON, "emptying {N} moved there took {took:?}");
    assert_eq!(s.mailbox.pending.len(), N as usize);
    assert!(s.mailbox.pending.iter().all(|p| matches!(p, Pending::Delete { from, .. } if from == "INBOX")));
    // Each Undo takes back one of the two.
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.count(deleted), N as usize);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.count(inbox), N as usize);
    assert!(s.mailbox.pending.is_empty());
}

#[test]
fn deleting_and_moving_a_big_selection_is_quick_and_undoes_in_one_step() {
    let (mut s, inbox) = session(FolderKind::Inbox, N);
    let account = s.mailbox.accounts[0].id;
    let ids: Vec<Id> = s.mailbox.messages_in(inbox).map(|m| m.id).collect();
    let (answer, took) = timed(|| s.run("mail.archive", &json!({"ids": ids})).unwrap());
    assert!(took < SOON, "archiving {N} took {took:?}");
    assert_eq!(answer["moved"], N);
    s.run("edit.undo", &json!({})).unwrap();
    let (answer, took) = timed(|| s.run("mail.delete", &json!({"ids": ids})).unwrap());
    assert!(took < SOON, "deleting {N} took {took:?}");
    assert_eq!((answer["deleted"].as_u64(), answer["permanently"].as_u64()), (Some(u64::from(N)), Some(0)));
    let deleted = s.mailbox.folder_of_kind(account, FolderKind::Deleted).unwrap();
    assert_eq!(s.mailbox.count(deleted), N as usize);
    let (_, took) = timed(|| s.run("mail.deletePermanently", &json!({"ids": ids})).unwrap());
    assert!(took < SOON, "deleting {N} for good took {took:?}");
    assert!(s.mailbox.messages.is_empty());
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.count(deleted), N as usize);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.count(inbox), N as usize);
    assert!(s.mailbox.pending.is_empty());
}

/// A session with a Microsoft account whose Deleted Items holds `n` messages its server has.
fn microsoft(n: u32) -> (Session, Id) {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let oauth = OAuth { provider: OAuthProvider::Microsoft, directory: Directory::Organisation, ..Default::default() };
    let account =
        mb.add_account(Account { email: "noor@example.org".into(), kind: AccountKind::Microsoft, oauth: Some(oauth), ..Default::default() });
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    for k in 1..=n {
        let server = ServerMessage { id: format!("server-{k}"), folder: "deleteditems".into(), ..Default::default() };
        let m = Message {
            folder: deleted,
            account,
            server: Some(server),
            subject: format!("Message {k}"),
            message_id: format!("<{k}@example.org>"),
            date: i64::from(k),
            from: Address::new("Bob", "bob@example.org"),
            ..Default::default()
        };
        mb.add_message(m).unwrap();
    }
    let mut s = Session::new(mb);
    s.run("folder.select", &json!({"id": deleted})).unwrap();
    (s, deleted)
}

#[test]
fn emptying_a_big_microsoft_deleted_items_is_quick_and_has_the_server_delete_each_once() {
    let (mut s, deleted) = microsoft(N);
    let (answer, took) = timed(|| s.run("folder.empty", &json!({"id": deleted})).unwrap());
    assert!(took < SOON, "emptying {N} took {took:?}");
    assert_eq!(answer["removed"], N);
    let deletes: HashSet<&str> = s
        .mailbox
        .queued
        .iter()
        .filter_map(|q| match q {
            Queued::DeleteMessage { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!((deletes.len(), s.mailbox.queued.len()), (N as usize, N as usize));
}
