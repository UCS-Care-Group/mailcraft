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
    // A flag changes nothing the list is built from (unfiltered): it stands too.
    s.run("mail.flag", &json!({"id": id})).unwrap();
    let rows = s.rows();
    assert_eq!(s.list_builds(), builds, "a flag: the list stands");
    assert_eq!(*rows, crate::list::rows(&s), "and is the list built again");
    drop(rows);
    // A pin moves the message to the top: built again.
    s.run("mail.pin", &json!({"id": id})).unwrap();
    let _ = s.rows();
    assert_eq!(s.list_builds(), builds + 1, "a pin: built again");
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

/// Counts left from an earlier revision aren't carried through read marks. A command that is no
/// step of the history (`mail.select`, a Send/Receive's pages) changes the mailbox without the
/// counts being let go of first, so counts from before it may still be held when a later one
/// changes read marks alone: they are of another mailbox, and stay behind. (Here they are
/// planted, as between two frames nothing asks for them in between.)
#[test]
fn counts_from_an_earlier_revision_are_not_carried_through_read_marks() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let inbox = s.mailbox.inbox().unwrap();
    let rows = s.rows();
    let right = s.folder_counts();
    let id = rows.iter().find(|r| r.unread > 0 && r.members.len() == 1).map(|r| r.id).unwrap();
    drop(rows);
    // Counts of nothing, left from the revision before the one the list was built under.
    let stale = Arc::new(FolderCounts::of(&mailcraft_model::Mailbox::default()));
    s.counts_cache = Some(((s.rev().wrapping_sub(1), s.mailbox.messages.len()), stale));
    // Opening the message marks it read, and is no step of the history.
    s.run("mail.select", &json!({"id": id})).unwrap();
    assert!(s.mailbox.message(id).unwrap().read);
    let after = s.folder_counts();
    assert_eq!(after.total(inbox), right.total(inbox), "counted afresh, not the stale ones put right");
    assert_eq!(after.unread(inbox), right.unread(inbox) - 1);
    assert_eq!(after.unread(inbox), s.mailbox.unread_count(inbox));
}

/// Counts a command builds as it runs are of the mailbox after its read marks already: Undo and
/// Redo show what they select again, which builds the list, and so the counts, for a message
/// listed under its open conversation; carried through the marks as well, they would have each
/// mark twice. Only the counts that stood before the command are carried over.
#[test]
fn counts_built_as_a_command_runs_are_not_carried_through_its_read_marks() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    s.run("view.conversations", &json!({"value": true})).unwrap();
    let row = s.rows().iter().find(|r| !r.others.is_empty()).cloned().unwrap();
    let other = row.others[0];
    let (folder, account) = s.mailbox.message(other).map(|m| (m.folder, m.account)).unwrap();
    s.run("mail.expandConversation", &json!({"id": row.id})).unwrap();
    s.run("mail.select", &json!({"id": other})).unwrap();
    let _ = s.rows();
    let _ = s.folder_counts();
    s.run("mail.markUnread", &json!({"id": other})).unwrap();
    for step in ["edit.undo", "edit.redo"] {
        s.run(step, &json!({})).unwrap();
        let counts = s.folder_counts();
        assert_eq!(counts.unread(folder), s.mailbox.unread_count(folder), "{step}: the folder's unread");
        assert_eq!(counts.account(account).unread, s.mailbox.messages.values().filter(|m| m.account == account && !m.read).count(), "{step}");
    }
    assert!(!s.mailbox.message(other).unwrap().read, "redone: unread again");
}

/// Selecting is a reading command (no copy of the mailbox is taken for it), and the message it
/// opens is marked read once it is done: the list stands, the counts follow, the mailbox is to be
/// saved, and no step of the history is made. A range or a selection that fails marks nothing.
#[test]
fn a_selection_marks_what_it_opens_read_after_it_without_a_step() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let inbox = s.mailbox.inbox().unwrap();
    let rows = s.rows();
    let unread: Vec<_> = rows.iter().filter(|r| r.unread > 0 && r.members.len() == 1).map(|r| r.id).collect();
    assert!(unread.len() >= 2, "the sample's Inbox has unread mail");
    drop(rows);
    let (builds, steps, unread_before) = (s.list_builds(), s.history.len(), s.folder_counts().unread(inbox));
    s.dirty = false;

    // A selection that fails changes nothing.
    assert!(s.run("mail.select", &json!({"id": unread[0], "copy": 99})).is_err());
    assert!(!s.mailbox.message(unread[0]).unwrap().read);
    assert!(!s.dirty);

    s.run("mail.select", &json!({"id": unread[0]})).unwrap();
    assert!(s.mailbox.message(unread[0]).unwrap().read, "opened: read");
    assert!(s.dirty, "and to be saved");
    assert_eq!(s.history.len(), steps, "no step of the history");
    assert_eq!(s.folder_counts().unread(inbox), unread_before - 1);
    assert_eq!(s.folder_counts().unread(inbox), s.mailbox.unread_count(inbox));
    assert_eq!(s.rows().iter().find(|r| r.id == unread[0]).map(|r| r.unread), Some(0));
    assert_eq!(s.list_builds(), builds, "the list stands");

    // A Shift+click over unread mail marks none of it: more than one is selected.
    s.run("mail.select", &json!({"id": unread[1], "range": true})).unwrap();
    assert!(s.sel.messages.len() > 1);
    assert!(!s.mailbox.message(unread[1]).unwrap().read, "a range opens nothing");
    assert_eq!(s.folder_counts().unread(inbox), s.mailbox.unread_count(inbox));

    // An arrow key that lands on an unread message marks it read the same way.
    s.run("mail.select", &json!({"id": unread[1]})).unwrap();
    assert!(s.mailbox.message(unread[1]).unwrap().read);
    assert_eq!(s.folder_counts().unread(inbox), s.mailbox.unread_count(inbox));
    assert_eq!(s.history.len(), steps);

    // Under the Unread filter the list is built again, and the message opened stays in it (#428).
    s.run("mail.markUnread", &json!({"ids": [unread[0], unread[1]]})).unwrap();
    s.run("view.filter", &json!({"value": "unread"})).unwrap();
    s.run("mail.select", &json!({"id": unread[0]})).unwrap();
    assert!(s.mailbox.message(unread[0]).unwrap().read);
    assert!(s.rows().iter().any(|r| r.id == unread[0]), "kept while it is selected");
    assert_eq!(s.folder_counts().unread(inbox), s.mailbox.unread_count(inbox));
}

/// A change of any kind (a flag, a move, a delete, mail coming, Undo and Redo) leaves the counts
/// brought in step with it what counting again over every message would make them, and doesn't
/// count again over every message for it.
#[test]
fn counts_brought_in_step_with_a_change_are_those_counted_again() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let inbox = s.mailbox.inbox().unwrap();
    let account = s.mailbox.folder(inbox).unwrap().account;
    let archive = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Archive).unwrap();
    let ids: Vec<_> = s.mailbox.messages_in(inbox).map(|m| m.id).take(6).collect();
    assert!(ids.len() >= 6, "the sample's Inbox has mail");
    let check = |s: &mut Session, what: &str| {
        let kept = s.folder_counts();
        assert_eq!(*kept, FolderCounts::of(&s.mailbox), "after {what}");
        let rows = s.rows();
        assert_eq!(*rows, crate::list::rows(s), "the list after {what} is the list built again");
    };
    check(&mut s, "nothing");
    let steps: Vec<(&str, serde_json::Value)> = vec![
        ("mail.flag", json!({"id": ids[0]})),
        ("mail.toggleFlag", json!({"id": ids[1]})),
        ("mail.pin", json!({"id": ids[1]})),
        ("mail.markRead", json!({"ids": [ids[1], ids[2]]})),
        ("mail.markUnread", json!({"id": ids[1]})),
        ("mail.move", json!({"ids": [ids[2], ids[3]], "folder": archive})),
        ("mail.delete", json!({"id": ids[4]})),
        ("mail.deletePermanently", json!({"id": ids[5]})),
        ("edit.undo", json!({})),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
        ("mail.move", json!({"id": ids[0], "folder": 999_999})),
    ];
    for (command, params) in steps {
        let rows = s.rows();
        drop(rows);
        let _ = s.folder_counts();
        let _ = s.run(command, &params);
        check(&mut s, command);
    }
    // Mail coming into the Inbox, as a run brings it: counted in step too.
    let mut m = (**s.mailbox.messages.get(&ids[0]).unwrap()).clone();
    m.subject = "Arrived".into();
    m.read = false;
    let _ = s.folder_counts();
    s.mailbox.add_message(m).unwrap();
    // (Only the counts notice a change made outside a command, by the number of messages.)
    assert_eq!(*s.folder_counts(), FolderCounts::of(&s.mailbox), "after a message added outside a command");
}

/// Counts of a mailbox that changed outside a command under the same revision (mail arriving
/// before the window notes it) aren't carried through the next command: they would leave out
/// what came. The message count beside the revision tells them apart, as for
/// [`Session::folder_counts`].
#[test]
fn counts_of_a_mailbox_changed_outside_a_command_are_counted_again_after_the_next() {
    for command in ["mail.flag", "mail.markRead", "mail.move"] {
        let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
        let inbox = s.mailbox.inbox().unwrap();
        let account = s.mailbox.folder(inbox).unwrap().account;
        let archive = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Archive).unwrap();
        let id = s.mailbox.messages_in(inbox).find(|m| !m.read).map(|m| m.id).unwrap();
        let mut m = (**s.mailbox.messages.get(&id).unwrap()).clone();
        m.subject = "Arrived".into();
        m.read = false;
        let _ = s.folder_counts();
        s.mailbox.add_message(m).unwrap();
        let params = if command == "mail.move" { json!({"id": id, "folder": archive}) } else { json!({"id": id}) };
        s.run(command, &params).unwrap();
        assert_eq!(*s.folder_counts(), FolderCounts::of(&s.mailbox), "after `{command}`");
    }
}

/// A message an agent's selection opens is marked read off the light path: the mark moves the
/// revision on, so that a save of the revision before it never counts the mark as saved.
#[test]
fn a_read_mark_off_the_light_path_moves_the_revision_on() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    s.prefs.mark_read_on_open = true;
    let inbox = s.mailbox.inbox().unwrap();
    let id = s.mailbox.messages_in(inbox).find(|m| !m.read && !m.draft).map(|m| m.id).unwrap();
    s.dirty = false;
    let before = s.rev();
    s.agent = Some(Vec::new());
    s.run("mail.select", &json!({"id": id})).unwrap();
    s.agent = None;
    assert!(s.mailbox.message(id).unwrap().read);
    assert!(s.dirty && s.rev() != before);
}
