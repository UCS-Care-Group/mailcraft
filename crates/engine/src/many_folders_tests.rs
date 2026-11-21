// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #991: a run that lists 10,000 folders has them all here, in the server's tree, and taking
//! that in is quick.
use std::time::{Duration, Instant};

use mailcraft_model::sync::{ListedFolder, MAX_LISTED_FOLDERS, SyncResult};
use mailcraft_model::{FolderKind, Id};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::*;

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn listed(remote: &str, parent: &str, name: &str) -> ListedFolder {
    ListedFolder { remote: remote.into(), kind: FolderKind::Custom, parent: parent.into(), name: name.into() }
}

#[test]
fn ten_thousand_listed_folders_are_all_here_in_their_tree() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let added = s.run("account.add", &json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    let a = added["id"].as_u64().unwrap();
    // Children listed before their parents, as a server may, a thousand of them side by side.
    let mut tree = vec![ListedFolder { kind: FolderKind::Inbox, ..listed("INBOX", "", "Inbox") }];
    let mut n = 0;
    while tree.len() < MAX_LISTED_FOLDERS {
        let top = format!("Area {}", n % 9);
        let leaf = format!("Box {n:05}");
        tree.push(listed(&format!("{top}/{leaf}/Old"), &format!("{top}/{leaf}"), "Old"));
        tree.push(listed(&format!("{top}/{leaf}"), &top, &leaf));
        if n < 9 {
            tree.push(listed(&top, "", &top));
        }
        n += 1;
    }
    tree.truncate(MAX_LISTED_FOLDERS);
    // One more past what a listing holds is not taken in.
    tree.push(listed("Beyond", "", "Beyond"));
    let before = s.mailbox.folders.iter().filter(|f| f.account == a).count();
    let started = Instant::now();
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: tree.clone(), partial: true, ..Default::default() }).unwrap();
    let took = started.elapsed();
    let by_remote: std::collections::HashMap<&str, (Id, Option<Id>, &str)> = s
        .mailbox
        .folders
        .iter()
        .filter(|f| f.account == a && !f.remote.is_empty())
        .map(|f| (f.remote.as_str(), (f.id, f.parent, f.name.as_str())))
        .collect();
    // Every folder listed is here (the Inbox the account had already), and nothing past them.
    assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == a).count(), before + MAX_LISTED_FOLDERS - 1);
    assert!(!by_remote.contains_key("Beyond"));
    // Each under its parent, by its last part.
    for f in tree.iter().take(MAX_LISTED_FOLDERS).filter(|f| !f.parent.is_empty()) {
        let (_, parent, name) = by_remote[f.remote.as_str()];
        assert_eq!((parent, name), (Some(by_remote[f.parent.as_str()].0), f.name.as_str()), "{}", f.remote);
    }
    assert!(took < Duration::from_secs(10), "{took:?}");

    // Said again, as every run says it: nothing changes.
    let folders = s.mailbox.folders.clone();
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: tree, partial: true, ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.folders, folders);
}

/// A run over 10,000 folders hands over a piece for each, and a host takes the waiting ones in
/// together: the whole account is read once for each batch, not once for each piece (#991).
#[test]
fn ten_thousand_pieces_of_one_run_are_taken_in_quickly() {
    use mailcraft_model::sync::FolderDelta;

    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let added = s.run("account.add", &json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    let a = added["id"].as_u64().unwrap();
    let remotes: Vec<String> = (0..MAX_LISTED_FOLDERS).map(|n| format!("Box {n:05}")).collect();
    let tree: Vec<ListedFolder> = remotes.iter().map(|r| listed(r, "", r)).collect();
    // The tree first, then each folder read in a piece of its own, the last one ending the run.
    let mut pieces = vec![SyncResult { account: a, listed: tree, partial: true, ..Default::default() }];
    pieces.extend(remotes.iter().enumerate().map(|(n, remote)| SyncResult {
        account: a,
        folders: vec![FolderDelta { remote: remote.clone(), uid_validity: 7, uid_seen: 1, complete: true, ..Default::default() }],
        partial: n + 1 < MAX_LISTED_FOLDERS,
        ..Default::default()
    }));
    let started = Instant::now();
    let mut run = cmd::account::SyncTotals::default();
    let mut pieces = pieces.into_iter().peekable();
    while pieces.peek().is_some() {
        let batch: Vec<SyncResult> = pieces.by_ref().take(cmd::account::MAX_PIECES_TOGETHER).collect();
        cmd::account::apply_sync_parts(&mut s, &mut run, batch).unwrap();
    }
    let took = started.elapsed();
    assert!(run.finished && run.errors.is_empty(), "{run:?}");
    let read = s.mailbox.folders.iter().filter(|f| f.account == a && f.remote.starts_with("Box ") && f.uid_validity == 7).count();
    assert_eq!(read, MAX_LISTED_FOLDERS);
    assert!(took < Duration::from_secs(10), "{took:?}");
    // Nothing to take in is no piece.
    assert!(cmd::account::apply_sync_parts(&mut s, &mut run, Vec::new()).is_err());
}

/// A folder whose reading comes in two pieces, the messages read again with the last (as a run
/// hands them over): taken in together, the mailbox is as it is when they are taken in one after
/// the other, and what was read again is taken in (#991).
#[test]
fn pieces_taken_in_together_are_pieces_taken_in_one_by_one() {
    use mailcraft_model::sync::FolderDelta;
    use mailcraft_model::{Address, Message};

    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let added = s.run("account.add", &json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap();
    let a = added["id"].as_u64().unwrap();
    let mail = |uid: u32, subject: &str| Message {
        uid,
        subject: subject.into(),
        from: Address::new("Bob", "bob@example.org"),
        date: 100 + i64::from(uid),
        message_id: format!("<{uid}@example.org>"),
        ..Default::default()
    };
    let inbox =
        |new: Vec<Message>| FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 2, new, ..Default::default() };
    let first = SyncResult { account: a, folders: vec![inbox(vec![mail(1, "Garbled \u{FFFD}"), mail(2, "Two")])], ..Default::default() };
    cmd::account::apply_sync(&mut s, first).unwrap();
    // Both marked to be read again.
    let ids: Vec<Id> = s.mailbox.messages.values().filter(|m| m.account == a).map(|m| m.id).collect();
    assert_eq!(ids.len(), 2);
    for id in ids {
        s.mailbox.message_mut(id).unwrap().refetch = true;
    }
    let again = FolderDelta { uid_seen: 3, refetched: vec![mail(1, "Readable")], refetch_done: vec![2], complete: true, ..inbox(Vec::new()) };
    let pieces = vec![
        SyncResult { account: a, folders: vec![FolderDelta { uid_seen: 3, ..inbox(vec![mail(3, "Three")]) }], partial: true, ..Default::default() },
        SyncResult { account: a, folders: vec![again], ..Default::default() },
    ];
    let mut one_by_one = Session::at(s.mailbox.clone(), now(), 0);
    let mut run = cmd::account::SyncTotals::default();
    for piece in pieces.clone() {
        cmd::account::apply_sync_part(&mut one_by_one, &mut run, piece).unwrap();
    }
    let mut together = Session::at(s.mailbox.clone(), now(), 0);
    cmd::account::apply_sync_parts(&mut together, &mut cmd::account::SyncTotals::default(), pieces).unwrap();
    assert!(together.mailbox == one_by_one.mailbox);
    let mine: Vec<&Message> = together.mailbox.messages.values().map(|m| m.as_ref()).filter(|m| m.account == a).collect();
    assert_eq!(mine.iter().find(|m| m.uid == 1).map(|m| m.subject.as_str()), Some("Readable"));
    assert!(mine.iter().filter(|m| m.uid != 3).all(|m| !m.refetch && m.refetched));
    assert_eq!(mine.len(), 3);
}
