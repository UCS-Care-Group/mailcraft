// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Shift+click from an anchor the list shows takes the span of the list's rows without asking
//! of each message whether the list shows it: what it selects, picks as conversations, focuses
//! and keeps in the list is what the click worked out by asking.

use std::collections::HashSet;

use mailcraft_model::Id;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"));
}

/// The Inbox, arranged as `how` says.
fn session(how: &[(&str, Value)]) -> Session {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    for (id, params) in how {
        run(&mut s, id, params.clone());
    }
    s
}

/// What a selection comes to: the messages, those picked as their conversation, the focus's row,
/// and what stays in the list.
fn state(s: &Session) -> (Vec<Id>, Vec<Id>, Option<(Id, u16)>, HashSet<Id>) {
    let kept = s.kept().map(|k| k.into_owned()).unwrap_or_default();
    (s.sel.messages.clone(), s.sel.whole.clone(), s.sel.focus_row, kept)
}

#[test]
fn a_range_from_a_listed_anchor_is_what_asking_of_each_message_selects() {
    let arrangements: Vec<Vec<(&str, Value)>> = vec![
        vec![("view.conversations", json!({"value": false}))],
        vec![("view.conversations", json!({"value": true}))],
        vec![("view.sort", json!({"by": "categories"}))],
    ];
    for how in arrangements {
        let mut probe = session(&how);
        let rows: Vec<(Id, u16)> = probe.rows().iter().map(|r| (r.id, r.copy)).collect();
        assert!(rows.len() >= 4, "{how:?}");
        let last = rows.len() - 1;
        for (from, to) in [(0, last), (last, 0), (1, 2), (2, 2), (0, 1)] {
            let (anchor, to) = (rows[from], rows[to]);
            // The shortcut: the anchor clicked, then Shift and the other row.
            let mut quick = session(&how);
            run(&mut quick, "mail.select", json!({"id": anchor.0, "copy": anchor.1}));
            run(&mut quick, "mail.select", json!({"id": to.0, "copy": to.1, "range": true}));
            let got = state(&quick);
            assert_eq!(got.0.first(), Some(&anchor.0), "{how:?}: the anchor first");
            assert_eq!(got.0.last(), Some(&to.0), "{how:?}: the focus last");
            assert_eq!(got.2, Some(to), "{how:?}: the focus on the row clicked");
            assert_eq!(quick.sel.anchor_row, Some(anchor), "{how:?}: the range keeps its anchor");
            let listed: HashSet<Id> = quick.rows().iter().map(|r| r.id).collect();
            assert!(got.0.iter().all(|id| listed.contains(id)), "{how:?}: all of it listed");
            assert_eq!(got.3, got.0.iter().copied().collect(), "{how:?}: and all of it kept in the list");
            // Asking of each message: the same click, with no anchor noted (the first message
            // selected is the anchor then), where that picks the same row.
            if anchor.1 == 0 {
                let mut asked = session(&how);
                run(&mut asked, "mail.select", json!({"id": anchor.0, "copy": anchor.1}));
                asked.sel.anchor_row = None;
                run(&mut asked, "mail.select", json!({"id": to.0, "copy": to.1, "range": true}));
                assert_eq!(state(&asked), got, "{how:?}: rows {from} to {to:?}");
            }
        }
    }
}
