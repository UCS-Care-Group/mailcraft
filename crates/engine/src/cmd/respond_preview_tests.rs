// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message of which only the headers and a preview are here (#653) is never answered or
//! forwarded as if it were whole: Reply, Reply All and Forward refuse until it is downloaded,
//! also when it can't be fetched right now (a move within the account sets its UID to 0 until
//! the next Send/Receive, so the window has nothing to wait for), and also when it can be fetched
//! (an IMAP preview has its UID and folder): the command itself refuses, for the CLI, MCP and
//! scripts run it without the window's fetch first, which fetches and runs it again once whole.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, Body, FolderKind, Id, Message};
use serde_json::json;

use crate::{Session, sample};

fn session() -> Session {
    let now = mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

/// A sample message turned into a preview that can't be fetched: no server id, no UID.
fn preview(s: &mut Session) -> Id {
    let id = s.mailbox.messages.values().find(|m| !m.draft && !m.attachments.is_empty()).unwrap().id;
    let m = s.mailbox.message_mut(id).unwrap();
    m.unfetched = true;
    m.uid = 0;
    m.server = None;
    m.attachments.clear();
    m.body = Body { text: "Only the first 255 characters".into(), html: None };
    id
}

#[test]
fn a_preview_is_neither_answered_nor_forwarded() {
    let mut s = session();
    let id = preview(&mut s);
    assert!(!s.mailbox.fetchable(s.mailbox.message(id).unwrap()), "nothing to wait for");
    for command in ["mail.reply", "mail.replyAll", "mail.forward", "mail.replyAndDelete"] {
        let e = s.run(command, &json!({"id": id})).unwrap_err().to_string();
        assert!(e.contains("isn't downloaded yet"), "{command}: {e}");
        assert!(s.compose.is_none(), "{command}: nothing is being written");
    }
}

#[test]
fn a_message_downloaded_is_answered_as_before() {
    let mut s = session();
    let id = preview(&mut s);
    s.mailbox.message_mut(id).unwrap().unfetched = false;
    s.run("mail.reply", &json!({"id": id})).unwrap();
    assert!(s.compose.is_some());
}

/// An IMAP account's Inbox with one received message: the session, the Inbox, the message.
fn imap() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let received = Message {
        uid: 5,
        message_id: "plans@example.org".into(),
        subject: "Plans".into(),
        from: Address::new("Bo", "bo@example.org"),
        to: vec![Address::new("Ada", "ada@mail.example")],
        date: 100,
        ..Default::default()
    };
    let delta = FolderDelta { id: None, remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 5, new: vec![received], ..Default::default() };
    s.run("sync.apply", &json!({"result": SyncResult { account: a, folders: vec![delta], ..Default::default() }})).unwrap();
    let inbox = s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap();
    let id = s.mailbox.messages_in(inbox).next().unwrap().id;
    (s, inbox, id)
}

#[test]
fn a_preview_a_host_could_fetch_is_refused_by_the_command_too() {
    let (mut s, inbox, id) = imap();
    let m = s.mailbox.message_mut(id).unwrap();
    m.unfetched = true;
    m.body = Body { text: "Only the first 255 characters".into(), html: None };
    assert!(s.mailbox.fetchable(s.mailbox.message(id).unwrap()), "a UID in a folder with a mailbox name: a host could fetch it");
    for command in ["mail.reply", "mail.replyAll", "mail.forward", "mail.replyAndDelete"] {
        let e = s.run(command, &json!({"id": id})).unwrap_err().to_string();
        assert!(e.contains("isn't downloaded yet"), "{command}: {e}");
        assert!(s.compose.is_none(), "{command}: nothing is being written");
        assert_eq!(s.mailbox.message(id).unwrap().folder, inbox, "{command}: the original stays where it is");
    }
}

#[test]
fn an_imap_message_fetched_is_answered_and_forwarded_as_before() {
    for command in ["mail.reply", "mail.replyAll", "mail.forward", "mail.replyAndDelete"] {
        let (mut s, _, id) = imap();
        s.mailbox.message_mut(id).unwrap().unfetched = true;
        assert!(s.run(command, &json!({"id": id})).is_err(), "{command}: a preview");
        // The window fetches it and runs the command again once it is whole.
        s.mailbox.message_mut(id).unwrap().unfetched = false;
        s.run(command, &json!({"id": id})).unwrap();
        assert!(s.compose.is_some(), "{command}");
    }
}
