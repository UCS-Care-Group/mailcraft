// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An IMAP account's Outbox sends only what was queued with Send (#607): a message the owner
//! moves into it stays there, and isn't sent, until Send Again.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, Folder, FolderKind, Id, Message, ServerFolder};
use serde_json::json;

use crate::Session;
use crate::cmd::account::sync_request;

/// An IMAP account with one received message in its Inbox: the session, the account, the message.
fn imap() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let received = Message {
        uid: 5,
        message_id: "plans@example.org".into(),
        subject: "Plans".into(),
        from: Address::new("Bo", "bo@example.org"),
        to: vec![Address::new("Cy", "cy@example.org")],
        date: 100,
        ..Default::default()
    };
    let delta = FolderDelta {
        id: None,
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 7,
        uid_seen: 5,
        new: vec![received],
        ..Default::default()
    };
    s.run("sync.apply", &json!({"result": SyncResult { account: a, folders: vec![delta], ..Default::default() }})).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let id = s.mailbox.messages_in(inbox).next().unwrap().id;
    (s, a, id)
}

fn to_send(s: &Session, account: Id) -> Vec<Id> {
    sync_request(s, account).unwrap().outbox.iter().map(|m| m.id).collect()
}

#[test]
fn a_message_moved_into_the_outbox_is_not_sent() {
    let (mut s, a, id) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap();
    assert_eq!(s.mailbox.message(id).unwrap().folder, outbox, "it stays in the Outbox");
    assert!(to_send(&s, a).is_empty(), "nothing is sent that was only moved there");
}

#[test]
fn what_was_queued_with_send_is_sent_beside_what_was_moved() {
    let (mut s, a, id) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Queued", "account": a})).unwrap();
    let queued = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap();
    assert_eq!(to_send(&s, a), vec![queued]);
}

#[test]
fn send_again_sends_a_message_moved_into_the_outbox() {
    let (mut s, a, id) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap();
    s.run("mail.sendAgain", &json!({"id": id})).unwrap();
    assert_eq!(to_send(&s, a), vec![id]);
}

#[test]
fn a_message_moved_out_of_the_outbox_again_is_no_longer_held() {
    let (mut s, a, id) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": inbox})).unwrap();
    assert!(s.mailbox.message(id).unwrap().held.is_empty());
}

#[test]
fn a_message_copied_into_the_outbox_is_not_sent() {
    let (mut s, a, id) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    s.run("mail.copy", &json!({"ids": [id], "folder": outbox})).unwrap();
    assert_eq!(s.mailbox.messages_in(outbox).count(), 1, "the copy is in the Outbox");
    assert!(to_send(&s, a).is_empty(), "nothing is sent that was only copied there");
}

#[test]
fn a_copy_out_of_the_outbox_is_not_held() {
    let (mut s, a, id) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap();
    s.run("mail.copy", &json!({"ids": [id], "folder": inbox})).unwrap();
    assert!(s.mailbox.messages_in(inbox).all(|m| m.held.is_empty()));
}

/// A message of a Microsoft account goes into another account by its own path (`move_across_graph`):
/// moved into an IMAP Outbox that way, it is held all the same.
#[test]
fn a_message_moved_from_a_microsoft_account_into_the_outbox_is_not_sent() {
    let (mut s, a, _) = imap();
    let microsoft = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = match s.mailbox.folder_of_kind(microsoft, FolderKind::Inbox) {
        Some(inbox) => inbox,
        None => {
            let inbox = s.mailbox.alloc_id();
            s.mailbox.folders.push(Folder { id: inbox, account: microsoft, name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() });
            inbox
        }
    };
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), name: "Inbox".into(), ..Default::default() });
    let server = ServerMessage { id: "M-1".into(), folder: "F-in".into(), ..Default::default() };
    let received = Message {
        folder: inbox,
        subject: "Invoice 42".into(),
        to: vec![Address::new("Cy", "cy@example.org")],
        server: Some(server),
        ..Default::default()
    };
    let m = s.mailbox.add_message(received).unwrap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    s.run("mail.move", &json!({"ids": [m], "folder": outbox})).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().folder, outbox, "it is in the IMAP Outbox");
    assert!(to_send(&s, a).is_empty(), "nothing is sent that was only moved there");
}
