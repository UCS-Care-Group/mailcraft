// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use std::sync::Arc;

use crate::mailbox::{MAX_ACCOUNTS, MAX_CATEGORIES, MAX_FOLDERS};
use crate::*;

fn mailbox() -> (Mailbox, Id) {
    let mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let account = mb.accounts[0].id;
    (mb, account)
}

fn mail(folder: Id, subject: &str, date: i64) -> Message {
    Message { folder, subject: subject.into(), date, from: Address::new("Bob", "bob@example.org"), ..Default::default() }
}

#[test]
fn junk_includes_its_subfolders() {
    let (mut mb, account) = mailbox();
    let junk = mb.folder_of_kind(account, FolderKind::Junk).unwrap();
    let inbox = mb.inbox().unwrap();
    let sub = mb.add_folder(account, Some(junk), "Sorted").unwrap();
    let deeper = mb.add_folder(account, Some(sub), "Older").unwrap();
    let elsewhere = mb.add_folder(account, Some(inbox), "Sorted").unwrap();
    assert!([junk, sub, deeper].iter().all(|f| mb.in_junk(*f)));
    assert!(![inbox, elsewhere, 9_999].iter().any(|f| mb.in_junk(*f)));
}

#[test]
fn a_new_account_has_the_standard_folders_and_a_calendar() {
    let (mb, account) = mailbox();
    let names: Vec<&str> = mb.children(account, None).iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Inbox", "Drafts", "Sent Items", "Deleted Items", "Archive", "Junk Email", "Outbox"]);
    assert_eq!(mb.inbox(), mb.folder_of_kind(account, FolderKind::Inbox));
    assert_eq!(mb.calendars.len(), 1);
    assert_eq!(mb.categories.len(), 6);
    assert_eq!(mb.default_account().map(|a| a.address().full()).as_deref(), Some("Ada Lovelace <ada@example.org>"));
}

#[test]
fn folders_nest_rename_move_and_delete() {
    let (mut mb, account) = mailbox();
    let inbox = mb.inbox().unwrap();
    let projects = mb.add_folder(account, Some(inbox), " Projects ").unwrap();
    let apollo = mb.add_folder(account, Some(projects), "Apollo").unwrap();
    assert_eq!(mb.folder_path(apollo), "Inbox/Projects/Apollo");
    assert_eq!(mb.subtree(projects), [projects, apollo]);
    // Names are unique among siblings, whatever the case.
    assert!(mb.add_folder(account, Some(inbox), "projects").is_err());
    assert!(mb.add_folder(account, Some(inbox), "  ").is_err());
    assert!(mb.add_folder(account, Some(inbox), "a/b").is_err());
    assert!(mb.add_folder(999, None, "x").is_err());
    mb.rename_folder(apollo, "Gemini").unwrap();
    assert!(mb.rename_folder(inbox, "In").is_err());
    // No cycles.
    assert!(mb.move_folder(projects, Some(apollo)).is_err());
    assert!(mb.move_folder(inbox, None).is_err());
    mb.move_folder(apollo, None).unwrap();
    assert_eq!(mb.folder_path(apollo), "Gemini");

    let m = mb.add_message(mail(projects, "Kickoff", 10)).unwrap();
    // First delete: into Deleted Items, messages and all.
    assert_eq!(mb.delete_folder(projects), Ok(false));
    assert_eq!(mb.folder_path(projects), "Deleted Items/Projects");
    assert!(mb.message(m).is_some());
    // Second delete: gone for good.
    assert_eq!(mb.delete_folder(projects), Ok(true));
    assert!(mb.folder(projects).is_none() && mb.message(m).is_none());
    assert!(mb.delete_folder(inbox).is_err());
}

#[test]
fn deep_nesting_is_refused() {
    let (mut mb, account) = mailbox();
    let mut parent = mb.inbox();
    let mut made = 0;
    for i in 0..100 {
        match mb.add_folder(account, parent, &format!("level {i}")) {
            Ok(id) => {
                parent = Some(id);
                made += 1;
            }
            Err(_) => break,
        }
    }
    assert!(made < 40, "{made}");
}

#[test]
fn messages_move_copy_and_delete_in_two_steps() {
    let (mut mb, account) = mailbox();
    let inbox = mb.inbox().unwrap();
    let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let a = mb.add_message(Message { uid: 44, ..mail(inbox, "One", 1) }).unwrap();
    let b = mb.add_message(Message { read: true, ..mail(inbox, "Two", 2) }).unwrap();
    assert_ne!(a, b);
    assert_eq!((mb.count(inbox), mb.unread_count(inbox), mb.badge(inbox)), (2, 1, 1));
    assert!(mb.add_message(mail(4242, "Nowhere", 3)).is_err());

    mb.move_message(a, archive).unwrap();
    assert_eq!(mb.message(a).map(|m| (m.folder, m.uid)), Some((archive, 0)));
    let copy = mb.copy_message(a, inbox).unwrap();
    assert_eq!(mb.message(copy).map(|m| m.subject.as_str()), Some("One"));
    assert!(mb.move_message(a, 4242).is_err() && mb.move_message(4242, inbox).is_err());

    assert_eq!(mb.delete_message(a), Ok(false));
    assert_eq!(mb.message(a).map(|m| m.folder), Some(deleted));
    assert_eq!(mb.badge(deleted), 0);
    assert_eq!(mb.delete_message(a), Ok(true));
    assert!(mb.message(a).is_none());
    assert!(mb.delete_message(a).is_err());
}

#[test]
fn snapshots_share_messages_until_one_changes() {
    let (mut mb, _) = mailbox();
    let inbox = mb.inbox().unwrap();
    let a = mb.add_message(mail(inbox, "One", 1)).unwrap();
    let b = mb.add_message(mail(inbox, "Two", 2)).unwrap();
    let snapshot = mb.clone();
    mb.message_mut(a).unwrap().read = true;
    assert!(!Arc::ptr_eq(&mb.messages[&a], &snapshot.messages[&a]));
    assert!(Arc::ptr_eq(&mb.messages[&b], &snapshot.messages[&b]));
    assert!(!snapshot.message(a).unwrap().read);
}

#[test]
fn conversations_follow_the_topic_and_skip_deleted_mail() {
    let (mut mb, account) = mailbox();
    let inbox = mb.inbox().unwrap();
    let sent = mb.folder_of_kind(account, FolderKind::Sent).unwrap();
    let first = mb.add_message(mail(inbox, "Lunch?", 10)).unwrap();
    let reply = mb.add_message(mail(sent, "RE: lunch?", 20)).unwrap();
    let again = mb.add_message(mail(inbox, "Re: RE: Lunch?", 30)).unwrap();
    let other = mb.add_message(mail(inbox, "Dinner?", 15)).unwrap();
    assert_eq!(mb.conversation(reply), [first, reply, again]);
    assert_eq!(mb.conversation(other), [other]);
    mb.delete_message(again).unwrap();
    assert_eq!(mb.conversation(first), [first, reply]);
    // Read from Deleted Items, the deleted message still shows its conversation.
    assert_eq!(mb.conversation(again), [first, reply, again]);
    assert!(mb.conversation(4242).is_empty());
}

#[test]
fn rules_run_on_new_mail() {
    let (mut mb, account) = mailbox();
    let inbox = mb.inbox().unwrap();
    let news = mb.add_folder(account, None, "News").unwrap();
    mb.rules = vec![
        Rule {
            id: 1,
            name: "Digest".into(),
            enabled: true,
            conditions: vec![Condition::SubjectContains("digest".into())],
            actions: vec![Action::MoveTo(news), Action::MarkRead, Action::Categorize("Blue category".into())],
            stop: true,
        },
        Rule {
            id: 2,
            name: "Flag Bob".into(),
            enabled: true,
            conditions: vec![Condition::FromContains("bob".into())],
            actions: vec![Action::Flag],
            stop: false,
        },
        Rule {
            id: 3,
            name: "Gone".into(),
            enabled: true,
            conditions: vec![Condition::SubjectContains("hello".into())],
            actions: vec![Action::MoveTo(4242), Action::Delete],
            stop: false,
        },
    ];
    let digest = mb.add_message(mail(inbox, "Weekly Digest", 1)).unwrap();
    assert_eq!(mb.apply_rules(digest), ["Digest"]);
    let m = mb.message(digest).unwrap();
    assert_eq!((m.folder, m.read, m.flag, m.categories.len()), (news, true, FlagStatus::None, 1));
    let hello = mb.add_message(mail(inbox, "Hello", 2)).unwrap();
    assert_eq!(mb.apply_rules(hello), ["Flag Bob", "Gone"]);
    let m = mb.message(hello).unwrap();
    assert_eq!((m.flag, mb.folder(m.folder).map(|f| f.kind)), (FlagStatus::Flagged, Some(FolderKind::Deleted)));
    assert!(mb.apply_rules(4242).is_empty());
}

#[test]
fn categories_rename_and_remove_everywhere() {
    let (mut mb, _) = mailbox();
    let inbox = mb.inbox().unwrap();
    let m = mb.add_message(Message { categories: vec!["Red category".into()], ..mail(inbox, "x", 1) }).unwrap();
    let c = mb.add_contact(Contact { first: "Bob".into(), categories: vec!["red CATEGORY".into(), "Other".into()], ..Default::default() });
    mb.rename_category("red category", "Urgent").unwrap();
    assert_eq!(mb.message(m).unwrap().categories, ["Urgent"]);
    assert_eq!(mb.contact(c).unwrap().categories, ["Urgent", "Other"]);
    assert!(mb.rename_category("Urgent", "Blue category").is_err());
    assert!(mb.rename_category("Nope", "X").is_err());
    mb.set_category("urgent", Rgb(1, 2, 3)).unwrap();
    assert_eq!(mb.category_color("Urgent"), Rgb(1, 2, 3));
    assert_eq!(mb.categories.len(), 6);
    mb.remove_category("URGENT").unwrap();
    assert!(mb.message(m).unwrap().categories.is_empty());
    assert_eq!(mb.contact(c).unwrap().categories, ["Other"]);
    assert_eq!(mb.category_color("Urgent"), Rgb(0x8A, 0x8A, 0x8A));
    assert!(mb.remove_category("Urgent").is_err());
}

#[test]
fn contacts_events_and_tasks() {
    let (mut mb, _) = mailbox();
    let c = mb.add_contact(Contact { first: "Grace".into(), emails: vec![Labeled::new("work", "grace@example.org")], ..Default::default() });
    assert_eq!(mb.contact_by_email("GRACE@example.org").map(|x| x.id), Some(c));
    mb.contact_mut(c).unwrap().favorite = true;
    mb.remove_contact(c).unwrap();
    assert!(mb.remove_contact(c).is_err());

    // An event with an unknown calendar lands in the default one.
    let e = mb.add_event(Event { calendar: 4242, title: "Stand-up".into(), ..Default::default() }).unwrap();
    assert_eq!(mb.event(e).map(|x| x.calendar), mb.default_calendar());
    mb.remove_event(e).unwrap();

    let inbox = mb.inbox().unwrap();
    let m = mb.add_message(mail(inbox, "Follow up", 1)).unwrap();
    let t = mb.add_task(Task { title: "Follow up".into(), message: Some(m), ..Default::default() });
    // Removing the message leaves the to-do, unlinked.
    mb.remove_message(m).unwrap();
    assert_eq!(mb.task(t).map(|x| x.message), Some(None));
    mb.remove_task(t).unwrap();
}

#[test]
fn a_mailbox_round_trips_as_json() {
    let (mut mb, account) = mailbox();
    let inbox = mb.inbox().unwrap();
    mb.add_folder(account, Some(inbox), "Projects").unwrap();
    mb.add_message(Message { attachments: vec![Attachment::new("a.txt", "text/plain", b"hi".to_vec())], ..mail(inbox, "With file", 5) }).unwrap();
    mb.add_contact(Contact { first: "Bob".into(), ..Default::default() });
    mb.add_event(Event { title: "Review".into(), recurrence: Some(Recurrence::every(Freq::Weekly)), ..Default::default() }).unwrap();
    mb.add_task(Task { title: "Write tests".into(), ..Default::default() });
    mb.rules.push(Rule {
        id: 1,
        name: "r".into(),
        enabled: true,
        conditions: vec![Condition::HasAttachment],
        actions: vec![Action::Flag],
        stop: false,
    });
    let json = serde_json::to_string(&mb).unwrap();
    let mut back: Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(back, mb);
    assert!(back.repair().is_empty());
    assert_eq!(back, mb);
    // New ids don't collide with loaded ones.
    let fresh = back.alloc_id();
    assert!(!back.messages.contains_key(&fresh) && back.folder(fresh).is_none());
}

#[test]
fn repair_fixes_a_hostile_file() {
    let json = r#"{
        "accounts": [{"id": 1, "name": "A"}, {"id": 1, "name": "B"}],
        "folders": [
            {"id": 5, "account": 1, "name": "Inbox", "kind": "inbox"},
            {"id": 6, "account": 1, "name": "Also Inbox", "kind": "inbox"},
            {"id": 7, "account": 1, "name": "Loop A", "parent": 8},
            {"id": 8, "account": 1, "name": "Loop B", "parent": 7},
            {"id": 9, "account": 77, "name": "Orphan"},
            {"id": 10, "account": 1, "name": "", "parent": 4242}
        ],
        "messages": {
            "3": {"id": 99, "folder": 5, "subject": "ok"},
            "4": {"id": 4, "folder": 4242, "subject": "homeless"},
            "18446744073709551615": {"folder": 9, "subject": "max"}
        },
        "events": {"20": {"calendar": 4242, "title": "e"}},
        "nextId": 2
    }"#;
    let mut mb: Mailbox = serde_json::from_str(json).unwrap();
    let fixed = mb.repair();
    assert!(!fixed.is_empty());
    // Every account has distinct ids and all its standard folders.
    assert_ne!(mb.accounts[0].id, mb.accounts[1].id);
    for a in &mb.accounts {
        for kind in FolderKind::STANDARD {
            assert_eq!(mb.folders.iter().filter(|f| f.account == a.id && f.kind == kind).count(), 1, "{kind:?}");
        }
    }
    // No loops, no dangling parents, no empty names.
    for f in &mb.folders {
        assert!(!mb.folder_path(f.id).is_empty() && !f.name.is_empty());
        assert!(f.parent.is_none_or(|p| mb.folder(p).is_some()));
    }
    assert!(mb.folders.iter().all(|f| f.name != "Orphan"));
    // Messages have their key as id and live in a real folder.
    for (id, m) in &mb.messages {
        assert_eq!(*id, m.id);
        assert!(mb.folder(m.folder).is_some_and(|f| f.account == m.account));
    }
    assert_eq!(mb.messages.len(), 3);
    assert_eq!(mb.event(20).map(|e| e.calendar), mb.default_calendar());
    // Ids keep working after the largest possible one.
    let id = mb.alloc_id();
    assert!(id >= 1);
    // A second pass has nothing left to fix.
    assert!(mb.repair().is_empty(), "{:?}", mb.clone().repair());
}

/// The flags a server would have for a message nobody has read.
fn unread(uid: u32) -> sync::RemoteFlags {
    sync::RemoteFlags { uid, ..Default::default() }
}

/// A mailbox whose Inbox, Archive and Deleted Items follow server mailboxes, with a message the
/// server has as UID 44 of its INBOX.
fn synced_mailbox() -> (Mailbox, Id, Id) {
    let (mut mb, account) = mailbox();
    for (kind, remote) in [(FolderKind::Inbox, "INBOX"), (FolderKind::Archive, "Archive"), (FolderKind::Deleted, "Trash")] {
        let id = mb.folder_of_kind(account, kind).unwrap();
        let f = mb.folder_mut(id).unwrap();
        (f.remote, f.uid_validity) = (remote.into(), 7);
    }
    let inbox = mb.inbox().unwrap();
    let a = mb.add_message(Message { uid: 44, synced: Some(unread(44)), ..mail(inbox, "One", 1) }).unwrap();
    (mb, account, a)
}

#[test]
fn moving_mail_the_server_has_waits_to_be_pushed() {
    let (mut mb, account, a) = synced_mailbox();
    let inbox = mb.inbox().unwrap();
    let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
    let local = mb.add_folder(account, None, "Only here").unwrap();
    let moved = |to: Id| Pending::Move { account, message: a, from: "INBOX".into(), uid: 44, synced: Some(unread(44)), to };

    mb.move_message(a, archive).unwrap();
    assert_eq!(mb.pending, [moved(archive)]);
    assert_eq!(mb.message(a).map(|m| (m.folder, m.uid, m.synced)), Some((archive, 0, None)));
    // Moved on before the server heard: one move, from where the server has it to where it is now.
    mb.message_mut(a).unwrap().read = true;
    mb.move_message(a, local).unwrap();
    assert_eq!(mb.pending, [moved(local)]);
    // To where it already is: nothing.
    mb.move_message(a, local).unwrap();
    assert_eq!(mb.pending, [moved(local)]);
    // Back where the server has it: the move is called off, and the message is what it was to the
    // server, with what was changed about it meanwhile still to be pushed.
    mb.move_message(a, inbox).unwrap();
    assert!(mb.pending.is_empty());
    assert_eq!(mb.message(a).map(|m| (m.folder, m.uid, m.synced, m.read)), Some((inbox, 44, Some(unread(44)), true)));

    // A copy is not on the server, and neither is what was never there: moving those says nothing.
    let copy = mb.copy_message(a, archive).unwrap();
    assert_eq!(mb.message(copy).map(|m| (m.uid, m.synced)), Some((0, None)));
    mb.move_message(copy, inbox).unwrap();
    // Nor is a message out of a folder the server doesn't have.
    let stray = mb.add_message(Message { uid: 9, ..mail(local, "Stray", 2) }).unwrap();
    mb.move_message(stray, inbox).unwrap();
    assert!(mb.pending.is_empty());
    assert_eq!(mb.message(stray).map(|m| m.uid), Some(0));
    assert!(mb.repair().is_empty());
}

#[test]
fn deleting_mail_the_server_has_waits_to_be_pushed() {
    let (mut mb, account, a) = synced_mailbox();
    let inbox = mb.inbox().unwrap();
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    let b = mb.add_message(Message { uid: 45, synced: Some(unread(45)), ..mail(inbox, "Two", 2) }).unwrap();
    let c = mb.add_message(Message { uid: 46, ..mail(inbox, "Three", 3) }).unwrap();
    let local = mb.add_message(mail(inbox, "Never sent anywhere", 4)).unwrap();

    // Into Deleted Items: a move like any other.
    assert_eq!(mb.delete_message(a), Ok(false));
    assert_eq!(mb.pending, [Pending::Move { account, message: a, from: "INBOX".into(), uid: 44, synced: Some(unread(44)), to: deleted }]);
    // And then for good, before the server heard of the move: deleted where the server has it.
    assert_eq!(mb.delete_message(a), Ok(true));
    assert_eq!(mb.pending, [Pending::Delete { account, from: "INBOX".into(), uid: 44, message_id: String::new(), seen: None }]);
    // For good straight away.
    mb.remove_message(b).unwrap();
    assert_eq!(mb.pending.last(), Some(&Pending::Delete { account, from: "INBOX".into(), uid: 45, message_id: String::new(), seen: None }));
    // What a synchronisation takes out because the server no longer has it is not told back.
    mb.forget_message(c).unwrap();
    // A message that was never on the server is nobody's business.
    mb.remove_message(local).unwrap();
    assert_eq!(mb.pending.len(), 2);
    assert!(mb.remove_message(a).is_err() && mb.forget_message(c).is_err());
    assert_eq!(mb.count(inbox), 0);
    assert!(mb.repair().is_empty());

    // A task keeps its to-do when the message goes either way.
    let d = mb.add_message(Message { uid: 47, ..mail(inbox, "Four", 5) }).unwrap();
    let t = mb.add_task(Task { title: "Follow up".into(), message: Some(d), ..Default::default() });
    mb.forget_message(d).unwrap();
    assert_eq!(mb.task(t).map(|x| x.message), Some(None));
}

#[test]
fn mail_moved_to_another_imap_account_waits_to_be_put_on_its_server_and_its_original_with_it() {
    let (mut mb, account, a) = synced_mailbox();
    mb.accounts[0].kind = AccountKind::Imap;
    let (inbox, archive) = (mb.inbox().unwrap(), mb.folder_of_kind(account, FolderKind::Archive).unwrap());
    let other = mb.add_account(Account { email: "work@example.org".into(), kind: AccountKind::Imap, ..Default::default() });
    let other_inbox = mb.folder_of_kind(other, FolderKind::Inbox).unwrap();
    mb.folder_mut(other_inbox).unwrap().remote = "INBOX".into();
    // Put on the other server by its own run; the original stays on its own until it is there
    // (#451). The other account's INBOX is not the one it came from.
    mb.move_message(a, other_inbox).unwrap();
    assert_eq!(mb.message(a).map(|m| (m.account, m.uid, m.synced)), Some((other, 0, None)));
    let moved_out = |to: Id| Pending::Move { account, message: a, from: "INBOX".into(), uid: 44, synced: Some(unread(44)), to };
    assert_eq!(mb.pending, [moved_out(other_inbox)]);
    assert!(
        matches!(mb.queued.as_slice(), [Queued::PlaceMessage { account: to, message, copy: false, from: None, .. }] if *to == other && *message == a)
    );
    // A move within its own account that was still waiting goes on into the other account.
    let b = mb.add_message(Message { uid: 45, synced: Some(unread(45)), ..mail(inbox, "Two", 2) }).unwrap();
    mb.move_message(b, archive).unwrap();
    mb.move_message(b, other_inbox).unwrap();
    assert_eq!(mb.pending.len(), 2);
    assert_eq!(mb.message(b).map(|m| (m.account, m.uid)), Some((other, 0)));
    assert!(mb.repair().is_empty());
    // On to this computer before the other server has it: nothing is put there, and the
    // original goes once it is safe here.
    let local = mb.local_account();
    let kept = mb.add_folder(local, None, "Kept").unwrap();
    mb.move_message(b, kept).unwrap();
    assert!(!mb.queued.iter().any(|q| matches!(q, Queued::PlaceMessage { message, .. } if *message == b)));
    assert!(mb.pending.iter().any(|p| matches!(p, Pending::Move { account: owner, message, .. } if *owner == account && *message == b)));
    // Back where its server has it: nothing is put anywhere, and nothing goes.
    mb.move_message(a, inbox).unwrap();
    assert_eq!(mb.message(a).map(|m| (m.account, m.uid, m.synced)), Some((account, 44, Some(unread(44)))));
    assert!(!mb.pending.iter().any(|p| matches!(p, Pending::Move { message, .. } if *message == a)));
    assert!(!mb.queued.iter().any(|q| matches!(q, Queued::PlaceMessage { message, .. } if *message == a)));
    assert!(mb.repair().is_empty());
}

#[test]
fn removing_a_folder_or_an_account_drops_what_waited_for_it() {
    let (mut mb, account, a) = synced_mailbox();
    let inbox = mb.inbox().unwrap();
    let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
    let projects = mb.add_folder(account, None, "Projects").unwrap();
    let old = mb.add_folder(account, None, "Old").unwrap();
    let f = mb.folder_mut(old).unwrap();
    (f.remote, f.uid_validity) = ("Old".into(), 3);
    let b = mb.add_message(Message { uid: 5, ..mail(old, "From the old folder", 2) }).unwrap();
    let c = mb.add_message(Message { uid: 6, ..mail(old, "Deleted there", 3) }).unwrap();
    let d = mb.add_message(Message { uid: 45, ..mail(inbox, "Stays", 4) }).unwrap();
    let first = mb.message(a).map(|m| m.uid).unwrap();
    mb.move_message(a, projects).unwrap();
    mb.move_message(b, archive).unwrap();
    mb.remove_message(c).unwrap();
    mb.move_message(d, archive).unwrap();
    assert_eq!(mb.pending.len(), 4);
    // Into Deleted Items, the folder is still a folder; the second delete removes it for good.
    assert_eq!(mb.delete_folder(projects), Ok(false));
    assert_eq!(mb.pending.len(), 4);
    assert_eq!(mb.delete_folder(projects), Ok(true));
    assert_eq!(mb.pending.len(), 4);
    assert_eq!((mb.delete_folder(old), mb.delete_folder(old)), (Ok(false), Ok(true)));
    // What was to leave Old or be deleted there is forgotten: the server's folder goes with it.
    // What was to go into Projects goes for good with it, deleted where the server has it, as a
    // message deleted for good is (#451); the rest waits on.
    assert!(
        matches!(&mb.pending[..], [Pending::Delete { from, uid, .. }, Pending::Move { message, .. }] if from == "INBOX" && *uid == first && *message == d),
        "{:?}",
        mb.pending
    );
    assert!(mb.message(a).is_none() && mb.message(b).is_some());
    assert!(mb.repair().is_empty());

    let second = mb.add_account(Account { email: "work@example.org".into(), ..Default::default() });
    mb.remove_account(account).unwrap();
    assert!(mb.pending.is_empty() && mb.account(second).is_some());
}

#[test]
fn what_waits_for_the_server_round_trips_and_is_left_out_when_there_is_none() {
    let (mut mb, account, a) = synced_mailbox();
    let plain = serde_json::to_string(&Mailbox::with_local_account("Ada", "ada@example.org")).unwrap();
    assert!(!plain.contains("pending") && !plain.contains("synced"), "{plain}");
    let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
    let inbox = mb.inbox().unwrap();
    let b = mb.add_message(Message { uid: 45, synced: Some(unread(45)), ..mail(inbox, "Two", 2) }).unwrap();
    mb.move_message(a, archive).unwrap();
    mb.remove_message(b).unwrap();
    // A message that is on the server without its UID being known yet.
    mb.add_message(Message { synced: Some(sync::RemoteFlags { read: true, ..Default::default() }), ..mail(archive, "Expected", 3) }).unwrap();
    let json = serde_json::to_string(&mb).unwrap();
    assert!(json.contains(r#""pending":[{"move":{"account":"#) && json.contains(r#"{"delete":{"account":"#), "{json}");
    let mut back: Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(back, mb);
    assert!(back.repair().is_empty());
    assert_eq!(back, mb);
}

#[test]
fn repair_drops_nonsense_about_the_server() {
    // Things that aren't changes at all are passed over when the file is read; the rest is
    // weighed by `repair`.
    let json = r#"{
        "accounts": [{"id": 1, "name": "A"}, {"id": 2, "name": "B"}],
        "folders": [
            {"id": 5, "account": 1, "name": "Inbox", "kind": "inbox", "remote": "INBOX", "uidValidity": 7},
            {"id": 6, "account": 1, "name": "Archive", "kind": "archive", "remote": "Archive", "uidValidity": 8},
            {"id": 7, "account": 2, "name": "Inbox", "kind": "inbox", "remote": "INBOX"}
        ],
        "messages": {
            "10": {"id": 10, "account": 1, "folder": 5, "uid": 3, "synced": {"uid": 3, "read": true}},
            "11": {"id": 11, "account": 1, "folder": 5, "uid": 3, "synced": {"uid": 3}},
            "12": {"id": 12, "account": 1, "folder": 5, "uid": 4, "synced": {"uid": 99, "read": true}},
            "13": {"id": 13, "account": 1, "folder": 5, "uid": 5, "synced": "yes"},
            "14": {"id": 14, "account": 1, "folder": 5, "uid": 6, "synced": {"uid": "six"}},
            "15": {"id": 15, "account": 1, "folder": 6, "synced": {"uid": 0, "read": true}},
            "16": {"id": 16, "account": 1, "folder": 6},
            "17": {"id": 17, "account": 1, "folder": 6, "synced": null},
            "18": {"id": 18, "account": 1, "folder": 6, "uid": 2}
        },
        "pending": [
            {"move": {"account": 1, "message": 16, "from": "INBOX", "uid": 40, "synced": {"uid": 41}, "to": 6}},
            5, "delete everything", null, [], {"zap": {"account": 1}}, {"move": {"account": "one"}}, {"delete": {}},
            {"move": {"account": 1, "message": 16, "from": "INBOX", "uid": 42, "to": 6}},
            {"move": {"account": 1, "message": 17, "from": "INBOX", "uid": 40, "to": 6}},
            {"move": {"account": 1, "message": 4242, "from": "INBOX", "uid": 43, "to": 6}},
            {"move": {"account": 1, "message": 17, "from": "INBOX", "uid": 44, "to": 4242}},
            {"move": {"account": 1, "message": 18, "from": "INBOX", "uid": 45, "to": 6}},
            {"move": {"account": 1, "message": 17, "from": "Archive", "uid": 46, "to": 6}},
            {"move": {"account": 2, "message": 17, "from": "INBOX", "uid": 47, "to": 6}},
            {"move": {"account": 1, "message": 17, "from": "Nowhere", "uid": 48, "to": 6}},
            {"move": {"account": 1, "message": 17, "from": "INBOX", "uid": 0, "to": 6}},
            {"delete": {"account": 1, "from": "INBOX", "uid": 3}},
            {"delete": {"account": 1, "from": "INBOX", "uid": 50}},
            {"delete": {"account": 1, "from": "INBOX", "uid": 50}},
            {"delete": {"account": 1, "from": "", "uid": 51}},
            {"delete": {"account": 77, "from": "INBOX", "uid": 52}},
            {"delete": {"account": 2, "from": "INBOX", "uid": 53}},
            {"delete": {"account": 1, "from": "INBOX", "uid": 0}}
        ]
    }"#;
    let mut mb: Mailbox = serde_json::from_str(json).unwrap();
    assert_eq!(mb.pending.len(), 17);
    let fixed = mb.repair();
    assert!(fixed.iter().any(|line| line.contains("untied")) && fixed.iter().any(|line| line.contains("waiting for the mail server")), "{fixed:?}");
    // A UID names one message of its folder, and the flags kept are those of that UID.
    let kept = |id: Id| mb.message(id).map(|m| (m.uid, m.synced.map(|s| (s.uid, s.read))));
    assert_eq!(kept(10), Some((3, Some((3, true)))));
    assert_eq!(kept(11), Some((0, None)));
    assert_eq!(kept(12), Some((4, None)));
    assert_eq!(kept(13), Some((5, None)));
    assert_eq!(kept(14), Some((6, None)));
    // On the server without a UID yet: that is a state of its own.
    assert_eq!(kept(15), Some((0, Some((0, true)))));
    // What is left could have been noted here: a move of a message that is where the move says,
    // without a UID, out of a mailbox its account follows; deletions of what no message here is.
    assert_eq!(
        mb.pending,
        [
            Pending::Move { account: 1, message: 16, from: "INBOX".into(), uid: 40, synced: None, to: 6 },
            Pending::Delete { account: 1, from: "INBOX".into(), uid: 50, message_id: String::new(), seen: None },
            Pending::Delete { account: 2, from: "INBOX".into(), uid: 53, message_id: String::new(), seen: None },
        ]
    );
    assert!(mb.repair().is_empty(), "{:?}", mb.clone().repair());

    // A list without end is cut.
    let many: Vec<String> = (1..=100_100).map(|uid| format!(r#"{{"delete": {{"account": 1, "from": "INBOX", "uid": {uid}}}}}"#)).collect();
    let json = format!(
        r#"{{"accounts": [{{"id": 1}}], "folders": [{{"id": 5, "account": 1, "name": "Inbox", "kind": "inbox", "remote": "INBOX"}}], "pending": [{}]}}"#,
        many.join(",")
    );
    let mut mb: Mailbox = serde_json::from_str(&json).unwrap();
    mb.repair();
    assert_eq!(mb.pending.len(), 100_000);
    // And something that isn't a list at all is no list.
    let mb: Mailbox = serde_json::from_str(r#"{"pending": {"move": 1}}"#).unwrap();
    assert!(mb.pending.is_empty());
}

#[test]
fn removing_an_account_takes_its_mail() {
    let (mut mb, account) = mailbox();
    let second = mb.add_account(Account { email: "work@example.org".into(), ..Default::default() });
    let inbox2 = mb.folder_of_kind(second, FolderKind::Inbox).unwrap();
    let m = mb.add_message(mail(inbox2, "Work", 1)).unwrap();
    assert_eq!(mb.message(m).map(|x| x.account), Some(second));
    assert_eq!(mb.calendars.len(), 1);
    mb.remove_account(second).unwrap();
    assert!(mb.message(m).is_none() && mb.folder(inbox2).is_none());
    assert!(mb.remove_account(second).is_err());
    assert!(mb.account(account).is_some());
}

#[test]
fn a_change_waiting_for_a_mailbox_numbered_anew_is_kept_when_it_can_be_found_again() {
    let (mut mb, account) = mailbox();
    let inbox = mb.inbox().unwrap();
    mb.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let seen = MailIdentity { date: 1_700_000_000, from: "a@mail.example".into(), subject: "Hi".into(), people: vec!["b@mail.example".into()] };
    let waiting = Pending::Delete { account, from: "INBOX".into(), uid: 0, message_id: "gone@mail.example".into(), seen: Some(seen) };
    let saved = serde_json::to_value(&waiting).unwrap();
    assert_eq!(saved["delete"]["messageId"], "gone@mail.example");
    assert_eq!(serde_json::from_value::<Pending>(saved).unwrap(), waiting);
    // Without a Message-ID it could never be found again: the repair lets it go.
    mb.pending = vec![waiting.clone(), Pending::Delete { account, from: "INBOX".into(), uid: 0, message_id: String::new(), seen: None }];
    let fixed = mb.repair();
    assert!(fixed.iter().any(|line| line.contains("waiting for the mail server")), "{fixed:?}");
    assert_eq!(mb.pending, [waiting]);
}

#[test]
fn a_change_of_0_8_0_waiting_for_a_renumbered_mailbox_is_let_go_and_the_file_still_opens() {
    let (mut mb, account) = mailbox();
    let inbox = mb.inbox().unwrap();
    mb.folder_mut(inbox).unwrap().remote = "INBOX".into();
    // A mailbox file of 0.8.0 has no `seen` in the change.
    let old: Pending =
        serde_json::from_value(serde_json::json!({"delete": {"account": account, "from": "INBOX", "uid": 0, "messageId": "x@mail.example"}}))
            .unwrap();
    assert!(matches!(&old, Pending::Delete { seen: None, .. }));
    mb.pending = vec![old];
    mb.repair();
    assert!(mb.pending.is_empty());
}

// ---- shared mailboxes (#70) ----

/// A mailbox with a local account and a Microsoft account of an organisation.
fn with_microsoft() -> (Mailbox, Id, Id) {
    let (mut mb, local) = mailbox();
    let oauth = OAuth { provider: OAuthProvider::Microsoft, directory: Directory::Organisation, ..Default::default() };
    let owner = mb.add_account(Account {
        email: "noor@example.org".into(),
        kind: AccountKind::Microsoft,
        oauth: Some(oauth),
        download_in_full: DownloadInFull::UpTo25Mb,
        ..Default::default()
    });
    (mb, local, owner)
}

#[test]
fn a_shared_mailbox_opens_under_a_microsoft_account_with_folders_of_its_own() {
    let (mut mb, local, owner) = with_microsoft();
    let shared = mb.add_shared_mailbox(owner, " support@example.org ").unwrap();
    let a = mb.account(shared).unwrap();
    assert_eq!((a.email.as_str(), a.kind, a.shared_of, a.oauth.is_none()), ("support@example.org", AccountKind::Microsoft, Some(owner), true));
    assert_eq!(a.download_in_full, DownloadInFull::UpTo25Mb, "as its owner has it");
    assert!(a.is_shared() && !mb.account(owner).unwrap().is_shared());
    assert_eq!(mb.owner_of(shared).map(|o| o.id), Some(owner));
    assert_eq!((mb.owner_of(owner), mb.owner_of(local)), (None, None));
    // Its own standard folders, none of them among the Favorites.
    let names: Vec<&str> = mb.children(shared, None).iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["Inbox", "Drafts", "Sent Items", "Deleted Items", "Archive", "Junk Email", "Outbox"]);
    assert!(mb.folders.iter().filter(|f| f.account == shared).all(|f| !f.favorite));
    // Listed under its owner.
    let b = mb.add_account(Account { email: "bob@example.org".into(), ..Default::default() });
    let order: Vec<Id> = mb.accounts_in_order().iter().map(|a| a.id).collect();
    assert_eq!(order, [local, owner, shared, b]);
    assert_eq!(mb.shared_mailboxes(owner).iter().map(|a| a.id).collect::<Vec<_>>(), [shared]);
}

#[test]
fn a_shared_mailbox_is_refused_where_it_cant_be_one() {
    let (mut mb, local, owner) = with_microsoft();
    let refused = |mb: &mut Mailbox, under: Id, address: &str| mb.add_shared_mailbox(under, address).unwrap_err().to_string();
    assert!(refused(&mut mb, local, "support@example.org").contains("Microsoft Graph"));
    assert!(refused(&mut mb, 999, "support@example.org").contains("no account"));
    assert!(refused(&mut mb, owner, "noor@EXAMPLE.org").contains("own address"));
    assert!(refused(&mut mb, owner, "ada@example.org").contains("already here"));
    for bad in ["", "support", "support@example.org/../me", "x@example.org?$top=1", "a b@example.org", "\u{0}@example.org"] {
        assert!(refused(&mut mb, owner, bad).contains("e-mail address"), "{bad:?}");
    }
    let shared = mb.add_shared_mailbox(owner, "support@example.org").unwrap();
    assert!(refused(&mut mb, owner, "SUPPORT@example.org").contains("already here"));
    assert!(refused(&mut mb, shared, "sales@example.org").contains("can't open another"));
    for n in 1..MAX_SHARED {
        mb.add_shared_mailbox(owner, &format!("team{n}@example.org")).unwrap();
    }
    assert!(refused(&mut mb, owner, "one-more@example.org").contains("at most"));
    // A personal Microsoft account has none.
    if let Ok(a) = mb.account_mut(owner) {
        a.oauth = Some(OAuth { provider: OAuthProvider::Microsoft, directory: Directory::Personal, ..Default::default() });
    }
    assert!(refused(&mut mb, owner, "other@example.org").contains("personal"));
}

#[test]
fn removing_an_account_takes_its_shared_mailboxes_and_their_mail() {
    let (mut mb, _, owner) = with_microsoft();
    let shared = mb.add_shared_mailbox(owner, "support@example.org").unwrap();
    let inbox = mb.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    mb.add_message(mail(inbox, "For the team", 1)).unwrap();
    // The shared mailbox alone goes by itself.
    let other = mb.add_shared_mailbox(owner, "sales@example.org").unwrap();
    mb.remove_account(other).unwrap();
    assert!(mb.account(other).is_none() && mb.account(shared).is_some());
    mb.remove_account(owner).unwrap();
    assert!(mb.account(shared).is_none());
    assert!(mb.folders.iter().all(|f| f.account != shared) && mb.messages.values().all(|m| m.account != shared));
}

#[test]
fn a_file_from_before_shared_mailboxes_opens_and_one_with_nonsense_too() {
    let old: Account = serde_json::from_str(r#"{"id": 3, "email": "noor@example.org", "kind": "microsoft"}"#).unwrap();
    assert_eq!(old.shared_of, None);
    assert!(!serde_json::to_string(&old).unwrap().contains("sharedOf"));
    let odd: Account = serde_json::from_str(r#"{"id": 3, "email": "support@example.org", "sharedOf": "nobody"}"#).unwrap();
    assert_eq!(odd.shared_of, None);
    let (mut mb, _, owner) = with_microsoft();
    let shared = mb.add_shared_mailbox(owner, "support@example.org").unwrap();
    let again: Mailbox = serde_json::from_str(&serde_json::to_string(&mb).unwrap()).unwrap();
    assert_eq!(again.account(shared).and_then(|a| a.shared_of), Some(owner));
    // A shared mailbox that names itself, or an owner that isn't one, has no owner to read it.
    if let Ok(a) = mb.account_mut(shared) {
        a.shared_of = Some(shared);
    }
    assert!(mb.owner_of(shared).is_none());
    assert_eq!(mb.accounts_in_order().len(), mb.accounts.len());
}

// ---- mail to keep offline (#143) ----

#[test]
fn mail_to_keep_offline_is_all_for_a_file_without_it_and_round_trips() {
    let old: Account = serde_json::from_str(r#"{"id": 3, "email": "noor@example.org", "kind": "microsoft"}"#).unwrap();
    assert_eq!(old.keep_offline, KeepOffline::All, "nothing is removed without the owner's choosing");
    assert!(!serde_json::to_string(&old).unwrap().contains("keepOffline"));
    let odd: Account = serde_json::from_str(r#"{"id": 3, "email": "noor@example.org", "keepOffline": "forever"}"#).unwrap();
    assert_eq!(odd.keep_offline, KeepOffline::All);
    let short = Account { keep_offline: KeepOffline::Months3, ..old };
    let json = serde_json::to_string(&short).unwrap();
    assert!(json.contains(r#""keepOffline":"3m""#), "{json}");
    assert_eq!(serde_json::from_str::<Account>(&json).unwrap().keep_offline, KeepOffline::Months3);
}

#[test]
fn a_shared_mailbox_keeps_as_much_offline_as_its_owner() {
    let (mut mb, local, owner) = with_microsoft();
    mb.account_mut(owner).unwrap().keep_offline = KeepOffline::Weeks2;
    let shared = mb.add_shared_mailbox(owner, "support@example.org").unwrap();
    assert_eq!(mb.keep_offline_of(shared), KeepOffline::Weeks2);
    mb.account_mut(owner).unwrap().keep_offline = KeepOffline::Years5;
    assert_eq!(mb.keep_offline_of(shared), KeepOffline::Years5, "it follows its owner's as that changes");
    assert_eq!((mb.keep_offline_of(local), mb.keep_offline_of(999_999)), (KeepOffline::All, KeepOffline::All));
}

// ---- an account a newer MailCraft added ----

/// What a newer MailCraft would write: a local account, then an account of a kind this version
/// doesn't know (`fromANewerVersion`, which no version has) with settings of its own, one of them in a shape this version would refuse
/// for an account it knows, and an IMAP account; the newer account's Inbox has a message. The file
/// and the newer account's id.
fn written_by_a_newer_version() -> (serde_json::Value, Id) {
    let (mut mb, _) = mailbox();
    let newer = mb.add_account(Account { email: "ada@jmap.example".into(), ..Default::default() });
    let imap = Server { host: "imap.example.org".into(), port: 993, security: Security::Tls };
    mb.add_account(Account { email: "ada@imap.example".into(), kind: AccountKind::Imap, incoming: Some(imap), ..Default::default() });
    let inbox = mb.folder_of_kind(newer, FolderKind::Inbox).unwrap();
    mb.add_message(mail(inbox, "Written in the newer version", 10)).unwrap();
    let mut file = serde_json::to_value(&mb).unwrap();
    let account = file["accounts"].as_array_mut().unwrap().iter_mut().find(|a| a["id"] == newer).unwrap();
    *account = serde_json::json!({
        "id": newer,
        "name": "Ada at home",
        "email": "ada@jmap.example",
        "displayName": "Ada",
        "kind": "fromANewerVersion",
        "settings": {"session": "https://jmap.example/.well-known/jmap", "auth": "bearer"},
        "incoming": "a newer version's own way of naming a server",
        "oauth": {"provider": "microsoft", "tenant": "organizations"},
        "username": "ada",
        "signature": "Home",
        "keepOffline": "6m",
        "comesLater": [1, 2.5, {"deep": null, "flag": true}],
    });
    (file, newer)
}

#[test]
fn an_account_of_a_kind_this_version_doesnt_know_is_kept_as_its_file_has_it() {
    let (file, newer) = written_by_a_newer_version();
    let mb: Mailbox = serde_json::from_value(file.clone()).unwrap();
    assert_eq!(mb.accounts.len(), 3);
    let a = mb.account(newer).unwrap();
    // Read for what says which account it is: never its servers or its sign-in, which nothing
    // here could use for it (an `incoming` this version can't read doesn't stop the file opening).
    assert_eq!(
        (a.kind, a.name.as_str(), a.email.as_str(), a.display_name.as_str()),
        (AccountKind::Unknown, "Ada at home", "ada@jmap.example", "Ada")
    );
    assert!(!a.kind.has_server());
    assert!(a.incoming.is_none() && a.outgoing.is_none() && a.oauth.is_none() && a.username.is_empty() && a.signature.is_empty());
    assert_eq!(a.keep_offline, KeepOffline::All);
    // Its folders and mail are read as always.
    assert_eq!(mb.count(mb.folder_of_kind(newer, FolderKind::Inbox).unwrap()), 1);
    // Written back, it is as it came, value for value; and so is everything else.
    assert_eq!(serde_json::to_value(&mb).unwrap(), file);
    let again: Mailbox = serde_json::from_str(&serde_json::to_string(&mb).unwrap()).unwrap();
    assert_eq!(again, mb);
    // Its debug output says how much is kept, never what.
    let debug = format!("{a:?}");
    assert!(debug.contains("as_written: Some(AsWritten { fields: 11 })"), "{debug}");
    assert!(!debug.contains(".well-known") && !debug.contains("bearer") && !debug.contains("organizations"), "{debug}");
}

#[test]
fn it_goes_back_with_the_id_the_mailbox_gives_it() {
    // A file whose newer account has no id: the repair gives it one, and that is the id written
    // back, so the next time it is the same account with the same folders.
    let (mut file, newer) = written_by_a_newer_version();
    let accounts = file["accounts"].as_array_mut().unwrap();
    let written = accounts.iter_mut().find(|a| a["id"] == newer).unwrap();
    written.as_object_mut().unwrap().remove("id");
    let mut mb: Mailbox = serde_json::from_value(file).unwrap();
    assert!(mb.repair().iter().any(|line| line == "an account id corrected"));
    let id = mb.accounts.iter().find(|a| a.kind == AccountKind::Unknown).unwrap().id;
    assert_ne!(id, 0);
    let saved = serde_json::to_value(&mb).unwrap();
    let back = saved["accounts"].as_array().unwrap().iter().find(|a| a["kind"] == "fromANewerVersion").unwrap();
    assert_eq!((back["id"].as_u64(), &back["settings"]["auth"]), (Some(id), &serde_json::json!("bearer")));
    let mut again: Mailbox = serde_json::from_value(saved).unwrap();
    assert!(again.repair().is_empty());
    assert_eq!(again, mb);
}

#[test]
fn a_kind_that_is_any_other_word_or_cant_be_read_is_one_this_version_doesnt_know() {
    let kind_of = |kind: serde_json::Value| {
        let mb: Mailbox = serde_json::from_value(serde_json::json!({"accounts": [{"id": 4, "email": "ada@example.org", "kind": kind}]})).unwrap();
        mb.accounts[0].kind
    };
    for kind in
        [serde_json::json!("exchange"), serde_json::json!("IMAP"), serde_json::json!(7), serde_json::json!({"newer": {}}), serde_json::Value::Null]
    {
        assert_eq!(kind_of(kind.clone()), AccountKind::Unknown, "{kind}");
    }
    for (word, kind) in [("local", AccountKind::Local), ("imap", AccountKind::Imap), ("microsoft", AccountKind::Microsoft)] {
        assert_eq!(kind_of(serde_json::json!(word)), kind);
    }
    // A file from before there were kinds says none: a local account, as always.
    let old: Mailbox = serde_json::from_str(r#"{"accounts": [{"id": 4, "email": "ada@example.org"}]}"#).unwrap();
    assert_eq!((old.accounts[0].kind, old.accounts[0].as_written.is_none()), (AccountKind::Local, true));
    // An account of a kind this version knows is read as strictly as ever.
    assert!(serde_json::from_str::<Mailbox>(r#"{"accounts": [{"id": 4, "kind": "imap", "incoming": "imap.example.org"}]}"#).is_err());
}

// ---- a file from before the JMAP setting ----

/// What a file held before an account could be a JMAP account (`jmap`) and before the server's
/// answered and forwarded marks were kept (`ServerMessage::answered`, `forwarded`): it reads and
/// writes back, byte for byte, as it did. The texts are what the version before wrote.
#[test]
fn a_file_from_before_the_jmap_setting_reads_and_writes_as_it_was() {
    use crate::sync::ServerMessage;
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use std::fmt::Debug;

    /// `value` is written as `written`, and `written` reads as `value` and is written the same.
    fn as_it_was<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T, written: &str) {
        assert_eq!(serde_json::to_string(value).unwrap(), written);
        let read: T = serde_json::from_str(written).unwrap();
        assert_eq!(&read, value);
        assert_eq!(serde_json::to_string(&read).unwrap(), written);
    }

    let oauth =
        OAuth { provider: OAuthProvider::Microsoft, client_id: "app-id".into(), tenant: "organizations".into(), directory: Directory::Organisation };
    let microsoft = Account {
        id: 2,
        name: "Noor".into(),
        email: "noor@example.org".into(),
        display_name: "Noor Haddad".into(),
        kind: AccountKind::Microsoft,
        oauth: Some(oauth),
        download_in_full: DownloadInFull::UpTo25Mb,
        keep_offline: KeepOffline::Months3,
        ..Default::default()
    };
    as_it_was(
        &microsoft,
        r#"{"id":2,"name":"Noor","email":"noor@example.org","displayName":"Noor Haddad","kind":"microsoft","oauth":{"provider":"microsoft","clientId":"app-id","tenant":"organizations","directory":"organisation"},"downloadInFull":"upTo25Mb","keepOffline":"3m"}"#,
    );
    let imap = Account {
        id: 3,
        name: "ada@example.org".into(),
        email: "ada@example.org".into(),
        display_name: "Ada".into(),
        kind: AccountKind::Imap,
        incoming: Some(Server { host: "imap.example.org".into(), port: 993, security: Security::Tls }),
        outgoing: Some(Server { host: "smtp.example.org".into(), port: 587, security: Security::StartTls }),
        username: "ada".into(),
        ..Default::default()
    };
    as_it_was(
        &imap,
        r#"{"id":3,"name":"ada@example.org","email":"ada@example.org","displayName":"Ada","kind":"imap","incoming":{"host":"imap.example.org","port":993,"security":"tls"},"outgoing":{"host":"smtp.example.org","port":587,"security":"startTls"},"username":"ada"}"#,
    );
    let server = ServerMessage {
        id: "AAkA".into(),
        folder: "F1".into(),
        read: true,
        flag: FlagStatus::Flagged,
        categories: vec!["Red".into()],
        importance: Importance::High,
        gone: true,
        size: 2048,
        change_key: "CK1".into(),
        ..Default::default()
    };
    let message = Message { id: 4, account: 2, folder: 9, subject: "Hi".into(), server: Some(server), ..Default::default() };
    as_it_was(
        &message,
        r#"{"id":4,"account":2,"folder":9,"messageId":"","from":{"email":""},"to":[],"subject":"Hi","date":0,"received":0,"body":{"text":""},"read":false,"flag":"none","importance":"normal","answered":false,"forwarded":false,"draft":false,"pinned":false,"server":{"id":"AAkA","folder":"F1","read":true,"flag":"flagged","categories":["Red"],"importance":"high","gone":true,"size":2048,"changeKey":"CK1"}}"#,
    );
    let plain = Message {
        id: 5,
        account: 2,
        folder: 9,
        server: Some(ServerMessage { id: "AAkB".into(), folder: "F1".into(), ..Default::default() }),
        ..Default::default()
    };
    as_it_was(
        &plain,
        r#"{"id":5,"account":2,"folder":9,"messageId":"","from":{"email":""},"to":[],"subject":"","date":0,"received":0,"body":{"text":""},"read":false,"flag":"none","importance":"normal","answered":false,"forwarded":false,"draft":false,"pinned":false,"server":{"id":"AAkB","folder":"F1","read":false,"flag":"none","importance":"normal"}}"#,
    );
    let folder = Folder {
        id: 9,
        account: 2,
        name: "Inbox".into(),
        kind: FolderKind::Inbox,
        server: Some(ServerFolder { id: "F1".into(), name: "Inbox".into(), ..Default::default() }),
        ..Default::default()
    };
    as_it_was(&folder, r#"{"id":9,"account":2,"name":"Inbox","kind":"inbox","favorite":false,"server":{"id":"F1","name":"Inbox"}}"#);
    // And whole mailboxes: no account says a word of it, and the kind that is new is only read.
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    mb.add_account(microsoft);
    mb.add_account(imap);
    let json = serde_json::to_string(&mb).unwrap();
    assert!(!json.contains("jmap") && !json.contains("answered\":true") && !json.contains("forwarded\":true"), "{json}");
    assert_eq!(serde_json::from_str::<Mailbox>(&json).unwrap(), mb);
}

#[test]
fn a_jmap_account_keeps_its_setting_and_a_damaged_one_costs_only_itself() {
    let session = "https://mail.example.org/jmap/session";
    let account = Account {
        id: 4,
        name: "Ada".into(),
        email: "ada@example.org".into(),
        kind: AccountKind::Jmap,
        jmap: Some(JmapServer { session: session.into(), bearer: true }),
        ..Default::default()
    };
    let json = serde_json::to_string(&account).unwrap();
    assert_eq!(
        json,
        format!(r#"{{"id":4,"name":"Ada","email":"ada@example.org","displayName":"","kind":"jmap","jmap":{{"session":"{session}","bearer":true}}}}"#)
    );
    assert_eq!(serde_json::from_str::<Account>(&json).unwrap(), account);
    // The kind is spelled the way the others are, and an account of another kind has no setting.
    assert_eq!(serde_json::to_value(AccountKind::Jmap).unwrap(), "jmap");
    assert!(!serde_json::to_string(&Account::default()).unwrap().contains("jmap"));
    // Something else where the setting should be (a later version's, a damaged file): the account
    // and the mailbox still open, without a server that could be used.
    for odd in ["5", "\"https://mail.example.org\"", "[]", "null", r#"{"session":5}"#, r#"{"bearer":"yes"}"#] {
        let read: Account = serde_json::from_str(&format!(r#"{{"id":4,"email":"ada@example.org","kind":"jmap","jmap":{odd}}}"#)).unwrap();
        assert_eq!(read.kind, AccountKind::Jmap, "{odd}");
        assert!(read.jmap.as_ref().is_none_or(|server| server.problem().is_some()), "{odd}: {:?}", read.jmap);
    }
    let bare: Account = serde_json::from_str(r#"{"id":4,"email":"ada@example.org","kind":"jmap","jmap":{}}"#).unwrap();
    assert_eq!(bare.jmap, Some(JmapServer::default()));
    assert_eq!(bare.jmap.as_ref().and_then(JmapServer::problem), Some("is empty"));
    // A mailbox that holds one is read whole, and repair leaves the account as it is.
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let id = mb.add_account(account.clone());
    let mut back: Mailbox = serde_json::from_str(&serde_json::to_string(&mb).unwrap()).unwrap();
    assert!(back.repair().is_empty());
    assert_eq!(back.account(id).map(|a| (a.kind, a.jmap.clone())), Some((AccountKind::Jmap, account.jmap)));
    assert_eq!(back.folders.iter().filter(|f| f.account == id).count(), FolderKind::STANDARD.len());
}

#[test]
fn only_a_jmap_or_microsoft_account_has_server_ids() {
    use AccountKind::*;
    assert_eq!([Local, Imap, Microsoft, Jmap, Unknown].map(AccountKind::server_ids), [false, false, true, true, false]);
    // A JMAP account is kept in step with a server, but nothing sends its Outbox yet. An account of a
    // kind this version doesn't know is neither: whatever it is, this version doesn't know how.
    assert_eq!([Local, Imap, Microsoft, Jmap, Unknown].map(AccountKind::has_server), [false, true, true, true, false]);
    assert_eq!([Local, Imap, Microsoft, Jmap, Unknown].map(AccountKind::sends_mail), [false, true, true, false, false]);
}

/// A JMAP account's server's mailboxes keep their names and places until changes go up: a rename
/// or a move made here would stay here alone. A folder made here is renamed and moved as any.
#[test]
fn a_jmap_servers_folder_is_neither_renamed_nor_moved_until_changes_go_up() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    assert!(!mb.has_jmap_account());
    let account = mb.add_account(Account { email: "ada@example.org".into(), kind: AccountKind::Jmap, ..Default::default() });
    assert!(mb.has_jmap_account());
    let theirs = mb.add_folder(account, None, "Projects").unwrap();
    mb.folder_mut(theirs).unwrap().server = Some(ServerFolder { id: "M5".into(), name: "Projects".into(), ..Default::default() });
    let ours = mb.add_folder(account, None, "Ours").unwrap();
    let before = mb.clone();
    assert!(mb.renaming_stays_on_server(theirs) && mb.folder_stays_on_server(theirs));
    assert_eq!(mb.rename_folder(theirs, "Plans").unwrap_err().to_string(), FOLDERS_STAY_YET);
    assert_eq!(mb.move_folder(theirs, Some(ours)).unwrap_err().to_string(), FOLDERS_STAY_YET);
    assert_eq!(mb, before);
    // Ours is renamed, goes inside theirs and back out (the server's folder isn't changed by it).
    assert!(!mb.renaming_stays_on_server(ours) && !mb.folder_stays_on_server(ours));
    mb.rename_folder(ours, "Mine").unwrap();
    mb.move_folder(ours, Some(theirs)).unwrap();
    mb.move_folder(ours, None).unwrap();
    assert_eq!(mb.folder(ours).map(|f| (f.name.as_str(), f.parent)), Some(("Mine", None)));
    assert_eq!(mb.folder(theirs), before.folder(theirs));
}

#[test]
fn repair_trims_rules_to_the_most_a_mailbox_keeps() {
    let (mut mb, _) = mailbox();
    for id in 1..=(MAX_RULES as Id + 50) {
        mb.rules.push(Rule {
            id,
            name: "r".into(),
            enabled: true,
            conditions: vec![Condition::HasAttachment],
            actions: vec![Action::Flag],
            stop: false,
        });
    }
    let fixed = mb.repair();
    assert_eq!(mb.rules.len(), MAX_RULES);
    assert!(fixed.iter().any(|f| f.contains("rules")), "{fixed:?}");
}

/// A mailbox of `accounts` accounts, each with the seven standard folders, as a file would hold it.
fn many_accounts(accounts: usize) -> Mailbox {
    let mut mb = Mailbox::default();
    for a in 1..=accounts as Id {
        mb.accounts.push(Account { id: a, name: format!("A{a}"), email: format!("a{a}@example.org"), ..Default::default() });
        for (i, kind) in FolderKind::STANDARD.into_iter().enumerate() {
            mb.folders.push(Folder { id: 1_000_000 + a * 10 + i as Id, account: a, name: kind.default_name().into(), kind, ..Default::default() });
        }
    }
    mb
}

#[test]
fn repair_of_many_accounts_is_linear_and_capped() {
    let mut mb = many_accounts(8000);
    let t = std::time::Instant::now();
    let fixed = mb.repair();
    assert!(t.elapsed().as_millis() < 1000, "{:?}", t.elapsed());
    assert_eq!(mb.accounts.len(), MAX_ACCOUNTS);
    assert!(fixed.iter().any(|f| f.contains("beyond the limits")), "{fixed:?}");
    // Every kept account still has each standard folder once, and no folder is without an account.
    assert_eq!(mb.folders.len(), MAX_ACCOUNTS * 7);
    assert!(mb.folders.iter().all(|f| mb.account(f.account).is_some()));
    // Under the caps nothing is dropped: the same shape as a person's.
    let mut ok = many_accounts(MAX_ACCOUNTS);
    assert!(ok.repair().is_empty());
}

#[test]
fn accounts_in_order_is_linear() {
    let mb = many_accounts(8000);
    let t = std::time::Instant::now();
    assert_eq!(mb.accounts_in_order().len(), 8000);
    assert!(t.elapsed().as_millis() < 1000, "{:?}", t.elapsed());
}

#[test]
fn repair_of_a_long_chain_of_folders_is_linear() {
    let (mut mb, account) = mailbox();
    let base = 10_000_000;
    for i in 0..(MAX_FOLDERS as Id - 10) {
        let parent = if i == 0 { None } else { Some(base + i - 1) };
        mb.folders.push(Folder { id: base + i, account, name: format!("f{i}"), parent, ..Default::default() });
    }
    let t = std::time::Instant::now();
    mb.repair();
    assert!(t.elapsed().as_millis() < 1000, "{:?}", t.elapsed());
    assert!(mb.folders.len() <= MAX_FOLDERS);
}

#[test]
fn repair_cleans_names_and_caps_categories() {
    let (mut mb, account) = mailbox();
    mb.folders.push(Folder { id: 900, account, name: "  Bad\u{7}\nname  ".into(), ..Default::default() });
    mb.categories = (0..MAX_CATEGORIES + 50).map(|i| Category { name: format!("c{i}"), ..Default::default() }).collect();
    mb.repair();
    assert_eq!(mb.folder(900).map(|f| f.name.as_str()), Some("Badname"));
    assert_eq!(mb.categories.len(), MAX_CATEGORIES);
}

#[test]
fn a_file_with_too_many_items_is_refused_while_it_is_read() {
    let mut json = String::from("{\"messages\":{");
    for i in 1..=2000 {
        json.push_str(&format!("{}\"{i}\":{{}}", if i > 1 { "," } else { "" }));
    }
    json.push_str("}}");
    assert_eq!(from_json(json.as_bytes(), 2000).map(|m| m.messages.len()).ok(), Some(2000));
    let err = from_json(json.as_bytes(), 1999).unwrap_err().to_string();
    assert!(err.contains("more items"), "{err}");
    // The budget is not left set for the next read.
    assert_eq!(serde_json::from_str::<Mailbox>(&json).map(|m| m.messages.len()).ok(), Some(2000));
}

#[test]
fn a_switch_to_ask_before_deleting_a_conversation_this_version_cant_read_still_opens_the_mailbox() {
    // As its neighbours: written as nothing this version reads, it is "nothing was said" (#503).
    for written in ["null", "\"yes\"", "1", "{\"ask\": false}"] {
        let json = format!(r#"{{"id": 3, "email": "noor@example.org", "skipConversationQuestion": {written}}}"#);
        let read = serde_json::from_str::<Account>(&json);
        assert!(read.as_ref().is_ok_and(|a| !a.skip_conversation_question), "{written}: {read:?}");
    }
    let said: Account = serde_json::from_str(r#"{"id": 3, "email": "noor@example.org", "skipConversationQuestion": true}"#).unwrap();
    assert!(said.skip_conversation_question);
}
