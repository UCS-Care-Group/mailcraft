// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message's header as it came (`Message::read_header`, up to 64 KB a message) is kept only for
//! the mail of an account a rule looks for words in the header for (#566): the mailbox doesn't
//! grow with it for any other mail. Mail that came before such a rule is mail the rule can't tell
//! of.

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
    let saved = serde_json::to_value(s.mailbox.messages.values().find(|m| m.subject == "Before").unwrap()).unwrap();
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

/// A Microsoft account's mail, listed and then downloaded whole: its header as it came is kept
/// at the download only while a rule of the account looks for words in the header, with the mark
/// that its lines ran past 64 KB, which no other rule reads.
#[test]
fn a_microsoft_download_keeps_the_header_as_it_came_only_for_a_header_rule() {
    use mailcraft_model::ServerFolder;
    use mailcraft_model::sync::{GraphFetchResult, GraphFetched, GraphMailResult, GraphMessage, MessagePage, ServerMessage};

    use crate::cmd::graph_mail;
    const ME: &str = "ada@example.org";
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    s.mailbox.folder_mut(inbox).unwrap().server = Some(ServerFolder { id: "F-in".into(), caught_up: true, ..Default::default() });
    let listed = |id: &str| GraphMessage {
        server: ServerMessage { id: id.into(), folder: "F-in".into(), size: 1_200, change_key: "CK1".into(), ..Default::default() },
        message_id: format!("{id}@example.org"),
        subject: id.into(),
        from: Address::new("Zoe Park", "zoe@example.org"),
        to: vec![Address::new("Ada", ME)],
        sent: 1_000,
        received: 1_060,
        ..Default::default()
    };
    // A long In-Reply-To takes the lines past 64 KB when `long`.
    let whole = |id: &str, long: bool| {
        let long_line = if long { format!("In-Reply-To: <{}@example.org>\r\n", "r".repeat(70_000)) } else { String::new() };
        GraphFetched {
        id: id.into(),
        mime: format!("From: Zoe Park <zoe@example.org>\r\nTo: Ada <{ME}>\r\nSubject: {id}\r\nMessage-ID: <{id}@example.org>\r\n{long_line}Content-Type: text/plain\r\n\r\nText.\r\n")
            .into_bytes(),
    }
    };
    let comes_as = |s: &mut Session, id: &str, long: bool| {
        let page = MessagePage { folder: "F-in".into(), messages: vec![listed(id)], ..Default::default() };
        graph_mail::apply_graph_mail(s, GraphMailResult { account, email: ME.into(), pages: vec![page], partial: true, ..Default::default() })
            .unwrap();
        graph_mail::apply_graph_fetch(s, GraphFetchResult { account, email: ME.into(), fetched: vec![whole(id, long)], ..Default::default() })
            .unwrap();
    };
    let comes = |s: &mut Session, id: &str| comes_as(s, id, false);
    let marks = |s: &Session, subject: &str| {
        let m = s.mailbox.messages.values().find(|m| m.subject == subject).unwrap();
        (m.read_header_cut, m.headers_cut)
    };
    comes(&mut s, "N1");
    assert_eq!(kept(&s, "N1"), "");
    comes_as(&mut s, "L1", true);
    assert_eq!((kept(&s, "L1"), marks(&s, "L1")), (String::new(), (false, false)));
    s.run(
        "rules.add",
        &json!({"name": "List", "server": false, "conditions": [{"type": "headerContains", "value": "x-list"}], "actions": [{"type": "markRead"}]}),
    )
    .unwrap();
    comes(&mut s, "N2");
    assert!(kept(&s, "N2").contains("Subject: N2"), "{:?}", kept(&s, "N2"));
    assert_eq!(marks(&s, "N2"), (false, false));
    comes_as(&mut s, "L2", true);
    assert!(kept(&s, "L2").contains("Subject: L2") && marks(&s, "L2") == (true, false), "{:?}", marks(&s, "L2"));
}
