// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snooze (#949) as each kind of account's server hears of it: the folder and the move go up as
//! any new folder and any move do, and the return is a move back. IMAP is looked at in the
//! operations of a Send/Receive request, Microsoft Graph and an Exchange server in their
//! requests with answers made by hand; a JMAP account's server, which isn't told of moves yet,
//! refuses it. (Against the stand-ins themselves: the apps' flow tests.)

use mailcraft_model::sync::{
    EwsFolderDeltas, EwsMailResult, EwsMessagePage, FolderChange, FolderChangeDone, FolderDelta, GraphFolder, GraphMessage, MessageChange,
    MessageChangeDone, MessageOutcome, RemoteOp, ServerMessage, SyncResult,
};
use mailcraft_model::{Account, AccountKind, Address, FolderKind, Id, Message, ServerFolder};
use serde_json::json;

use super::account::{apply_sync, sync_request};
use super::ews_mail::up_tests::{folder_at, piece, setup as ews_setup, took};
use super::ews_mail::{apply_ews_mail, ews_mail_request};
use super::graph_mail::graph_mail_request;
use super::graph_up::tests::{NOW, setup as graph_setup};
use crate::{Clock, Session};

fn at(s: &mut Session, now: i64) {
    s.clock = Clock { now: Some(now), offset: Some(0), zone: None };
}

/// An IMAP account whose Inbox follows "INBOX" and holds "Kickoff" (UID 5), the server having
/// listed it: the session, the account, the Inbox and the message.
fn imap() -> (Session, Id, Id, Id) {
    let mut s = Session::empty();
    let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let kickoff = Message { uid: 5, subject: "Kickoff".into(), message_id: "kickoff@mail.example".into(), date: 100, ..Default::default() };
    let folder =
        FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 5, new: vec![kickoff], ..Default::default() };
    apply_sync(&mut s, SyncResult { account, folders: vec![folder], ..Default::default() }).unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let message = s.mailbox.messages_in(inbox).next().map(|m| m.id).unwrap();
    at(&mut s, NOW);
    (s, account, inbox, message)
}

fn imap_ops(s: &Session, account: Id) -> Vec<RemoteOp> {
    sync_request(s, account).unwrap().ops
}

#[test]
fn an_imap_server_is_told_to_make_the_folder_and_move_the_message_and_then_to_move_it_back() {
    let (mut s, account, inbox, m) = imap();
    s.run("mail.snooze", &json!({"ids": [m], "until": NOW + 600})).unwrap();
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    assert_eq!(
        imap_ops(&s, account),
        [RemoteOp::Move { message: m, from: "INBOX".into(), uid: 5, to: String::new(), to_folder: snoozed, create: vec!["Snoozed".into()] }]
    );
    // The server did it: the folder is its mailbox now, and the message has a UID there.
    s.mailbox.pending.clear();
    s.mailbox.folder_mut(snoozed).unwrap().remote = "Snoozed".into();
    s.mailbox.folder_mut(snoozed).unwrap().uid_validity = 3;
    s.mailbox.message_mut(m).unwrap().uid = 1;
    // Return at the time: a move back, to the folder it came from.
    at(&mut s, NOW + 600);
    assert_eq!(s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([m]));
    assert_eq!(
        imap_ops(&s, account),
        [RemoteOp::Move { message: m, from: "Snoozed".into(), uid: 1, to: "INBOX".into(), to_folder: inbox, create: vec![] }]
    );
}

#[test]
fn snoozed_and_returned_before_the_server_heard_nothing_goes_up() {
    let (mut s, account, inbox, m) = imap();
    s.run("mail.snooze", &json!({"ids": [m], "until": NOW + 600})).unwrap();
    s.run("mail.unsnooze", &json!({"ids": [m]})).unwrap();
    assert_eq!(s.mailbox.message(m).unwrap().folder, inbox);
    assert_eq!(imap_ops(&s, account), []);
}

#[test]
fn a_move_out_of_snoozed_seen_in_a_run_calls_the_snooze_off() {
    let (mut s, account, inbox, m) = imap();
    s.run("mail.snooze", &json!({"ids": [m], "until": NOW + 600})).unwrap();
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    s.mailbox.pending.clear();
    s.mailbox.folder_mut(snoozed).unwrap().remote = "Snoozed".into();
    s.mailbox.message_mut(m).unwrap().uid = 1;
    // Another program moved it to the Inbox: the server's Snoozed no longer lists it, and its
    // Inbox lists it again as a new UID with the same Message-ID.
    let back = Message { uid: 9, subject: "Kickoff".into(), message_id: "kickoff@mail.example".into(), date: 100, ..Default::default() };
    let there = FolderDelta {
        id: Some(inbox),
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 7,
        uid_seen: 9,
        new: vec![back],
        ..Default::default()
    };
    let gone = FolderDelta {
        id: Some(snoozed),
        remote: "Snoozed".into(),
        kind: FolderKind::Custom,
        uid_validity: 3,
        removed: vec![1],
        ..Default::default()
    };
    apply_sync(&mut s, SyncResult { account, folders: vec![there, gone], ..Default::default() }).unwrap();
    assert!(s.mailbox.messages.values().all(|m| m.snooze.is_none_or(|z| !z.waiting())), "nothing waits in Snoozed any more");
    at(&mut s, NOW + 700);
    assert_eq!(s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([]));
}

#[test]
fn graph_hears_of_the_new_folder_and_the_move_and_a_return_is_a_move_to_the_inbox() {
    let (mut s, account, m, _) = graph_setup();
    s.run("mail.snooze", &json!({"ids": [m], "until": NOW + 600})).unwrap();
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    let request = graph_mail_request(&s, account).unwrap();
    assert!(
        request.changes.iter().any(|c| matches!(c, FolderChange::Create { folder, name, .. } if *folder == snoozed && name == "Snoozed")),
        "{:?}",
        request.changes
    );
    assert!(
        request
            .messages
            .iter()
            .any(|c| matches!(c, MessageChange::Move { message, id, to_folder, .. } if *message == m && id == "M-1" && *to_folder == Some(snoozed))),
        "{:?}",
        request.messages
    );
    // The server made the folder and moved the message there.
    s.mailbox.folder_mut(snoozed).unwrap().server = Some(ServerFolder { id: "F-sn".into(), name: "Snoozed".into(), ..Default::default() });
    s.mailbox.message_mut(m).unwrap().server.as_mut().unwrap().folder = "F-sn".into();
    s.mailbox_changed();
    assert_eq!(graph_mail_request(&s, account).unwrap().messages, []);
    at(&mut s, NOW + 600);
    assert_eq!(s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([m]));
    assert_eq!(
        graph_mail_request(&s, account).unwrap().messages,
        [MessageChange::Move { message: m, id: "M-1".into(), to: "F-in".into(), to_folder: None }]
    );
}

#[test]
fn graph_hears_nothing_of_a_snooze_returned_before_the_run() {
    let (mut s, account, m, _) = graph_setup();
    s.run("mail.snooze", &json!({"ids": [m], "until": NOW + 600})).unwrap();
    s.run("mail.unsnooze", &json!({"ids": [m]})).unwrap();
    assert_eq!(graph_mail_request(&s, account).unwrap().messages, []);
}

#[test]
fn an_exchange_server_hears_of_the_folder_and_the_move_and_a_return_is_a_move_back_to_its_inbox() {
    let (mut s, account, a, _) = ews_setup();
    let now = s.now();
    at(&mut s, now);
    s.run("mail.snooze", &json!({"ids": [a], "until": now + 600})).unwrap();
    let snoozed = s.mailbox.snoozed_folder(account).unwrap();
    let request = ews_mail_request(&s, account, false).unwrap();
    let [FolderChange::Create { folder, name, parent, .. }] = request.changes.as_slice() else { panic!("{:?}", request.changes) };
    assert_eq!((*folder, name.as_str(), parent.as_str()), (snoozed, "Snoozed", ""));
    assert!(
        request.messages.iter().any(|c| matches!(c, MessageChange::Move { id, to_folder, .. } if id == "A1" && *to_folder == Some(snoozed))),
        "{:?}",
        request.messages
    );
    let there = GraphFolder { id: "FS".into(), name: "Snoozed".into(), kind: FolderKind::Custom, ..Default::default() };
    let made = FolderChangeDone { change: request.changes[0].clone(), outcome: mailcraft_model::sync::ChangeOutcome::Done { folder: there } };
    let moved = MessageChangeDone {
        change: request.messages[0].clone(),
        outcome: MessageOutcome::Done { categories: None, flag: None, folder: "FS".into(), id: "A1-moved".into() },
        there: None,
    };
    took(&mut s, account, EwsMailResult { changes: vec![made], messages: vec![moved], partial: true, ..piece(account) });
    assert_eq!(s.mailbox.message(a).unwrap().server.as_ref().unwrap().folder, "FS");
    at(&mut s, now + 601);
    assert_eq!(s.run("mail.returnDue", &json!({})).unwrap()["returned"], json!([a]));
    let next = ews_mail_request(&s, account, false).unwrap();
    assert!(next.messages.iter().any(|c| matches!(c, MessageChange::Move { id, to, .. } if id == "A1-moved" && to == "F1")), "{:?}", next.messages);
}

#[test]
fn an_exchange_server_bringing_a_reply_in_the_same_conversation_brings_the_snoozed_message_back() {
    let (mut s, account, a, b) = ews_setup();
    let now = s.now();
    at(&mut s, now);
    for id in [a, b] {
        s.mailbox.message_mut(id).unwrap().server.as_mut().unwrap().thread = format!("C-{id}");
    }
    s.run("mail.snooze", &json!({"ids": [a, b], "until": now + 86_400})).unwrap();
    let there = EwsFolderDeltas { changed: vec![folder_at("FS", "Snoozed")], ..Default::default() };
    took(&mut s, account, EwsMailResult { folders: Some(there), hierarchy: "H2".into(), partial: true, ..piece(account) });
    let reply = GraphMessage {
        server: ServerMessage { id: "A9".into(), folder: "F1".into(), change_key: "CK".into(), thread: format!("C-{a}"), ..Default::default() },
        message_id: "a9@example.org".into(),
        subject: "Re: Kickoff".into(),
        from: Address::new("Zoe", "zoe@example.org"),
        received: now + 10,
        ..Default::default()
    };
    let page = EwsMessagePage { folder: "F1".into(), messages: vec![reply], last: true, from: "S1".into(), state: "S2".into(), ..Default::default() };
    took(&mut s, account, EwsMailResult { pages: vec![page], partial: true, ..piece(account) });
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let back = s.mailbox.message(a).unwrap();
    assert_eq!((back.folder, back.read), (inbox, false));
    assert!(back.snooze.is_some_and(|z| z.returned == Some(now)));
    assert_ne!(s.mailbox.message(b).unwrap().folder, inbox, "the other conversation waits on");
    let _ = apply_ews_mail;
}

#[test]
fn a_jmap_server_isnt_told_of_moves_yet_so_a_message_it_has_is_refused_and_stays() {
    let mut s = Session::empty();
    let account =
        s.mailbox.add_account(Account { name: "Jay".into(), email: "jay@example.org".into(), kind: AccountKind::Jmap, ..Default::default() });
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let server = ServerMessage { id: "J1".into(), folder: "mb-in".into(), ..Default::default() };
    let m = s.mailbox.add_message(Message { account, folder: inbox, server: Some(server), ..Default::default() }).unwrap();
    at(&mut s, NOW);
    let refused = s.run("mail.snooze", &json!({"ids": [m], "until": NOW + 600}));
    assert!(refused.is_err());
    assert_eq!(s.mailbox.message(m).unwrap().folder, inbox);
    assert!(s.mailbox.snoozed_folder(account).is_none());
    // Mail only this computer has in that account moves as any other.
    let local = s.mailbox.add_message(Message { account, folder: inbox, ..Default::default() }).unwrap();
    s.run("mail.snooze", &json!({"ids": [local], "until": NOW + 600})).unwrap();
    assert_eq!(s.mailbox.message(local).unwrap().folder, s.mailbox.snoozed_folder(account).unwrap());
}
