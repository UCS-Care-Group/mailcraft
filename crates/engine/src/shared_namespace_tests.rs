// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Other users' shared IMAP folders that an earlier version kept side by side at the top of the
//! account (`INBOX (2)`, `Archiv (2)`) go to their place in the server's tree at the next run:
//! moved, given back the server's name, never made twice.
use mailcraft_model::sync::{FolderDelta, ListedFolder, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Message};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use crate::*;

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn listed(remote: &str, kind: FolderKind, parent: &str, name: &str) -> ListedFolder {
    ListedFolder { remote: remote.into(), kind, parent: parent.into(), name: name.into() }
}

fn delta(remote: &str, kind: FolderKind) -> FolderDelta {
    FolderDelta { remote: remote.into(), kind, uid_validity: 7, complete: true, ..Default::default() }
}

/// The account's folders that follow the server, as (path, parent's path, name).
fn tree(s: &Session, account: Id) -> Vec<(String, String, String)> {
    let folders: Vec<_> = s.mailbox.folders.iter().filter(|f| f.account == account && !f.remote.is_empty()).collect();
    let path = |id: Option<Id>| id.and_then(|id| folders.iter().find(|f| f.id == id)).map(|f| f.remote.clone()).unwrap_or_default();
    let mut out: Vec<_> = folders.iter().map(|f| (f.remote.clone(), path(f.parent), f.name.clone())).collect();
    out.sort();
    out
}

#[test]
fn shared_folders_kept_flat_move_into_their_tree() {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap();
    let a = added["id"].as_u64().unwrap();
    let carol = "shared/carol@example.org";
    // What an earlier version made of the listing: the account's own folders, and the others'
    // named by their last part at the top, numbered where that was taken.
    let old = SyncResult {
        account: a,
        listed: vec![listed("INBOX", FolderKind::Inbox, "", "INBOX"), listed("Archiv", FolderKind::Custom, "", "Archiv")],
        folders: vec![
            delta("INBOX", FolderKind::Inbox),
            delta("Archiv", FolderKind::Custom),
            delta(&format!("{carol}/INBOX"), FolderKind::Custom),
            delta(&format!("{carol}/Archiv"), FolderKind::Custom),
            delta("shared/dave@example.org/INBOX", FolderKind::Custom),
        ],
        ..Default::default()
    };
    cmd::account::apply_sync(&mut s, old).unwrap();
    let flat = tree(&s, a);
    let at_top: Vec<&str> =
        flat.iter().filter(|(remote, parent, _)| remote.starts_with("shared/") && parent.is_empty()).map(|f| f.2.as_str()).collect();
    assert_eq!(at_top.len(), 3, "{flat:?}");
    assert!(at_top.iter().any(|name| name.ends_with(')')), "{flat:?}");
    let ids_before: Vec<Id> = s.mailbox.folders.iter().filter(|f| f.account == a && f.remote.starts_with("shared/")).map(|f| f.id).collect();

    // The next run lists them in their tree, with the folders above them.
    let new = vec![
        listed("INBOX", FolderKind::Inbox, "", "INBOX"),
        listed("Archiv", FolderKind::Custom, "", "Archiv"),
        listed("shared", FolderKind::Custom, "", "shared"),
        listed(carol, FolderKind::Custom, "shared", "carol@example.org"),
        listed(&format!("{carol}/INBOX"), FolderKind::Custom, carol, "INBOX"),
        listed(&format!("{carol}/Archiv"), FolderKind::Custom, carol, "Archiv"),
        listed("shared/dave@example.org", FolderKind::Custom, "shared", "dave@example.org"),
        listed("shared/dave@example.org/INBOX", FolderKind::Custom, "shared/dave@example.org", "INBOX"),
    ];
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: new.clone(), partial: true, ..Default::default() }).unwrap();
    let shared: Vec<(String, String, String)> = tree(&s, a).into_iter().filter(|f| f.0.starts_with("shared")).collect();
    let want = |r: &str, p: &str, n: &str| (r.to_string(), p.to_string(), n.to_string());
    assert_eq!(
        shared,
        [
            want("shared", "", "shared"),
            want(carol, "shared", "carol@example.org"),
            want(&format!("{carol}/Archiv"), carol, "Archiv"),
            want(&format!("{carol}/INBOX"), carol, "INBOX"),
            want("shared/dave@example.org", "shared", "dave@example.org"),
            want("shared/dave@example.org/INBOX", "shared/dave@example.org", "INBOX"),
        ]
    );
    // The same folders, moved: none made twice.
    for id in ids_before {
        assert!(s.mailbox.folder(id).is_some_and(|f| f.parent.is_some()), "{id}");
    }
    // The account's own stay as they are.
    let own: Vec<_> = tree(&s, a).into_iter().filter(|f| !f.0.starts_with("shared")).collect();
    assert_eq!(own.iter().find(|f| f.0 == "Archiv").map(|f| (f.1.as_str(), f.2.as_str())), Some(("", "Archiv")));
    let inbox = s.mailbox.folders.iter().filter(|f| f.account == a && f.kind == FolderKind::Inbox).map(|f| f.remote.as_str()).collect::<Vec<_>>();
    assert_eq!(inbox, ["INBOX"]);

    // Said again, as every run says it: nothing changes.
    let folders = s.mailbox.folders.clone();
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: new, partial: true, ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.folders, folders);
}

fn sent_copy(uid: u32) -> Message {
    Message {
        uid,
        from: Address::new("Ada", "ada@example.org"),
        subject: format!("Sent {uid}"),
        message_id: format!("{uid}.sent@example.org"),
        date: 1_700_000_000,
        ..Default::default()
    }
}

/// An account whose Sent Items and Deleted Items an earlier version tied to Carol's shared
/// `\Sent` and `\Trash`, with a message in each (#1863). Returns the session, the account,
/// and the ids of those two folders.
fn tied_to_carol(own: bool) -> (Session, Id, Id, Id) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap();
    let a = added["id"].as_u64().unwrap();
    let carol = "shared/carol@example.org";
    let mut listing = vec![
        listed("INBOX", FolderKind::Inbox, "", "INBOX"),
        listed(&format!("{carol}/Sent"), FolderKind::Sent, "", &format!("{carol}/Sent")),
        listed(&format!("{carol}/Trash"), FolderKind::Deleted, "", &format!("{carol}/Trash")),
    ];
    let mut folders = vec![
        delta("INBOX", FolderKind::Inbox),
        FolderDelta { new: vec![sent_copy(1)], uid_seen: 1, ..delta(&format!("{carol}/Sent"), FolderKind::Sent) },
        FolderDelta { new: vec![sent_copy(2)], uid_seen: 2, ..delta(&format!("{carol}/Trash"), FolderKind::Deleted) },
    ];
    if own {
        listing.push(listed("Sent", FolderKind::Custom, "", "Sent"));
        folders.push(delta("Sent", FolderKind::Custom));
    }
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: listing, folders, ..Default::default() }).unwrap();
    let of = |s: &Session, kind| s.mailbox.folder_of_kind(a, kind).unwrap();
    let (sent, deleted) = (of(&s, FolderKind::Sent), of(&s, FolderKind::Deleted));
    assert_eq!(s.mailbox.folder(sent).map(|f| f.remote.as_str()), Some("shared/carol@example.org/Sent"));
    assert_eq!(s.mailbox.folder(deleted).map(|f| f.remote.as_str()), Some("shared/carol@example.org/Trash"));
    (s, a, sent, deleted)
}

/// What the next run lists of that account: Carol's folders in her tree, as ordinary ones.
fn listing_now(own: bool) -> Vec<ListedFolder> {
    let carol = "shared/carol@example.org";
    let mut out = vec![
        listed("INBOX", FolderKind::Inbox, "", "INBOX"),
        listed("shared", FolderKind::Custom, "", "shared"),
        listed(carol, FolderKind::Custom, "shared", "carol@example.org"),
        listed(&format!("{carol}/Sent"), FolderKind::Custom, carol, "Sent"),
        listed(&format!("{carol}/Trash"), FolderKind::Custom, carol, "Trash"),
    ];
    if own {
        out.push(listed("Sent", FolderKind::Sent, "", "Sent"));
    }
    out
}

/// A standard folder an earlier version tied to another user's mailbox gives its job up at the
/// next run (#1863): it stays, with its mail, as an ordinary folder in that mailbox's place in
/// the tree, and the job goes to the account's own mailbox's folder, or to a new empty one.
#[test]
fn a_standard_folder_tied_to_anothers_mailbox_gives_its_job_up() {
    let carol = "shared/carol@example.org";
    let (mut s, a, sent, deleted) = tied_to_carol(true);
    let own_sent = s.mailbox.folders.iter().find(|f| f.account == a && f.remote == "Sent").map(|f| f.id).unwrap();
    let mail_before: Vec<(Id, Id)> = s.mailbox.messages.values().filter(|m| m.account == a).map(|m| (m.id, m.folder)).collect();

    let now = listing_now(true);
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: now.clone(), partial: true, ..Default::default() }).unwrap();
    // The folders that had the jobs follow Carol's mailboxes still, as ordinary folders in her tree,
    // under their names there.
    let shared: Vec<(String, String, String)> = tree(&s, a).into_iter().filter(|f| f.0.starts_with(carol)).collect();
    let want = |r: &str, p: &str, n: &str| (r.to_string(), p.to_string(), n.to_string());
    assert_eq!(
        shared,
        [want(carol, "shared", "carol@example.org"), want(&format!("{carol}/Sent"), carol, "Sent"), want(&format!("{carol}/Trash"), carol, "Trash")]
    );
    for id in [sent, deleted] {
        assert_eq!(s.mailbox.folder(id).map(|f| f.kind), Some(FolderKind::Custom));
    }
    // Sent Items is the folder of the account's own Sent; Deleted Items, which has no mailbox of
    // the account's own listed, is a new empty folder here, under its own name.
    assert_eq!(s.mailbox.folder_of_kind(a, FolderKind::Sent), Some(own_sent));
    let new_deleted = s.mailbox.folder_of_kind(a, FolderKind::Deleted).unwrap();
    assert!(new_deleted != deleted);
    assert_eq!(s.mailbox.folder(new_deleted).map(|f| (f.name.as_str(), f.remote.as_str(), f.parent)), Some(("Deleted Items", "", None)));
    // No message lost or moved.
    let mail_after: Vec<(Id, Id)> = s.mailbox.messages.values().filter(|m| m.account == a).map(|m| (m.id, m.folder)).collect();
    assert_eq!(mail_after, mail_before);
    // One folder of each kind still.
    for kind in FolderKind::STANDARD {
        assert_eq!(s.mailbox.folders.iter().filter(|f| f.account == a && f.kind == kind).count(), 1, "{kind:?}");
    }

    // Said again, as every run says it: nothing changes.
    let folders = s.mailbox.folders.clone();
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: now, partial: true, ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.folders, folders);
}

/// Without a mailbox of the account's own for the job, Sent Items is a new empty folder too; it
/// takes the account's own mailbox once the server lists one.
#[test]
fn without_an_own_mailbox_the_job_goes_to_a_new_folder() {
    let (mut s, a, sent, _) = tied_to_carol(false);
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: listing_now(false), partial: true, ..Default::default() }).unwrap();
    let new_sent = s.mailbox.folder_of_kind(a, FolderKind::Sent).unwrap();
    assert!(new_sent != sent);
    assert_eq!(s.mailbox.folder(new_sent).map(|f| (f.name.as_str(), f.remote.as_str())), Some(("Sent Items", "")));
    assert!(s.mailbox.messages.values().any(|m| m.folder == sent));
    cmd::account::apply_sync(&mut s, SyncResult { account: a, listed: listing_now(true), partial: true, ..Default::default() }).unwrap();
    assert_eq!(s.mailbox.folder(new_sent).map(|f| f.remote.as_str()), Some("Sent"));
    assert_eq!(s.mailbox.folder_of_kind(a, FolderKind::Sent), Some(new_sent));
}
