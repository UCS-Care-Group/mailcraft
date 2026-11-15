// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's labels of messages (#1153): what a synchronisation brings, the search and what agents
//! see.

use mailcraft_model::sync::{FolderDelta, GmailLabels, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::json;

use crate::Session;

fn gmail_session() -> (Session, Id, Id) {
    let mut s = Session::empty();
    let servers = json!({"email": "ada@mail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}});
    let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    (s, account, inbox)
}

fn message(uid: u32, labels: &[&str]) -> Message {
    Message {
        uid,
        from: Address::new("Rowan", "rowan@sender.example"),
        subject: format!("Message {uid}"),
        message_id: format!("<{uid}@sender.example>"),
        date: 1_700_000_000 + i64::from(uid),
        gmail_labels: labels.iter().map(|l| l.to_string()).collect(),
        ..Default::default()
    }
}

fn delta(inbox: Id, new: Vec<Message>, gmail_labels: Vec<GmailLabels>) -> FolderDelta {
    let uid_seen = new.iter().map(|m| m.uid).max().unwrap_or(2);
    FolderDelta {
        id: Some(inbox),
        remote: "INBOX".into(),
        kind: FolderKind::Inbox,
        uid_validity: 7,
        uid_seen,
        new,
        gmail_labels,
        ..Default::default()
    }
}

fn apply(s: &mut Session, account: Id, folders: Vec<FolderDelta>) {
    s.run("sync.apply", &json!({"result": SyncResult { account, folders, ..Default::default() }})).unwrap();
}

fn labels_of(s: &Session, account: Id, uid: u32) -> Vec<String> {
    s.mailbox.messages.values().find(|m| m.account == account && m.uid == uid).map(|m| m.gmail_labels.clone()).unwrap_or_default()
}

#[test]
fn labels_arrive_cleaned_and_follow_the_server() {
    let (mut s, account, inbox) = gmail_session();
    // A result can say anything: what it says is cleaned again.
    let new = vec![message(1, &["\\Inbox", "Work/Clients", "a\u{202e}\u{0}b", "work/clients"]), message(2, &[])];
    apply(&mut s, account, vec![delta(inbox, new, Vec::new())]);
    assert_eq!(labels_of(&s, account, 1), ["\\Inbox", "Work/Clients", "ab"]);
    assert!(labels_of(&s, account, 2).is_empty());

    // The server says they changed; a UID the mailbox doesn't have is passed over.
    let flood: Vec<String> = (0..500).map(|n| format!("L{n}")).collect();
    let said = vec![
        GmailLabels { uid: 1, labels: vec!["\\Inbox".into(), "Personal".into()] },
        GmailLabels { uid: 2, labels: flood },
        GmailLabels { uid: 99, labels: vec!["Ghost".into()] },
    ];
    apply(&mut s, account, vec![delta(inbox, Vec::new(), said)]);
    assert_eq!(labels_of(&s, account, 1), ["\\Inbox", "Personal"]);
    assert_eq!(labels_of(&s, account, 2).len(), mailcraft_model::gmail::MAX_GMAIL_LABELS);
    assert!(!s.mailbox.messages.values().any(|m| m.gmail_labels.iter().any(|l| l == "Ghost")));

    // Labels taken off on the server are taken off here.
    apply(&mut s, account, vec![delta(inbox, Vec::new(), vec![GmailLabels { uid: 1, labels: Vec::new() }])]);
    assert!(labels_of(&s, account, 1).is_empty());
}

/// The subjects the list in view shows, row by row.
fn listed(s: &Session) -> Vec<String> {
    crate::list::rows(s).iter().filter_map(|r| s.mailbox.message(r.id)).map(|m| m.subject.clone()).collect()
}

#[test]
fn the_search_and_agents_see_labels() {
    let (mut s, account, inbox) = gmail_session();
    apply(&mut s, account, vec![delta(inbox, vec![message(1, &["\\Inbox", "Receipts"]), message(2, &[])], Vec::new())]);
    s.run("folder.select", &json!({"id": inbox})).unwrap();
    s.run("view.search", &json!({"query": "label:receipts"})).unwrap();
    assert_eq!(listed(&s), ["Message 1"]);
    let one = s.mailbox.messages.values().find(|m| m.uid == 1).unwrap();
    assert_eq!(crate::cmd::message_summary(&s, one)["gmailLabels"], json!(["\\Inbox", "Receipts"]));
    let two = s.mailbox.messages.values().find(|m| m.uid == 2).unwrap();
    assert!(crate::cmd::message_summary(&s, two).get("gmailLabels").is_none());
}
