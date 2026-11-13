// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! All Inboxes lists every account's Inbox, and Mark All as Read covers each of them.

use std::collections::HashSet;

use serde_json::json;

use crate::{Session, sample};

fn session() -> Session {
    let now = mailcraft_time::DateTime::new(mailcraft_time::Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

#[test]
fn every_inbox_is_one_list_and_mark_all_read_covers_them() {
    let mut s = session();
    let first = s.mailbox.inboxes()[0];
    s.run("mail.markAllRead", &json!({"folder": first})).unwrap();
    let one = s.mailbox.messages_in(first).next().unwrap().id;
    s.run("mail.markUnread", &json!({"id": one})).unwrap();

    s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap();
    assert_eq!(s.mailbox.inboxes().len(), 2);
    let second = s.mailbox.inboxes().into_iter().find(|id| *id != first).unwrap();
    let copy = s.run("mail.copy", &json!({"ids": [one], "folder": second})).unwrap()["copies"][0].as_u64().unwrap();
    s.run("mail.markUnread", &json!({"id": copy})).unwrap();

    let answer = s.run("view.allInboxes", &json!({})).unwrap();
    assert_eq!(answer["unread"], 2);
    assert!(s.view.all_inboxes);
    let shown: HashSet<_> = s.rows().iter().map(|r| s.mailbox.message(r.id).unwrap().folder).collect();
    assert!(shown.contains(&first) && shown.contains(&second), "{shown:?}");

    assert_eq!(s.run("mail.markAllRead", &json!({"allInboxes": true})).unwrap()["changed"], 2);
    assert!(s.mailbox.messages_in(first).all(|m| m.read));
    assert!(s.mailbox.messages_in(second).all(|m| m.read));
    assert!(s.run("rules.run", &json!({"allInboxes": true})).unwrap()["matched"].as_u64().is_some());

    s.run("folder.select", &json!({"id": first})).unwrap();
    assert!(!s.view.all_inboxes);
    let back: HashSet<_> = s.rows().iter().map(|r| s.mailbox.message(r.id).unwrap().folder).collect();
    assert_eq!(back, HashSet::from([first]));
}
