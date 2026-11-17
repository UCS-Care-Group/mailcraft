// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's header as it came (`Message::read_header`, up to 64 KB a message) is kept only for
//! the mail of an account a rule looks for words in the header for (#566): the mailbox doesn't
//! grow with it for any other mail. Mail that came before such a rule is mail the rule can't tell
//! of, as mail downloaded by MailCraft 0.12.0 and earlier.

use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{Address, FolderKind, Id, Message};
use serde_json::{Value, json};

use crate::Session;

/// A message as one read from its bytes keeps its header as it came.
fn received(uid: u32, subject: &str) -> Message {
    Message {
        uid,
        message_id: format!("m{uid}@example.org"),
        subject: subject.into(),
        from: Address::new("Bo", "bo@example.org"),
        date: 100 + i64::from(uid),
        read_header: format!("From: Bo <bo@example.org>\nSubject: {subject}\n"),
        ..Default::default()
    }
}

/// A Send/Receive of `account`'s Inbox that brings `new`.
fn arrives(s: &mut Session, account: Id, seen: u32, new: Vec<Message>) {
    let folder = s.mailbox.folder_of_kind(account, FolderKind::Inbox);
    let delta =
        FolderDelta { id: folder, remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: seen, new, ..Default::default() };
    s.run("sync.apply", &json!({"result": SyncResult { account, folders: vec![delta], ..Default::default() }})).unwrap();
}

fn imap(s: &mut Session, email: &str) -> Id {
    s.run("account.add", &json!({"email": email, "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap()
}

fn kept(s: &Session, subject: &str) -> String {
    s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.read_header.clone()).unwrap()
}

#[test]
fn the_header_as_it_came_is_kept_only_for_an_account_a_rule_reads_it_for() {
    let mut s = Session::empty();
    let ada = imap(&mut s, "ada@mail.example");
    let bo = imap(&mut s, "bo@mail.example");
    // No rule looks for words in the header: nothing of it is kept, in memory or in the file.
    arrives(&mut s, ada, 1, vec![received(1, "Before")]);
    assert_eq!(kept(&s, "Before"), "");
    let saved = serde_json::to_value(&*s.mailbox.messages.values().find(|m| m.subject == "Before").unwrap()).unwrap();
    assert!(saved.get("readHeader").is_none(), "{saved}");

    // A rule of the other account's: still nothing for this one's mail.
    let header = |words: &str| json!({"type": "headerContains", "value": words});
    let rule = |account: Id, conditions: Value, exceptions: Value| json!({"name": "Header", "account": account, "conditions": conditions, "exceptions": exceptions, "actions": [{"type": "markRead"}]});
    s.run("rules.add", &rule(bo, json!([header("x-list")]), json!([]))).unwrap();
    arrives(&mut s, ada, 2, vec![received(2, "Other's rule")]);
    assert_eq!(kept(&s, "Other's rule"), "");

    // A rule of its own, switched off, that looks for them in an exception: kept from now on.
    let made = s.run("rules.add", &rule(ada, json!([{"type": "subjectContains", "value": "Plans"}]), json!([header("x-none")]))).unwrap();
    let id = made["id"].as_u64().unwrap();
    s.run("rules.enable", &json!({"id": id, "value": false})).unwrap();
    arrives(&mut s, ada, 3, vec![received(3, "After")]);
    assert!(kept(&s, "After").contains("Subject: After"));
    // And a rule for every account's mail is one for this account's too.
    let mut s = Session::empty();
    let ada = imap(&mut s, "ada@mail.example");
    s.run("rules.add", &json!({"name": "All", "conditions": [header("x-list")], "actions": [{"type": "markRead"}]})).unwrap();
    s.mailbox.rules[0].account = 0;
    arrives(&mut s, ada, 1, vec![received(1, "Every account's")]);
    assert!(!kept(&s, "Every account's").is_empty());
}

/// Run Rules Now after a rule with words in the header is made: the mail that came before can't
/// be told of, so the exception leaves it alone; the mail that came after is told of, and acted on.
#[test]
fn mail_from_before_a_header_rule_is_left_alone_by_its_exception() {
    let mut s = Session::empty();
    let ada = imap(&mut s, "ada@mail.example");
    let inbox_of = |s: &Session| s.mailbox.folder_of_kind(ada, FolderKind::Inbox).unwrap();
    arrives(&mut s, ada, 1, vec![received(1, "Plans before")]);
    let inbox = inbox_of(&s);
    let filed = s.run("folder.new", &json!({"name": "Filed", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    s.run(
        "rules.add",
        &json!({"name": "File", "account": ada, "conditions": [{"type": "subjectContains", "value": "Plans"}],
            "exceptions": [{"type": "headerContains", "value": "x-none"}], "actions": [{"type": "moveTo", "value": filed}]}),
    )
    .unwrap();
    // (Arriving mail is filed as it comes: this one is left in the Inbox for Run Rules Now.)
    s.mailbox.rules[0].enabled = false;
    arrives(&mut s, ada, 2, vec![received(2, "Plans after")]);
    s.mailbox.rules[0].enabled = true;
    assert!(!kept(&s, "Plans after").is_empty() && kept(&s, "Plans before").is_empty());
    s.run("rules.run", &json!({"folder": inbox})).unwrap();
    let folder = |subject: &str| s.mailbox.messages.values().find(|m| m.subject == subject).unwrap().folder;
    assert_eq!((folder("Plans before"), folder("Plans after")), (inbox, filed));
}
