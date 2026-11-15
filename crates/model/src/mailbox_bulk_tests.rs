// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Many messages removed, deleted or moved at once (#1037): the same changes for the servers as
//! one message at a time, in time that grows with the number of messages.

use std::time::{Duration, Instant};

use crate::sync::{RemoteFlags, ServerMessage};
use crate::*;

/// A mailbox with an account whose Inbox, Archive and Deleted Items follow IMAP mailboxes and a
/// Microsoft account, both with mail of the server's.
struct Fixture {
    mb: Mailbox,
    imap: Id,
    microsoft: Id,
}

fn folder(mb: &Mailbox, account: Id, kind: FolderKind) -> Id {
    mb.folder_of_kind(account, kind).unwrap()
}

fn fixture() -> Fixture {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let imap = mb.accounts[0].id;
    for (kind, remote) in [(FolderKind::Inbox, "INBOX"), (FolderKind::Archive, "Archive"), (FolderKind::Deleted, "Trash")] {
        let id = folder(&mb, imap, kind);
        let f = mb.folder_mut(id).unwrap();
        (f.remote, f.uid_validity) = (remote.into(), 7);
    }
    let oauth = OAuth { provider: OAuthProvider::Microsoft, directory: Directory::Organisation, ..Default::default() };
    let microsoft =
        mb.add_account(Account { email: "noor@example.org".into(), kind: AccountKind::Microsoft, oauth: Some(oauth), ..Default::default() });
    Fixture { mb, imap, microsoft }
}

fn mail(folder: Id, n: u32) -> Message {
    Message {
        folder,
        subject: format!("Message {n}"),
        message_id: format!("<{n}@example.org>"),
        date: i64::from(n),
        from: Address::new("Bob", "bob@example.org"),
        ..Default::default()
    }
}

/// `n` messages the IMAP server has in `folder`, UIDs from `first`.
fn imap_mail(mb: &mut Mailbox, folder: Id, first: u32, n: u32) -> Vec<Id> {
    (first..first + n)
        .map(|uid| mb.add_message(Message { uid, synced: Some(RemoteFlags { uid, ..Default::default() }), ..mail(folder, uid) }).unwrap())
        .collect()
}

/// `n` messages the Microsoft server has in `folder`.
fn graph_mail(mb: &mut Mailbox, folder: Id, first: u32, n: u32) -> Vec<Id> {
    (first..first + n)
        .map(|k| {
            let server = ServerMessage { id: format!("graph-{k}"), folder: "de".into(), ..Default::default() };
            mb.add_message(Message { server: Some(server), ..mail(folder, k) }).unwrap()
        })
        .collect()
}

/// A mailbox with a bit of everything that waits for a server: moves of the IMAP server's mail
/// not pushed yet, mail only this computer has, Microsoft mail, a copy waiting to be made, a
/// deletion queued already, and a to-do made from a message. Answers the messages, in an order
/// that mixes them up.
fn busy() -> (Mailbox, Vec<Id>) {
    let Fixture { mut mb, imap, microsoft } = fixture();
    let (inbox, archive, deleted) =
        (folder(&mb, imap, FolderKind::Inbox), folder(&mb, imap, FolderKind::Archive), folder(&mb, imap, FolderKind::Deleted));
    let mut ids = imap_mail(&mut mb, inbox, 1, 6);
    // Two moved into Archive and one into Deleted Items, none of it pushed yet.
    mb.move_messages(&ids[..2], archive).unwrap();
    mb.move_messages(&ids[2..3], deleted).unwrap();
    ids.extend(imap_mail(&mut mb, deleted, 100, 3));
    ids.push(mb.add_message(mail(inbox, 200)).unwrap());
    let graph_deleted = folder(&mb, microsoft, FolderKind::Deleted);
    let graph_inbox = folder(&mb, microsoft, FolderKind::Inbox);
    let graph = graph_mail(&mut mb, graph_deleted, 300, 3);
    // One the server is already deleting (queued earlier), and a copy waiting to be made of another.
    let copy = mb.copy_message(graph[1], graph_inbox).unwrap();
    mb.queued.push(Queued::DeleteMessage { account: microsoft, id: "graph-300".into(), kept: None });
    ids.extend(graph);
    ids.push(copy);
    ids.extend(graph_mail(&mut mb, graph_inbox, 400, 2));
    mb.add_task(Task { title: "Follow up".into(), message: Some(ids[4]), ..Default::default() });
    // Mixed up, and one named twice.
    ids.rotate_left(5);
    ids.push(ids[0]);
    (mb, ids)
}

/// What a server is to be told and what is left here.
fn state(mb: &Mailbox) -> (Vec<Pending>, Vec<Queued>, Vec<Id>, Vec<Option<Id>>) {
    let messages = mb.messages.values().map(|m| m.id).collect();
    let tasks = mb.tasks.values().map(|t| t.message).collect();
    (mb.pending.clone(), mb.queued.clone(), messages, tasks)
}

#[test]
fn removing_many_tells_the_servers_what_removing_each_in_turn_does() {
    let (mut one_by_one, ids) = busy();
    let mut at_once = one_by_one.clone();
    let mut removed = 0;
    for id in &ids {
        removed += usize::from(one_by_one.remove_message(*id).is_ok());
    }
    assert_eq!(at_once.remove_messages(&ids), Ok(removed));
    assert_eq!(state(&at_once), state(&one_by_one));
    // Every move waiting became a deletion where the server has the message, and the Microsoft
    // server deletes each of its messages once.
    assert!(at_once.pending.iter().all(|p| matches!(p, Pending::Delete { .. })));
    assert_eq!(at_once.pending.len(), 9);
    let graph_deletes = at_once.queued.iter().filter(|q| matches!(q, Queued::DeleteMessage { .. })).count();
    assert_eq!(graph_deletes, 5);
    assert!(!at_once.queued.iter().any(|q| matches!(q, Queued::CopyMessage { .. })));
    assert!(at_once.messages.is_empty() && at_once.tasks.values().all(|t| t.message.is_none()));
}

#[test]
fn deleting_many_tells_the_servers_what_deleting_each_in_turn_does() {
    let (mut one_by_one, ids) = busy();
    let mut at_once = one_by_one.clone();
    // What moves goes first: deleted in that order one by one, the changes come in the same order.
    let (moving, staying): (Vec<Id>, Vec<Id>) = {
        let mb = &one_by_one;
        let bins: Vec<Id> = mb.accounts.iter().filter_map(|a| mb.folder_of_kind(a.id, FolderKind::Deleted)).collect();
        let mut seen = std::collections::HashSet::new();
        ids.iter().copied().filter(|id| seen.insert(*id)).partition(|id| mb.message(*id).is_some_and(|m| !bins.contains(&m.folder)))
    };
    let mut gone = 0;
    for id in moving.iter().chain(&staying) {
        gone += usize::from(one_by_one.delete_message(*id).unwrap());
    }
    assert_eq!(at_once.delete_messages(&ids), Ok(gone));
    assert_eq!(state(&at_once), state(&one_by_one));
    assert_eq!(gone, staying.len());
    // What moved is in Deleted Items, with its move waiting for the server.
    assert!(moving.iter().all(|id| at_once.message(*id).is_some()));
    assert!(staying.iter().all(|id| at_once.message(*id).is_none()));
}

#[test]
fn a_message_that_isnt_here_or_stays_on_its_server_stops_the_removal() {
    let Fixture { mut mb, imap, .. } = fixture();
    let inbox = folder(&mb, imap, FolderKind::Inbox);
    let ids = imap_mail(&mut mb, inbox, 1, 3);
    assert!(mb.remove_messages(&[ids[0], 9_999_999]).is_err());
    assert!(mb.delete_messages(&[9_999_999]).is_err());
    // What went before it went whole: no move is left waiting for it.
    assert!(mb.message(ids[0]).is_none() && mb.message(ids[1]).is_some());
    assert!(!mb.pending.iter().any(|p| matches!(p, Pending::Move { .. })));
}

/// How long `f` takes.
fn timed(f: impl FnOnce()) -> Duration {
    let t = Instant::now();
    f();
    t.elapsed()
}

/// Generous enough for a slow machine running the tests unoptimised; one message at a time, the
/// work grows with the square of the count and takes minutes.
const SOON: Duration = Duration::from_secs(10);

#[test]
fn deleting_and_then_emptying_a_big_imap_folder_is_linear() {
    const N: u32 = 100_000;
    let Fixture { mut mb, imap, .. } = fixture();
    let inbox = folder(&mb, imap, FolderKind::Inbox);
    let deleted = folder(&mb, imap, FolderKind::Deleted);
    let ids = imap_mail(&mut mb, inbox, 1, N);
    // Into Deleted Items: a move each, waiting for the server.
    let took = timed(|| assert_eq!(mb.delete_messages(&ids), Ok(0)));
    assert!(took < SOON, "deleting {N} took {took:?}");
    assert_eq!(mb.pending.len(), N as usize);
    assert_eq!(mb.count(deleted), N as usize);
    // Emptied before the server heard: each move becomes a deletion where the server has it.
    let took = timed(|| assert_eq!(mb.remove_messages(&ids), Ok(N as usize)));
    assert!(took < SOON, "emptying {N} took {took:?}");
    assert_eq!(mb.pending.len(), N as usize);
    assert!(mb.pending.iter().all(|p| matches!(p, Pending::Delete { from, .. } if from == "INBOX")));
    assert!(mb.messages.is_empty());
}

#[test]
fn emptying_a_big_microsoft_folder_is_linear() {
    const N: u32 = 100_000;
    let Fixture { mut mb, microsoft, .. } = fixture();
    let deleted = folder(&mb, microsoft, FolderKind::Deleted);
    let ids = graph_mail(&mut mb, deleted, 1, N);
    let took = timed(|| assert_eq!(mb.remove_messages(&ids), Ok(N as usize)));
    assert!(took < SOON, "emptying {N} took {took:?}");
    assert_eq!(mb.queued.len(), N as usize);
    assert!(mb.messages.is_empty());
}

#[test]
fn moving_a_big_selection_is_linear() {
    const N: u32 = 100_000;
    let Fixture { mut mb, imap, .. } = fixture();
    let (inbox, archive) = (folder(&mb, imap, FolderKind::Inbox), folder(&mb, imap, FolderKind::Archive));
    let ids = imap_mail(&mut mb, inbox, 1, N);
    let moves: Vec<(Id, Id)> = ids.iter().map(|id| (*id, archive)).collect();
    let took = timed(|| assert_eq!(mb.move_each(&moves).map(|m| m.len()), Ok(N as usize)));
    assert!(took < SOON, "moving {N} took {took:?}");
    assert_eq!(mb.pending.len(), N as usize);
}
