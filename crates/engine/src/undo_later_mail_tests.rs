// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Undo takes back what its step did and nothing else (#605): mail that arrived after the step,
//! and how far each IMAP folder has been read, stay as they are.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use crate::Session;

fn received(uid: u32, subject: &str) -> Message {
    Message {
        uid,
        message_id: format!("m{uid}@example.org"),
        subject: subject.into(),
        from: Address::new("Bo", "bo@example.org"),
        date: 100 + i64::from(uid),
        ..Default::default()
    }
}

/// What a Send/Receive brings into `folder` of `account`: `new` mail, the folder now read up to `seen`.
fn arrives(s: &mut Session, account: Id, folder: Option<Id>, seen: u32, new: Vec<Message>) {
    let delta =
        FolderDelta { id: folder, remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: seen, new, ..Default::default() };
    s.run("sync.apply", &json!({"result": SyncResult { account, folders: vec![delta], ..Default::default() }})).unwrap();
}

/// An IMAP account whose Inbox has one message from its server (uid 5): session, account, Inbox, message.
fn imap() -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    arrives(&mut s, a, None, 5, vec![received(5, "Plans")]);
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let id = s.mailbox.messages_in(inbox).next().unwrap().id;
    (s, a, inbox, id)
}

fn with_subject(s: &Session, subject: &str) -> Option<Id> {
    s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id)
}

#[test]
fn undo_of_a_flag_keeps_mail_that_came_in_after_it() {
    let (mut s, a, inbox, plans) = imap();
    s.run("mail.flag", &json!({"ids": [plans]})).unwrap();
    assert!(s.mailbox.message(plans).unwrap().flag == mailcraft_model::FlagStatus::Flagged);
    arrives(&mut s, a, Some(inbox), 6, vec![received(6, "Later")]);
    let folder_then = s.mailbox.folder(inbox).unwrap().clone();
    assert_eq!(folder_then.uid_seen, 6);

    assert!(s.undo());
    assert!(s.mailbox.message(plans).unwrap().flag != mailcraft_model::FlagStatus::Flagged, "the flag is taken off");
    let later = with_subject(&s, "Later").expect("the mail that arrived after the step is kept");
    assert_eq!((s.mailbox.message(later).unwrap().uid, s.mailbox.message(later).unwrap().folder), (6, inbox));
    assert_eq!(s.mailbox.folder(inbox).unwrap(), &folder_then, "how far the folder has been read is kept");

    assert!(s.redo());
    assert!(s.mailbox.message(plans).unwrap().flag == mailcraft_model::FlagStatus::Flagged);
    assert!(with_subject(&s, "Later").is_some());
    assert_eq!(s.mailbox.folder(inbox).unwrap().uid_seen, 6);
}

#[test]
fn undo_of_a_delete_puts_the_message_back_and_keeps_later_mail() {
    let (mut s, a, inbox, plans) = imap();
    s.run("mail.delete", &json!({"ids": [plans]})).unwrap();
    arrives(&mut s, a, Some(inbox), 6, vec![received(6, "Later")]);
    assert!(s.undo());
    assert_eq!(s.mailbox.message(plans).unwrap().folder, inbox, "back in the Inbox");
    assert!(with_subject(&s, "Later").is_some());
    assert_eq!(s.mailbox.folder(inbox).unwrap().uid_seen, 6);
    // The move to Deleted Items that waited for the server is called off.
    assert!(s.mailbox.pending.is_empty(), "{:?}", s.mailbox.pending);
    assert!(s.redo());
    assert_ne!(s.mailbox.message(plans).unwrap().folder, inbox);
    assert!(with_subject(&s, "Later").is_some());
}

#[test]
fn undo_of_a_step_that_added_a_message_removes_it_and_keeps_later_mail() {
    let (mut s, a, inbox, _) = imap();
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Queued", "account": a})).unwrap();
    let queued = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    arrives(&mut s, a, Some(inbox), 6, vec![received(6, "Later")]);
    assert!(s.undo());
    assert!(s.mailbox.message(queued).is_none(), "what the step added is taken away");
    assert!(with_subject(&s, "Later").is_some());
    assert!(s.redo());
    assert!(s.mailbox.message(queued).is_some());
    assert!(with_subject(&s, "Later").is_some());
}

#[test]
fn undo_of_a_move_goes_back_and_keeps_later_mail_and_its_own_moves() {
    let (mut s, a, inbox, plans) = imap();
    let archive = s.mailbox.folder_of_kind(a, FolderKind::Archive).unwrap();
    s.run("mail.move", &json!({"ids": [plans], "folder": archive})).unwrap();
    assert_eq!(s.mailbox.pending.len(), 1);
    arrives(&mut s, a, Some(inbox), 6, vec![received(6, "Later")]);
    // Something moves the new mail on its own, as a rule does: a waiting change of its own.
    let later = with_subject(&s, "Later").unwrap();
    s.mailbox.move_message(later, archive).unwrap();
    assert_eq!(s.mailbox.pending.len(), 2);

    assert!(s.undo());
    assert_eq!(s.mailbox.message(plans).unwrap().folder, inbox);
    assert_eq!(s.mailbox.message(plans).unwrap().uid, 5, "the server's copy is known again");
    assert_eq!(s.mailbox.message(later).unwrap().folder, archive, "later mail is where it is now");
    assert_eq!(s.mailbox.pending.len(), 1, "only the undone step's waiting change goes: {:?}", s.mailbox.pending);
    assert_eq!(s.mailbox.pending[0].source(), ("INBOX", 6));
}

#[test]
fn undo_of_a_new_folder_keeps_later_mail_even_in_the_folder() {
    let (mut s, a, inbox, plans) = imap();
    let made = s.run("folder.new", &json!({"name": "Receipts", "account": a, "parent": null})).unwrap()["id"].as_u64().unwrap();
    arrives(&mut s, a, Some(inbox), 6, vec![received(6, "Later")]);
    assert!(s.undo());
    assert!(s.mailbox.folder(made).is_none(), "the folder the step made is gone");
    assert!(with_subject(&s, "Later").is_some());
    assert!(s.mailbox.message(plans).is_some());
    assert_eq!(s.mailbox.folder(inbox).unwrap().uid_seen, 6);
    assert!(s.mailbox.messages.values().all(|m| s.mailbox.folder(m.folder).is_some()), "every message lies in a folder");
}

#[test]
fn undo_of_a_new_account_leaves_nothing_of_it_behind() {
    let (mut s, _, _, _) = imap();
    let b = s.run("account.add", &json!({"email": "bo@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    arrives(&mut s, b, None, 3, vec![received(3, "For Bo")]);
    assert!(s.undo());
    assert!(s.mailbox.account(b).is_none());
    assert!(s.mailbox.messages.values().all(|m| s.mailbox.account(m.account).is_some() && s.mailbox.folder(m.folder).is_some()));
    assert!(s.mailbox.folders.iter().all(|f| s.mailbox.account(f.account).is_some()));
    assert!(with_subject(&s, "For Bo").is_none());
}

#[test]
fn mail_the_server_took_away_since_stays_gone() {
    let (mut s, a, inbox, plans) = imap();
    arrives(&mut s, a, Some(inbox), 6, vec![received(6, "Later")]);
    s.run("mail.flag", &json!({"ids": [plans]})).unwrap();
    let later = with_subject(&s, "Later").unwrap();
    // A later run says the server no longer has uid 6.
    let delta = FolderDelta {
        id: Some(inbox),
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 7,
        uid_seen: 6,
        removed: vec![6],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": SyncResult { account: a, folders: vec![delta], ..Default::default() }})).unwrap();
    assert!(s.mailbox.message(later).is_none());
    assert!(s.undo());
    assert!(s.mailbox.message(later).is_none(), "undo doesn't bring back what the server removed");
    assert!(s.mailbox.message(plans).unwrap().flag != mailcraft_model::FlagStatus::Flagged);
}
