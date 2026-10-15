use std::sync::Arc;

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
    assert_eq!(mb.pending, [Pending::Delete { account, from: "INBOX".into(), uid: 44, remote_id: String::new() }]);
    // For good straight away.
    mb.remove_message(b).unwrap();
    assert_eq!(mb.pending.last(), Some(&Pending::Delete { account, from: "INBOX".into(), uid: 45, remote_id: String::new() }));
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
fn mail_moved_to_another_account_stays_on_its_own_server() {
    let (mut mb, account, a) = synced_mailbox();
    let archive = mb.folder_of_kind(account, FolderKind::Archive).unwrap();
    let other = mb.add_account(Account { email: "work@example.org".into(), ..Default::default() });
    let other_inbox = mb.folder_of_kind(other, FolderKind::Inbox).unwrap();
    mb.folder_mut(other_inbox).unwrap().remote = "INBOX".into();
    // A server can't hand a message to another one.
    mb.move_message(a, other_inbox).unwrap();
    assert!(mb.pending.is_empty());
    assert_eq!(mb.message(a).map(|m| (m.account, m.uid, m.synced)), Some((other, 0, None)));
    // Nor when a move within its own account was still waiting; and the other account's INBOX
    // is not the one it came from.
    let b = mb.add_message(Message { uid: 45, synced: Some(unread(45)), ..mail(mb.inbox().unwrap(), "Two", 2) }).unwrap();
    mb.move_message(b, archive).unwrap();
    assert_eq!(mb.pending.len(), 1);
    mb.move_message(b, other_inbox).unwrap();
    assert!(mb.pending.is_empty());
    assert_eq!(mb.message(b).map(|m| (m.account, m.uid)), Some((other, 0)));
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
    mb.move_message(a, projects).unwrap();
    mb.move_message(b, archive).unwrap();
    mb.remove_message(c).unwrap();
    mb.move_message(d, archive).unwrap();
    assert_eq!(mb.pending.len(), 4);
    // Into Deleted Items, the folder is still a folder; the second delete removes it for good.
    assert_eq!(mb.delete_folder(projects), Ok(false));
    assert_eq!(mb.pending.len(), 4);
    assert_eq!(mb.delete_folder(projects), Ok(true));
    assert_eq!(mb.pending.len(), 3);
    assert_eq!((mb.delete_folder(old), mb.delete_folder(old)), (Ok(false), Ok(true)));
    // What was to go into Projects, and what was to leave Old or be deleted there, is forgotten;
    // the rest waits on.
    assert_eq!(mb.pending, [Pending::Move { account, message: d, from: "INBOX".into(), uid: 45, synced: None, to: archive }]);
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
            Pending::Delete { account: 1, from: "INBOX".into(), uid: 50, remote_id: String::new() },
            Pending::Delete { account: 2, from: "INBOX".into(), uid: 53, remote_id: String::new() },
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

/// A mailbox whose account has a mail server, with "Projects" (and "Apollo" inside it) following
/// mailboxes there, and Deleted Items following the server's.
fn served() -> (Mailbox, Id, Id, Id) {
    let mut mb = Mailbox::default();
    let account = mb.add_account(Account { email: "ada@example.org".into(), kind: AccountKind::Imap, ..Default::default() });
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    mb.folder_mut(deleted).unwrap().remote = "Deleted Items".into();
    let projects = mb.add_folder(account, None, "Projects").unwrap();
    let apollo = mb.add_folder(account, Some(projects), "Apollo").unwrap();
    mb.folder_mut(projects).unwrap().remote = "Projects".into();
    mb.folder_mut(apollo).unwrap().remote = "Projects/Apollo".into();
    (mb, account, projects, apollo)
}

#[test]
fn what_the_owner_does_to_folders_is_noted_for_the_server_once() {
    let (mut mb, account, projects, _) = served();
    let made = mb.add_folder(account, None, "New").unwrap();
    mb.note_folder_made(made);
    mb.note_folder_made(made);
    // Renamed, then moved: one change, from where the server has it.
    mb.rename_folder(projects, "Plans").unwrap();
    mb.note_folder_moved(projects, None);
    mb.move_folder(projects, Some(made)).unwrap();
    mb.note_folder_moved(projects, None);
    assert_eq!(
        mb.folder_changes,
        [FolderChange::Create { account, folder: made }, FolderChange::Rename { account, folder: projects, from: "Projects".into(), parent: None }]
    );
    assert!(mb.clone().repair().is_empty());
    // Deleted for good: its mailboxes go, the innermost first, and its own change with it.
    let deleted = mb.folder_of_kind(account, FolderKind::Deleted).unwrap();
    mb.move_folder(projects, Some(deleted)).unwrap();
    assert!(mb.delete_folder(projects).unwrap());
    assert_eq!(
        mb.folder_changes,
        [
            FolderChange::Create { account, folder: made },
            FolderChange::Delete { account, remote: "Projects/Apollo".into() },
            FolderChange::Delete { account, remote: "Projects".into() },
        ]
    );
    // Nothing is noted for a folder of an account without a server.
    let (mut local, local_account) = mailbox();
    let f = local.add_folder(local_account, None, "Mine").unwrap();
    local.note_folder_made(f);
    assert!(local.delete_folder(f).is_ok() && local.folder_changes.is_empty());
}

#[test]
fn a_folder_forgotten_takes_its_messages_and_changes_and_tells_nobody() {
    let (mut mb, account, projects, apollo) = served();
    let m = mb.add_message(Message { folder: apollo, uid: 4, ..Default::default() }).unwrap();
    mb.pending.push(Pending::Delete { account, from: "Projects/Apollo".into(), uid: 9, remote_id: String::new() });
    mb.note_folder_moved(apollo, Some(projects));
    mb.forget_folder(projects).unwrap();
    assert!(mb.folder(projects).is_none() && mb.folder(apollo).is_none() && mb.message(m).is_none());
    assert!(mb.pending.is_empty() && mb.folder_changes.is_empty());
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert!(mb.forget_folder(inbox).is_err() && mb.forget_folder(999).is_err());
}

#[test]
fn a_rename_on_the_server_renames_what_lies_inside_and_what_waits() {
    let (mut mb, account, projects, apollo) = served();
    mb.pending.push(Pending::Delete { account, from: "Projects/Apollo".into(), uid: 9, remote_id: String::new() });
    mb.folder_changes.push(FolderChange::Rename { account, folder: apollo, from: "Projects/Apollo".into(), parent: Some(projects) });
    let other = mb.add_folder(account, None, "Projects2").unwrap();
    mb.folder_mut(other).unwrap().remote = "Projects2".into();
    mb.remote_renamed(account, "Projects", "Deleted Items/Projects", "/");
    let remote = |id| mb.folder(id).unwrap().remote.clone();
    assert_eq!(
        (remote(projects), remote(apollo), remote(other)),
        ("Deleted Items/Projects".into(), "Deleted Items/Projects/Apollo".into(), "Projects2".into())
    );
    assert_eq!(mb.pending[0].source(), ("Deleted Items/Projects/Apollo", 9));
    assert!(matches!(&mb.folder_changes[0], FolderChange::Rename { from, .. } if from == "Deleted Items/Projects/Apollo"));
    // A file that asks the server to change folders it never had is put right.
    mb.folder_changes.push(FolderChange::Rename { account, folder: 999, from: "INBOX".into(), parent: None });
    mb.folder_changes.push(FolderChange::Delete { account, remote: "Projects2".into() });
    mb.folder_changes.push(FolderChange::Create { account: 12345, folder: other });
    let fixed = mb.repair();
    assert_eq!(fixed.len(), 1, "{fixed:?}");
    assert_eq!(mb.folder_changes.len(), 1);
}
