// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Selecting in the message list from the keyboard (#426): Up, Down, Home, End, Page Up and Page
//! Down move the focus, and with Shift (`extend`) the selection runs from its anchor to it.

use mailcraft_model::Id;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::list::Row;
use crate::{Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The sample's Inbox, with nothing selected.
fn inbox() -> (Session, Vec<Id>) {
    let mut s = Session::at(sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    s.sel.messages.clear();
    s.sel.anchor_row = None;
    s.sel.focus_row = None;
    let ids: Vec<Id> = s.rows().iter().map(|r| r.id).collect();
    assert!(ids.len() >= 6, "the sample's Inbox has rows enough: {}", ids.len());
    (s, ids)
}

const SHIFT: fn() -> Value = || json!({"extend": true});

fn sorted(mut v: Vec<Id>) -> Vec<Id> {
    v.sort_unstable();
    v
}

#[test]
fn shift_down_runs_from_the_anchor_and_back_makes_it_smaller() {
    let (mut s, ids) = inbox();
    run(&mut s, "mail.select", json!({"id": ids[1]}));
    run(&mut s, "mail.next", SHIFT());
    run(&mut s, "mail.next", SHIFT());
    assert_eq!(s.sel.messages, ids[1..=3].to_vec(), "the anchor first, the focus last");
    assert_eq!(s.sel.focus(), Some(ids[3]));
    run(&mut s, "mail.previous", SHIFT());
    assert_eq!(s.sel.messages, ids[1..=2].to_vec(), "going back takes the last one out");
    // Past the anchor, the range runs the other way from it.
    run(&mut s, "mail.previous", SHIFT());
    run(&mut s, "mail.previous", SHIFT());
    assert_eq!(sorted(s.sel.messages.clone()), sorted(ids[0..=1].to_vec()));
    assert_eq!(s.sel.focus(), Some(ids[0]));
    assert_eq!(s.sel.anchor_row.map(|(id, _)| id), Some(ids[1]), "the anchor stays");
    // Down without Shift is one message again, and the next range runs from it.
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.messages, vec![ids[1]]);
    run(&mut s, "mail.next", SHIFT());
    assert_eq!(s.sel.messages, ids[1..=2].to_vec());
}

#[test]
fn a_shift_click_range_goes_on_from_the_keyboard() {
    let (mut s, ids) = inbox();
    run(&mut s, "mail.select", json!({"id": ids[2]}));
    run(&mut s, "mail.select", json!({"id": ids[4], "range": true}));
    run(&mut s, "mail.next", SHIFT());
    assert_eq!(s.sel.messages, ids[2..=5].to_vec());
}

#[test]
fn home_and_end_go_to_the_first_and_the_last_row() {
    let (mut s, ids) = inbox();
    let end = ids.len() - 1;
    run(&mut s, "mail.select", json!({"id": ids[2]}));
    run(&mut s, "mail.last", SHIFT());
    assert_eq!(s.sel.messages, ids[2..].to_vec(), "Shift+End: from the anchor to the last");
    run(&mut s, "mail.first", SHIFT());
    assert_eq!(sorted(s.sel.messages.clone()), sorted(ids[..=2].to_vec()), "Shift+Home: from the anchor to the first");
    assert_eq!(s.sel.focus(), Some(ids[0]));
    run(&mut s, "mail.last", json!({}));
    assert_eq!(s.sel.messages, vec![ids[end]]);
    run(&mut s, "mail.first", json!({}));
    assert_eq!(s.sel.messages, vec![ids[0]]);
    // At the ends, a step goes nowhere further.
    run(&mut s, "mail.previous", SHIFT());
    assert_eq!(s.sel.messages, vec![ids[0]]);
    run(&mut s, "mail.last", json!({}));
    run(&mut s, "mail.next", json!({}));
    assert_eq!(s.sel.messages, vec![ids[end]]);
}

#[test]
fn page_keys_move_by_the_rows_a_page_holds() {
    let (mut s, ids) = inbox();
    run(&mut s, "mail.first", json!({}));
    run(&mut s, "mail.pageDown", json!({"rows": 3}));
    assert_eq!(s.sel.messages, vec![ids[3]]);
    run(&mut s, "mail.pageUp", json!({"rows": 2, "extend": true}));
    assert_eq!(sorted(s.sel.messages.clone()), sorted(ids[1..=3].to_vec()));
    assert_eq!(s.sel.focus(), Some(ids[1]));
    run(&mut s, "mail.pageUp", json!({"rows": 2}));
    assert_eq!(s.sel.messages, vec![ids[0]], "a page past the top stops at the first row");
}

#[test]
fn hostile_page_sizes_are_clamped() {
    let (mut s, ids) = inbox();
    let end = ids.len() - 1;
    let at = |s: &Session| ids.iter().position(|id| Some(*id) == s.sel.focus());
    run(&mut s, "mail.first", json!({}));
    run(&mut s, "mail.pageDown", json!({"rows": 0}));
    assert_eq!(at(&s), Some(1), "a page is a row at the least");
    for rows in [json!(18446744073709551615u64), json!(1e300), json!(-5), json!("x"), json!(null), json!([1])] {
        run(&mut s, "mail.first", json!({}));
        run(&mut s, "mail.pageDown", json!({"rows": rows, "extend": "yes"}));
        let i = at(&s).unwrap();
        assert!(i <= end && i > 0, "{rows}: row {i}");
        assert_eq!(s.sel.messages.len(), 1, "{rows}: `extend` that isn't true doesn't extend");
        run(&mut s, "mail.pageUp", json!({"rows": rows}));
        assert!(at(&s).is_some(), "{rows}");
    }
}

#[test]
fn an_empty_list_moves_nothing() {
    let (mut s, _) = inbox();
    run(&mut s, "view.search", json!({"query": "subject:zzzqqqnothingmatches"}));
    assert!(s.rows().is_empty());
    for id in ["mail.next", "mail.previous", "mail.first", "mail.last", "mail.pageDown", "mail.pageUp"] {
        for p in [json!({}), SHIFT(), json!({"rows": 5, "extend": true})] {
            run(&mut s, id, p);
            assert!(s.sel.messages.is_empty(), "{id}");
        }
    }
}

#[test]
fn from_no_focus_down_starts_at_the_top_and_up_at_the_bottom() {
    let (mut s, ids) = inbox();
    run(&mut s, "mail.next", SHIFT());
    assert_eq!(s.sel.messages, vec![ids[0]]);
    s.sel.messages.clear();
    run(&mut s, "mail.previous", SHIFT());
    assert_eq!(s.sel.messages, vec![ids[ids.len() - 1]]);
}

/// The Inbox arranged by categories, with one message (`a`) under two of them, so listed twice.
fn arranged() -> (Session, Id) {
    let (mut s, _) = inbox();
    let every: Vec<Id> = s.mailbox.messages.keys().copied().collect();
    for id in every {
        s.mailbox.message_mut(id).unwrap().categories.clear();
    }
    let ids: Vec<Id> = s.rows().iter().map(|r| r.id).filter(|id| s.mailbox.message(*id).is_some_and(|m| !m.pinned)).take(4).collect();
    let cats: [&[&str]; 4] = [&["Red category", "Blue category"], &["Red category"], &["Blue category"], &["Blue category"]];
    for (k, (id, names)) in ids.iter().zip(cats).enumerate() {
        let m = s.mailbox.message_mut(*id).unwrap();
        m.received = now() + 1000 - k as i64;
        m.categories = names.iter().map(|c| c.to_string()).collect();
    }
    run(&mut s, "view.sort", json!({"by": "categories"}));
    s.sel.messages.clear();
    s.sel.anchor_row = None;
    (s, ids[0])
}

fn row_of(rows: &[Row], id: Id, copy: u16) -> usize {
    rows.iter().position(|r| r.id == id && r.copy == copy).unwrap()
}

#[test]
fn a_message_under_several_categories_is_taken_once_and_the_anchor_keeps_its_row() {
    let (mut s, a) = arranged();
    let rows = s.rows();
    let (first, second) = (row_of(&rows, a, 0), row_of(&rows, a, 1));
    assert!(first < second);
    run(&mut s, "mail.select", json!({"id": a, "copy": 0}));
    for _ in first..second {
        run(&mut s, "mail.next", SHIFT());
    }
    // The focus is a's second row: a is in the selection once, and last, as the focus.
    assert_eq!(s.sel.focus(), Some(a));
    assert_eq!(s.sel.focus_copy(), 1);
    assert_eq!(s.sel.messages.iter().filter(|id| **id == a).count(), 1);
    let mut want: Vec<Id> = rows[first..=second].iter().map(|r| r.id).collect();
    want.sort_unstable();
    want.dedup();
    assert_eq!(sorted(s.sel.messages.clone()), want);
    assert_eq!(s.sel.anchor_row, Some((a, 0)), "the anchor is still a's first row");
    // One row on (when there is one), the range still runs from a's first row.
    if second + 1 < rows.len() {
        run(&mut s, "mail.next", SHIFT());
        let mut want: Vec<Id> = rows[first..=second + 1].iter().map(|r| r.id).collect();
        want.sort_unstable();
        want.dedup();
        assert_eq!(sorted(s.sel.messages.clone()), want);
        assert_eq!(s.sel.messages.first(), Some(&a));
    }
    // And back to the anchor: just a.
    for _ in first..rows.len() {
        run(&mut s, "mail.previous", SHIFT());
        if s.sel.focus_row == Some((a, 0)) {
            break;
        }
    }
    assert_eq!(s.sel.messages, vec![a]);
}
