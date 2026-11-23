// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Other users' shared IMAP folders that an earlier version kept side by side at the top of the
//! account (`INBOX (2)`, `Archiv (2)`) go to their place in the server's tree at the next run:
//! moved, given back the server's name, never made twice.
use mailcraft_model::sync::{FolderDelta, ListedFolder, SyncResult};
use mailcraft_model::{FolderKind, Id};
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
