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
