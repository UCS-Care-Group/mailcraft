// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The folder and account counts ([`crate::counts`]) say what the mailbox's own counting says,
//! and are counted once per change.

use std::sync::Arc;

use serde_json::json;

use crate::Session;
use crate::counts::FolderCounts;

const NOW: i64 = 1_791_993_600;

#[test]
fn the_counts_are_the_mailboxs_own_for_every_folder_and_account() {
    let mb = crate::sample::mailbox(NOW, 0);
    let counts = FolderCounts::of(&mb);
    assert!(mb.folders.iter().any(|f| mb.unread_count(f.id) > 0), "the sample has unread mail");
    for f in &mb.folders {
        assert_eq!((counts.total(f.id), counts.unread(f.id)), (mb.count(f.id), mb.unread_count(f.id)), "{}", f.name);
    }
    for a in &mb.accounts {
        let own = mb.messages.values().filter(|m| m.account == a.id);
        assert_eq!(counts.account(a.id).total, own.clone().count(), "{}", a.name);
        assert_eq!(counts.account(a.id).unread, own.filter(|m| !m.read).count(), "{}", a.name);
    }
    assert_eq!(counts.messages(), mb.messages.len());
    assert_eq!(counts.folder(0), Default::default(), "a folder of no id holds nothing");
}

#[test]
fn the_counts_are_shared_until_the_mailbox_changes_and_then_counted_again() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let inbox = s.mailbox.inbox().unwrap();
    let first = s.folder_counts();
    assert!(Arc::ptr_eq(&first, &s.folder_counts()), "nothing changed: the same counts");
    let unread = first.unread(inbox);
    assert!(unread > 0);
    let id = s.mailbox.messages_in(inbox).find(|m| !m.read).map(|m| m.id).unwrap();
    s.run("mail.markRead", &json!({"id": id})).unwrap();
    let after = s.folder_counts();
    assert!(!Arc::ptr_eq(&first, &after), "a change: counted again");
    assert_eq!(after.unread(inbox), unread - 1);
    assert_eq!(after.total(inbox), first.total(inbox));
    // A change made outside a command (mail arriving) is noted by the window; the counts follow.
    s.mailbox.message_mut(id).unwrap().read = false;
    assert!(Arc::ptr_eq(&after, &s.folder_counts()), "not noted yet");
    s.mailbox_changed();
    assert_eq!(s.folder_counts().unread(inbox), unread);
}

/// The list built from the folders' own messages ([`crate::list::listing`] with the counts) is
/// the list a walk over every message builds, whatever is in view.
#[test]
fn the_list_built_from_a_folders_own_messages_is_the_list_a_walk_over_every_message_builds() {
    let mb = crate::sample::mailbox(NOW, 0);
    let sent = mb.folders.iter().find(|f| f.kind == mailcraft_model::FolderKind::Sent).map(|f| f.id).unwrap();
    let steps = [
        ("view.filter", json!({"value": "all"})),
        ("view.conversations", json!({"value": true})),
        ("folder.select", json!({"id": sent})),
        ("view.filter", json!({"value": "unread"})),
        ("view.search", json!({"query": "the", "scope": "everywhere"})),
        ("view.search", json!({"query": "the", "scope": "mailbox"})),
        ("view.search", json!({"query": "the", "scope": "subfolders"})),
        ("view.sort", json!({"by": "from"})),
    ];
    let mut listed = 0;
    for n in 1..=steps.len() {
        // Both sessions at the same view, by the same steps; one has counted, the other never.
        let mut counted = Session::at(mb.clone(), NOW, 0);
        let mut walk = Session::at(mb.clone(), NOW, 0);
        for (cmd, params) in steps.iter().take(n) {
            counted.run(cmd, params).unwrap();
            walk.run(cmd, params).unwrap();
        }
        walk.counts_cache = None;
        let built = counted.rows();
        assert!(counted.fresh_counts().is_some(), "step {n}: the rows were built from the counts");
        let walked = crate::list::rows(&walk);
        let shape = |rows: &[crate::list::Row]| rows.iter().map(|r| (r.id, r.members.clone(), r.others.clone(), r.group.clone())).collect::<Vec<_>>();
        assert_eq!(shape(&built), shape(&walked), "step {n}: {}", steps[n - 1].0);
        listed += built.len();
    }
    assert!(listed > 0, "the steps list something");
}

/// A command that changes where a message is, or whether it is read, and then asks of the list
/// gets the list as the mailbox is then, not as it was counted before the command.
#[test]
fn a_command_that_changes_the_mailbox_and_asks_of_the_list_gets_it_as_the_mailbox_is_then() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let inbox = s.mailbox.inbox().unwrap();
    let _ = s.folder_counts();
    s.run("view.filter", &json!({"value": "unread"})).unwrap();
    let before = s.rows();
    let id = before.first().map(|r| r.id).unwrap();
    // Marking read lists what is unread afterwards, from a fresh count.
    s.run("mail.markRead", &json!({"id": id})).unwrap();
    let after = s.rows();
    assert!(!after.iter().any(|r| r.id == id), "read: no longer listed under Unread");
    assert_eq!(after.len(), before.len() - 1);
    assert_eq!(s.folder_counts().unread(inbox), after.len());
}

/// A read mark alone (a message opened, a read mark from the server) leaves the list in view
/// standing with its unread counts put right, rather than built again, while the list is
/// unfiltered and unsearched; anything more, or a filtered list, builds it again.
#[test]
fn read_marks_alone_leave_the_list_and_the_counts_standing_put_right() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let inbox = s.mailbox.inbox().unwrap();
    let rows = s.rows();
    let unread_before = s.folder_counts().unread(inbox);
    let builds = s.list_builds();
    let id = rows.iter().find(|r| r.unread > 0 && r.members.len() == 1).map(|r| r.id).unwrap();
    drop(rows);
    s.run("mail.markRead", &json!({"id": id})).unwrap();
    let rows = s.rows();
    assert_eq!(s.list_builds(), builds, "a read mark alone: the list stands");
    assert_eq!(rows.iter().find(|r| r.id == id).map(|r| r.unread), Some(0), "its row's unread is put right");
    assert_eq!(s.folder_counts().unread(inbox), unread_before - 1, "the folder's unread follows");
    assert_eq!(s.folder_counts().unread(inbox), s.mailbox.unread_count(inbox), "and is the mailbox's own");
    s.run("mail.markUnread", &json!({"id": id})).unwrap();
    assert_eq!(s.list_builds(), builds);
    assert_eq!(s.rows().iter().find(|r| r.id == id).map(|r| r.unread), Some(1));
    assert_eq!(s.folder_counts().unread(inbox), unread_before);
    // Opening a message marks it read: the same.
    let other = s.rows().iter().find(|r| r.unread > 0 && r.id != id).map(|r| r.id).unwrap();
    s.run("mail.select", &json!({"id": other})).unwrap();
    assert!(s.mailbox.message(other).unwrap().read);
    assert_eq!(s.list_builds(), builds, "opening a message: the list stands");
    // A flag is more than a read mark: built again.
    s.run("mail.flag", &json!({"id": id})).unwrap();
    let _ = s.rows();
    assert_eq!(s.list_builds(), builds + 1, "a flag: built again");
    // Under a filter the list depends on the read state: built again.
    s.run("view.filter", &json!({"value": "unread"})).unwrap();
    let rows = s.rows();
    let builds = s.list_builds();
    let id = rows.iter().find(|r| r.members.len() == 1).map(|r| r.id).unwrap();
    drop(rows);
    s.run("mail.markRead", &json!({"id": id})).unwrap();
    let _ = s.rows();
    assert_eq!(s.list_builds(), builds + 1, "filtered: built again");
}
