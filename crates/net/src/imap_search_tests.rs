// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An IMAP search against the in-process server: a message that is not downloaded is still found.

use std::collections::{HashMap, HashSet};

use mailcraft_model::{Account, AccountKind, Security, Server};

use super::search;
use crate::fake::{Fake, FakeMailbox, FakeMessage, ImapWorld};

fn none_known() -> HashMap<String, HashSet<u32>> {
    HashMap::new()
}

#[test]
fn an_older_message_is_found_on_the_server() {
    let body = b"From: Bob <bob@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: Your thermaltake order\r\n\
        Date: Tue, 01 Jul 2003 10:52:37 +0200\r\nMessage-ID: <order@example.org>\r\n\
        MIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe board shipped.\r\n";
    let imap = Fake::imap(ImapWorld {
        user: "ada".into(),
        password: "hunter2".into(),
        mailboxes: vec![FakeMailbox {
            raw: "INBOX".into(),
            flags: "\\HasNoChildren".into(),
            uid_validity: 11,
            messages: vec![FakeMessage {
                uid: 4,
                flags: String::new(),
                internal_date: " 1-Jul-2003 09:52:37 +0000".into(),
                body: body.to_vec(),
                claimed_size: None,
            }],
            refuse_open: false,
            next_uid: 0,
        }],
        ..Default::default()
    });
    let account = Account {
        id: 3,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        username: "ada".into(),
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        ..Default::default()
    };
    let mut pages = Vec::new();
    search(&account, "hunter2", 9, &["INBOX".into()], "thermaltake", &none_known(), false, |page| pages.push(page));
    assert_eq!(pages.len(), 1, "{pages:?}");
    assert!(!pages[0].partial);
    assert!(pages[0].errors.is_empty(), "{:?}", pages[0].errors);
    assert_eq!(pages[0].messages.len(), 1);
    assert_eq!(pages[0].messages[0].server.id, "imap:4");
    assert_eq!(pages[0].messages[0].server.folder, "INBOX");
    assert!(pages[0].messages[0].subject.to_lowercase().contains("thermaltake"));
    pages.clear();
    search(&account, "hunter2", 9, &["INBOX".into()], "from:bob", &none_known(), false, |page| pages.push(page));
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].messages.len(), 1, "from:bob is the sender");
    pages.clear();
    search(&account, "hunter2", 9, &["INBOX".into()], "from:thermaltake", &none_known(), false, |page| pages.push(page));
    assert!(pages[0].errors.is_empty(), "{:?}", pages[0].errors);
    assert!(pages[0].messages.is_empty(), "a word in the subject is not the sender");
    pages.clear();
    search(&account, "hunter2", 9, &["INBOX".into()], "is:unread", &none_known(), false, |page| pages.push(page));
    assert!(pages[0].errors.is_empty(), "a search the server cannot be asked is not an error: {:?}", pages[0].errors);
    assert!(pages[0].messages.is_empty());
}

#[test]
fn a_word_outside_ascii_is_sent_as_utf_8_and_a_known_uid_is_not_counted() {
    let body = |subject: &str, uid: u32| FakeMessage {
        uid,
        flags: String::new(),
        internal_date: " 1-Jul-2003 09:52:37 +0000".into(),
        body: format!("From: Ada <ada@example.org>\r\nSubject: {subject}\r\n\r\n{subject}\r\n").into_bytes(),
        claimed_size: None,
    };
    let imap = Fake::imap(ImapWorld {
        user: "ada".into(),
        password: "hunter2".into(),
        mailboxes: vec![FakeMailbox {
            raw: "INBOX".into(),
            flags: "\\HasNoChildren".into(),
            uid_validity: 11,
            messages: vec![body("älter", 2), body("älter noch", 9)],
            refuse_open: false,
            next_uid: 0,
        }],
        ..Default::default()
    });
    let account = Account {
        id: 3,
        email: "ada@example.org".into(),
        kind: AccountKind::Imap,
        username: "ada".into(),
        incoming: Some(Server { host: "127.0.0.1".into(), port: imap.port, security: Security::None }),
        ..Default::default()
    };
    let mut pages = Vec::new();
    search(&account, "hunter2", 9, &["INBOX".into()], "älter", &none_known(), false, |page| pages.push(page));
    assert!(pages[0].errors.is_empty(), "{:?}", pages[0].errors);
    assert_eq!(pages[0].messages.len(), 2);
    assert_eq!(pages[0].messages[0].server.id, "imap:9", "the newer UID is first");
    let log = imap.world().log.clone();
    let sent = log.iter().any(|line| line.contains("CHARSET") && line.contains("UTF-8"));
    assert!(sent, "the word went with CHARSET UTF-8: {log:?}");
    let mut known = none_known();
    known.insert("INBOX".into(), HashSet::from([9]));
    pages.clear();
    search(&account, "hunter2", 9, &["INBOX".into()], "älter", &known, false, |page| pages.push(page));
    assert_eq!(pages[0].messages.iter().map(|m| m.server.id.as_str()).collect::<Vec<_>>(), ["imap:2"]);
}
