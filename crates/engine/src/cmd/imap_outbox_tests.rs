// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An IMAP account's Outbox sends only what was queued with Send (#607): a message the owner
//! moves into it stays there, and isn't sent, until Send Again.

use mailcraft_model::sync::ServerMessage;
use mailcraft_model::sync::{FolderDelta, RemoteOp, SyncResult};
use mailcraft_model::{Address, Body, Folder, FolderKind, Id, Message, Queued, ServerFolder};
use serde_json::json;

use crate::Session;
use crate::cmd::account::{apply_sync, sync_request};

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

/// A message whole on this computer, in a folder "Kept" there: its id.
fn kept_here(s: &mut Session, subject: &str) -> Id {
    let local = s.mailbox.local_account();
    let kept = s.mailbox.add_folder(local, None, "Kept").unwrap();
    let m = Message {
        folder: kept,
        subject: subject.into(),
        message_id: format!("{}@example.org", subject.to_lowercase()),
        from: Address::new("Ada", "ada@mail.example"),
        to: vec![Address::new("Cy", "cy@example.org")],
        date: 100,
        read: true,
        body: Body { text: format!("All of {subject}."), html: None },
        ..Default::default()
    };
    s.mailbox.add_message(m).unwrap()
}

/// The server said `sent` went, and that it filed a copy in its mailbox for sent mail; that copy
/// hasn't come down yet.
fn sent_and_filed(s: &mut Session, account: Id, sent: &[Id]) {
    apply_sync(s, SyncResult { account, sent: sent.to_vec(), filed: sent.to_vec(), ..Default::default() }).unwrap();
}

/// What the next run of `account` would put on its server.
fn to_place(s: &Session, account: Id) -> Vec<Id> {
    sync_request(s, account).unwrap().places.iter().map(|p| p.message.id).collect()
}

/// Whether anything still waits to put `id` on a server.
fn waits_to_be_placed(s: &Session, id: Id) -> bool {
    s.mailbox.queued.iter().any(|q| matches!(q, Queued::PlaceMessage { message, .. } if *message == id))
}

/// Moved in from this computer and sent with Send Again (#704): the send files it in Sent Items,
/// and the next run doesn't put it there again as a message moved in.
#[test]
fn a_message_moved_in_from_this_computer_and_sent_again_is_not_put_on_the_server_too() {
    let (mut s, a, _) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    let sent = s.mailbox.folder_of_kind(a, FolderKind::Sent).unwrap();
    let id = kept_here(&mut s, "Notes");
    s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap();
    assert!(to_place(&s, a).is_empty(), "nothing goes up from the Outbox");
    s.run("mail.sendAgain", &json!({"id": id})).unwrap();
    assert_eq!(to_send(&s, a), vec![id]);
    sent_and_filed(&mut s, a, &[id]);
    assert_eq!(s.mailbox.message(id).unwrap().folder, sent);
    assert!(to_place(&s, a).is_empty(), "the send filed it: it isn't put there twice");
    assert!(!waits_to_be_placed(&s, id));
    assert!(to_send(&s, a).is_empty());
}

/// The same for a copy from this computer.
#[test]
fn a_message_copied_in_and_sent_again_is_not_put_on_the_server_too() {
    let (mut s, a, _) = imap();
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    let id = kept_here(&mut s, "Notes");
    let copy = s.run("mail.copy", &json!({"ids": [id], "folder": outbox})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.sendAgain", &json!({"id": copy})).unwrap();
    sent_and_filed(&mut s, a, &[copy]);
    assert!(to_place(&s, a).is_empty());
    assert!(!waits_to_be_placed(&s, copy));
    // The message it was copied from stays where it was.
    assert!(s.mailbox.message(id).is_some_and(|m| m.subject == "Notes" && m.account != a));
}

/// Moved in from this computer into the Inbox, then on into the Outbox, and sent with Send
/// Again: filed by the send alone.
#[test]
fn a_message_moved_in_and_on_into_the_outbox_and_sent_again_is_not_put_on_the_server_too() {
    let (mut s, a, _) = imap();
    let (inbox, outbox) = (s.mailbox.folder_of_kind(a, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap());
    let id = kept_here(&mut s, "Notes");
    s.run("mail.move", &json!({"ids": [id], "folder": inbox})).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap();
    assert!(to_send(&s, a).is_empty(), "held: only Send sends");
    s.run("mail.sendAgain", &json!({"id": id})).unwrap();
    sent_and_filed(&mut s, a, &[id]);
    assert!(to_place(&s, a).is_empty());
    assert!(!waits_to_be_placed(&s, id));
}

/// Moved in from another IMAP account and sent with Send Again: not put on this server as a
/// message moved in. Its original stays on the other server until the copy the send filed comes
/// down here, and then goes, as the original of any message moved in does.
#[test]
fn a_message_moved_in_from_another_imap_account_and_sent_again_lets_its_original_go_once_filed() {
    let (mut s, a, _) = imap();
    let sent_delta =
        |uid_seen, new| FolderDelta { remote: "Sent".into(), kind: FolderKind::Sent, uid_validity: 3, uid_seen, new, ..Default::default() };
    apply_sync(&mut s, SyncResult { account: a, folders: vec![sent_delta(0, vec![])], ..Default::default() }).unwrap();
    let b = s.run("account.add", &json!({"email": "bo@other.example", "incoming": {"host": "imap.other.example"}})).unwrap()["id"].as_u64().unwrap();
    let theirs = Message {
        uid: 9,
        message_id: "hello@example.org".into(),
        subject: "Hello".into(),
        to: vec![Address::new("Cy", "cy@example.org")],
        date: 200,
        body: Body { text: "Hello there.".into(), html: None },
        ..Default::default()
    };
    let inbox = FolderDelta {
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 4,
        uid_seen: 9,
        new: vec![theirs.clone()],
        ..Default::default()
    };
    apply_sync(&mut s, SyncResult { account: b, folders: vec![inbox], ..Default::default() }).unwrap();
    let id = s.mailbox.messages.values().find(|m| m.subject == "Hello").unwrap().id;
    let outbox = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap();
    s.run("mail.sendAgain", &json!({"id": id})).unwrap();
    sent_and_filed(&mut s, a, &[id]);
    assert!(to_place(&s, a).is_empty());
    assert!(!waits_to_be_placed(&s, id));
    let deletes = |s: &Session| sync_request(s, b).unwrap().ops.into_iter().filter(|op| matches!(op, RemoteOp::Delete { .. })).count();
    assert_eq!(deletes(&s), 0, "the original stays until the filed copy is known here");
    // The copy the send filed comes down: it is this message, and the original goes.
    apply_sync(&mut s, SyncResult { account: a, folders: vec![sent_delta(1, vec![Message { uid: 1, read: true, ..theirs }])], ..Default::default() })
        .unwrap();
    assert_eq!(s.mailbox.messages.values().filter(|m| m.subject == "Hello").count(), 1);
    assert_ne!(s.mailbox.message(id).unwrap().uid, 0);
    assert_eq!(deletes(&s), 1);
}

/// A Microsoft account whose Inbox and Outbox follow folders of its server: the account, its
/// Inbox and its Outbox.
fn microsoft(s: &mut Session) -> (Id, Id, Id) {
    let ms = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let (inbox, outbox) = (s.mailbox.folder_of_kind(ms, FolderKind::Inbox).unwrap(), s.mailbox.folder_of_kind(ms, FolderKind::Outbox).unwrap());
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), name: "Inbox".into(), ..Default::default() });
    s.mailbox.folder_mut(outbox).unwrap().server = Some(ServerFolder { id: "F-ou".into(), name: "Outbox".into(), ..Default::default() });
    (ms, inbox, outbox)
}

fn graph_sends(s: &Session, account: Id) -> Vec<Id> {
    crate::cmd::graph_send::sends(s, account).iter().map(|u| u.message).collect()
}

/// Moved into a Microsoft account from this computer, and on into its Outbox before its server
/// has it (#704): it is still mail from elsewhere, which doesn't go into an Outbox that sends what
/// lies in it. It stays where it was, to be put on the server there, and nothing is sent.
#[test]
fn mail_moved_into_a_microsoft_account_doesnt_go_on_into_its_outbox_before_its_server_has_it() {
    let (mut s, _, _) = imap();
    let (ms, inbox, outbox) = microsoft(&mut s);
    let id = kept_here(&mut s, "Notes");
    s.run("mail.move", &json!({"ids": [id], "folder": inbox})).unwrap();
    let refused = s.run("mail.move", &json!({"ids": [id], "folder": outbox})).unwrap_err();
    assert!(refused.to_string().contains("can't go into an Outbox"), "{refused}");
    assert_eq!(s.mailbox.message(id).unwrap().folder, inbox);
    assert!(waits_to_be_placed(&s, id));
    assert!(graph_sends(&s, ms).is_empty());
}

/// A copy in a Microsoft account's Outbox would be sent by the next Send/Receive, and made on
/// the server too (#704): it isn't made.
#[test]
fn a_copy_doesnt_go_into_a_microsoft_accounts_outbox() {
    let (mut s, _, _) = imap();
    let (ms, inbox, outbox) = microsoft(&mut s);
    let server = ServerMessage { id: "M-1".into(), folder: "F-in".into(), ..Default::default() };
    let received = Message {
        folder: inbox,
        subject: "Invoice".into(),
        to: vec![Address::new("Cy", "cy@example.org")],
        server: Some(server),
        ..Default::default()
    };
    let m = s.mailbox.add_message(received).unwrap();
    let refused = s.run("mail.copy", &json!({"ids": [m], "folder": outbox})).unwrap_err();
    assert!(refused.to_string().contains(mailcraft_model::COPY_INTO_OUTBOX), "{refused}");
    assert_eq!(s.mailbox.messages_in(outbox).count(), 0);
    assert!(s.mailbox.queued.is_empty(), "{:?}", s.mailbox.queued);
    assert!(graph_sends(&s, ms).is_empty());
    // Moved there, the server's message stays and isn't sent (#607).
    s.run("mail.move", &json!({"ids": [m], "folder": outbox})).unwrap();
    assert!(graph_sends(&s, ms).is_empty());
}
