// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The list's sorts, which take what they compare from each message once, put the messages in
//! the order comparing the messages themselves does, for every arrangement both ways round.

use mailcraft_model::{FlagStatus, Importance, Message};
use serde_json::json;

use crate::list::when;
use crate::{Session, SortBy};

const NOW: i64 = 1_791_993_600;

/// The order as comparing two messages at a time gives it, as the list sorted before.
fn reference(shown: &mut [&Message], sort: SortBy, desc: bool) {
    shown.sort_by(|a, b| {
        let key = match sort {
            SortBy::Date => when(a).cmp(&when(b)),
            SortBy::From => a.from.display().to_lowercase().cmp(&b.from.display().to_lowercase()).then(when(b).cmp(&when(a))),
            SortBy::Subject => a.topic().to_lowercase().cmp(&b.topic().to_lowercase()).then(when(b).cmp(&when(a))),
            SortBy::Size => a.list_size().cmp(&b.list_size()),
            SortBy::Importance => a.importance.cmp(&b.importance).then(when(a).cmp(&when(b))),
            SortBy::Categories => when(b).cmp(&when(a)),
        };
        let key = if desc && sort != SortBy::Categories { key.reverse() } else { key };
        b.pinned.cmp(&a.pinned).then(key).then(b.id.cmp(&a.id))
    });
}

#[test]
fn every_arrangement_lists_in_the_order_comparing_messages_gives() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    s.prefs.show_groups = false;
    s.view.conversations = false;
    let inbox = s.mailbox.inbox().unwrap();
    // Ties of every kind: the same date, sender, subject and importance, and pinned mail.
    let ids: Vec<_> = s.mailbox.messages_in(inbox).map(|m| m.id).collect();
    assert!(ids.len() >= 6);
    let first = (**s.mailbox.messages.get(&ids[0]).unwrap()).clone();
    for (i, id) in ids.iter().enumerate().take(6) {
        let m = s.mailbox.message_mut(*id).unwrap();
        if i % 2 == 0 {
            (m.date, m.received, m.from, m.subject) =
                (first.date, first.received, first.from.clone(), format!("RE: {}", first.subject.to_uppercase()));
        }
        m.pinned = i == 3;
        m.importance = if i % 3 == 0 { Importance::High } else { Importance::Normal };
        m.flag = FlagStatus::None;
    }
    for sort in ["date", "from", "subject", "size", "importance"] {
        for descending in [false, true] {
            s.run("view.sort", &json!({"by": sort, "descending": descending})).unwrap();
            let listed: Vec<_> = s.rows().iter().map(|r| r.id).collect();
            let mut shown: Vec<&Message> = s.mailbox.messages_in(inbox).map(|m| m.as_ref()).collect();
            reference(&mut shown, s.view.sort, descending);
            let expected: Vec<_> = shown.iter().map(|m| m.id).collect();
            assert_eq!(listed, expected, "arranged by {sort}, descending {descending}");
        }
    }
}
