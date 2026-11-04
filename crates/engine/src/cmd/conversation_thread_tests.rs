// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A conversation is one thread with one subject (#795): two unrelated messages that happen to
//! have the same subject are two conversations in the list, in the reading pane and for
//! Delete, and mail kept from before conversation ids were asked for gets them from its server,
//! a run at a time.

use mailcraft_model::sync::{ServerMessage, ThreadOf};
use mailcraft_model::{Account, AccountKind, FolderKind, Id, Mailbox, Message};
use serde_json::{Value, json};

use crate::Session;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn mail(folder: Id, subject: &str, message_id: &str, when: i64) -> Message {
    Message { folder, subject: subject.into(), message_id: message_id.into(), date: when, received: when, ..Default::default() }
}

/// A mailbox on this computer with two unrelated "Invoice for October" messages from two senders,
/// one of them answered from Sent Items.
fn invoices() -> (Session, [Id; 3]) {
    let mut mb = Mailbox::with_local_account("Avery", "avery@example.org");
    let account = mb.accounts[0].id;
    let (inbox, sent) = (mb.folder_of_kind(account, FolderKind::Inbox).unwrap(), mb.folder_of_kind(account, FolderKind::Sent).unwrap());
    let ours = mb.add_message(mail(inbox, "Invoice for October", "inv.1@one.example", 1_000)).unwrap();
    let answer = Message {
        in_reply_to: "inv.1@one.example".into(),
        references: vec!["inv.1@one.example".into()],
        ..mail(sent, "RE: Invoice for October", "inv.2@example.org", 2_000)
    };
    let answer = mb.add_message(answer).unwrap();
    let theirs = mb.add_message(mail(inbox, "Invoice for October", "inv.1@two.example", 3_000)).unwrap();
    let mut s = Session::new(mb);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.conversations", json!({"value": true}));
    (s, [ours, answer, theirs])
}

#[test]
fn two_unrelated_messages_with_one_subject_are_two_rows_and_delete_conversation_takes_one() {
    let (mut s, [ours, answer, theirs]) = invoices();
    let rows = s.rows();
    let row = |id: Id| rows.iter().find(|r| r.members.contains(&id)).unwrap();
    assert_eq!(rows.len(), 2, "two conversations, not one");
    assert_eq!((row(ours).members.clone(), row(ours).others.clone()), (vec![ours], vec![answer]));
    assert_eq!((row(theirs).members.clone(), row(theirs).others.clone()), (vec![theirs], vec![]));
    // The reading pane's conversation, as the list's.
    assert_eq!(*s.conversation(ours), [ours, answer]);
    assert_eq!(*s.conversation(theirs), [theirs]);
    // Delete on ours, its closed row picked (it stands for its conversation): theirs stays.
    run(&mut s, "mail.select", json!({"id": ours}));
    run(&mut s, "mail.delete", json!({}));
    let deleted = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Deleted).unwrap();
    let inbox = s.mailbox.folder_of_kind(s.mailbox.accounts[0].id, FolderKind::Inbox).unwrap();
    assert_eq!(s.mailbox.message(ours).unwrap().folder, deleted);
    assert_eq!(s.mailbox.message(theirs).unwrap().folder, inbox, "the unrelated message is left where it was");
    // And opening one conversation opens it alone.
    run(&mut s, "edit.undo", json!({}));
    run(&mut s, "mail.expandConversation", json!({"id": ours}));
    let rows = s.rows();
    assert!(rows.iter().filter(|r| r.under.is_some()).all(|r| r.id != theirs));
    assert!(!rows.iter().find(|r| r.id == theirs).unwrap().open);
}

/// A Microsoft account's mail kept by an earlier version: headers and a preview, and no
/// conversation id yet.
fn microsoft(ids: &[(&str, &str)]) -> (Session, Id, Vec<Id>) {
    let mut mb = Mailbox::default();
    let account =
        mb.add_account(Account { name: "me@example.org".into(), email: "me@example.org".into(), kind: AccountKind::Microsoft, ..Default::default() });
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let made: Vec<Id> = ids
        .iter()
        .enumerate()
        .map(|(n, (server, message_id))| {
            let when = 1_000 * i64::try_from(n).unwrap();
            let server = ServerMessage { id: (*server).into(), ..Default::default() };
            mb.add_message(Message { account, server: Some(server), unfetched: true, ..mail(inbox, "Invoice", message_id, when) }).unwrap()
        })
        .collect();
    (Session::new(mb), account, made)
}

#[test]
fn mail_kept_from_before_gets_its_conversation_ids_a_run_at_a_time() {
    let (mut s, account, made) = microsoft(&[("AAk-1", "a@one.example"), ("AAk-2", "b@two.example"), ("AAk-3", "c@three.example")]);
    // Until they come, they stay together as before.
    assert_eq!(*s.conversation(made[0]), made);
    // Newest first, each once.
    let asked = crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap().threads;
    assert_eq!(asked, ["AAk-3", "AAk-2", "AAk-1"]);
    // The server says two; of the third it says none: it isn't asked again.
    let threads = vec![
        ThreadOf { id: "AAk-1".into(), thread: "AAQkconvA".into() },
        ThreadOf { id: "AAk-2".into(), thread: "AAQkconvB".into() },
        ThreadOf { id: "AAk-3".into(), thread: String::new() },
        // One it wasn't asked about, or that isn't this account's, is nothing.
        ThreadOf { id: "AAk-9".into(), thread: "AAQkconvA".into() },
    ];
    let result = mailcraft_model::sync::GraphMailResult { account, email: "me@example.org".into(), threads, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    assert!(crate::cmd::graph_mail::graph_mail_request(&s, account).unwrap().threads.is_empty());
    let server = |id: Id| s.mailbox.message(id).unwrap().server.clone().unwrap();
    assert_eq!((server(made[0]).thread, server(made[2]).thread_asked), ("AAQkconvA".to_string(), true));
    // Now each is what its id says: A and B apart; C, still not known, alone among the known.
    assert_eq!(*s.conversation(made[0]), [made[0]]);
    assert_eq!(*s.conversation(made[1]), [made[1]]);
    assert_eq!(*s.conversation(made[2]), [made[2]]);
}

#[test]
fn a_listing_without_a_conversation_id_keeps_the_one_known() {
    let (mut s, account, made) = microsoft(&[("AAk-1", "a@one.example")]);
    s.mailbox.message_mut(made[0]).unwrap().server.as_mut().unwrap().thread = "AAQkconvA".into();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(mailcraft_model::ServerFolder { id: "F".into(), ..Default::default() });
    // A delta that says the message again without its conversation (as a JMAP window's marks do).
    let there = mailcraft_model::sync::GraphMessage {
        server: ServerMessage { id: "AAk-1".into(), read: true, ..Default::default() },
        subject: "Invoice".into(),
        received: 5,
        ..Default::default()
    };
    let page = mailcraft_model::sync::MessagePage { folder: "F".into(), messages: vec![there], ..Default::default() };
    let result = mailcraft_model::sync::GraphMailResult { account, email: "me@example.org".into(), pages: vec![page], ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    let server = s.mailbox.message(made[0]).unwrap().server.clone().unwrap();
    assert_eq!((server.read, server.thread.as_str()), (true, "AAQkconvA"));
}

#[test]
fn hostile_conversation_answers_are_held_to_their_sizes() {
    let (mut s, account, made) = microsoft(&[("AAk-1", "a@one.example")]);
    let mut threads: Vec<ThreadOf> = (0..5_000).map(|n| ThreadOf { id: format!("junk-{n}"), thread: "x".repeat(10_000) }).collect();
    threads.push(ThreadOf { id: "AAk-1".into(), thread: "conv\n\u{0}".into() });
    let result = mailcraft_model::sync::GraphMailResult { account, email: "me@example.org".into(), threads, ..Default::default() };
    crate::cmd::graph_mail::apply_graph_mail(&mut s, result).unwrap();
    // Past the most a run asks about, nothing is looked at; an id that is none is none.
    let server = s.mailbox.message(made[0]).unwrap().server.clone().unwrap();
    assert!(server.thread.is_empty());
}
