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
            SortBy::Size => a.list_size().cmp(&b.list_size()).then(when(b).cmp(&when(a))),
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

/// #1511: what is equal in the arrangement's own value stays in date order, newest first, as in
/// classic Outlook: after sorting by From (or Subject, or Size) the mail of one sender, of one
/// subject, of one size is not in some chaotic order. Six synthetic messages of two senders and
/// two subjects with interleaved dates and one size.
#[test]
fn what_is_equal_in_the_arrangement_stays_newest_first() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    s.prefs.show_groups = false;
    s.view.conversations = false;
    let inbox = s.mailbox.inbox().unwrap();
    let ids: Vec<_> = s.mailbox.messages_in(inbox).map(|m| m.id).collect();
    assert!(ids.len() >= 6);
    let from = [s.mailbox.messages.get(&ids[0]).unwrap().from.clone(), s.mailbox.messages.get(&ids[1]).unwrap().from.clone()];
    assert_ne!(from[0].display().to_lowercase(), from[1].display().to_lowercase());
    // Sender and subject alternate while the dates run in a shuffled order, the ids against them.
    let days = [4_i64, 1, 6, 3, 5, 2];
    let mine: Vec<_> = ids.iter().copied().take(6).collect();
    for (i, id) in mine.iter().enumerate() {
        let m = s.mailbox.message_mut(*id).unwrap();
        m.from = from[i % 2].clone();
        m.subject = format!("Subject {}", i % 2);
        (m.date, m.received) = (NOW - days[i] * 86_400, NOW - days[i] * 86_400);
        m.pinned = false;
        m.not_downloaded = 5_000;
    }
    let newest_first = |same: &dyn Fn(usize) -> usize| -> Vec<_> {
        let mut by_date: Vec<usize> = (0..6).collect();
        by_date.sort_by_key(|i| days[*i]);
        let mut keys: Vec<usize> = (0..6).map(same).collect();
        keys.sort_unstable();
        keys.dedup();
        keys.iter().flat_map(|k| by_date.iter().filter(|i| same(**i) == *k).map(|i| mine[*i])).collect()
    };
    let names = [from[0].display().to_lowercase(), from[1].display().to_lowercase()];
    let first_sender = usize::from(names[1] < names[0]);
    for (sort, expected) in
        [("from", newest_first(&|i| usize::from(i % 2 != first_sender))), ("subject", newest_first(&|i| i % 2)), ("size", newest_first(&|_| 0))]
    {
        s.run("view.sort", &json!({"by": sort, "descending": false})).unwrap();
        let listed: Vec<_> = s.rows().iter().map(|r| r.id).filter(|id| mine.contains(id)).collect();
        assert_eq!(listed, expected, "arranged by {sort}: newest first within what is equal");
    }
}
