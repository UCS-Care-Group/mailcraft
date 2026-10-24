// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message listed under several categories has several rows: what acts on "the row" acts on the
//! one the owner chose (#327).
use mailcraft_model::Id;
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::list::{Row, listed};
use crate::*;

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// The inbox arranged by categories, with four unpinned messages `[a, b, c, d]` newest first:
/// a is under Red and Blue, b under Red, c and d under Blue.
fn arranged() -> (Session, [Id; 4]) {
    let mut s = Session::at(crate::sample::mailbox(now(), 0), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    let every: Vec<Id> = s.mailbox.messages.keys().copied().collect();
    for id in every {
        s.mailbox.message_mut(id).unwrap().categories.clear();
    }
    let ids: Vec<Id> = s.rows().iter().map(|r| r.id).filter(|id| s.mailbox.message(*id).is_some_and(|m| !m.pinned)).take(4).collect();
    let ids = [ids[0], ids[1], ids[2], ids[3]];
    let cats: [&[&str]; 4] = [&["Red category", "Blue category"], &["Red category"], &["Blue category"], &["Blue category"]];
    for (k, (id, names)) in ids.iter().zip(cats).enumerate() {
        let m = s.mailbox.message_mut(*id).unwrap();
        m.received = now() + 1000 - k as i64;
        m.categories = names.iter().map(|c| c.to_string()).collect();
    }
    run(&mut s, "view.sort", json!({"by": "categories"}));
    (s, ids)
}

fn headings(rows: &[Row]) -> Vec<&str> {
    rows.iter().filter_map(|r| r.group.as_deref()).filter(|g| *g != "Pinned").collect()
}

#[test]
fn shift_click_runs_from_the_row_clicked_not_the_first_row_of_the_message() {
    let (mut s, [a, b, ..]) = arranged();
    // a's second row is under Red; b is the row below it.
    run(&mut s, "mail.select", json!({"id": a, "copy": 1}));
    run(&mut s, "mail.select", json!({"id": b, "range": true}));
    let mut picked = s.sel.messages.clone();
    picked.sort_unstable();
    let mut want = vec![a, b];
    want.sort_unstable();
    assert_eq!(picked, want);
    assert_eq!(s.sel.focus(), Some(b));
    // The anchor stays on a range's next extension.
    run(&mut s, "mail.select", json!({"id": b, "range": true}));
    assert_eq!(s.sel.anchor_row, Some((a, 1)));
}

#[test]
fn delete_from_the_second_row_goes_on_from_there() {
    let (mut s, [a, b, ..]) = arranged();
    run(&mut s, "mail.select", json!({"id": a, "copy": 1}));
    run(&mut s, "mail.delete", json!({}));
    assert_eq!(s.sel.messages, vec![b], "the row below the one deleted from");
}

#[test]
fn find_related_counts_a_message_once() {
    let (mut s, [a, ..]) = arranged();
    run(&mut s, "mail.select", json!({"id": a}));
    let r = run(&mut s, "mail.findRelated", json!({}));
    let rows = s.rows();
    assert!(rows.len() > listed(&rows), "a is listed twice");
    assert_eq!(r["found"], listed(&rows));
}

#[test]
fn headings_say_which_are_the_owners_categories() {
    let (mut s, [a, b, c, d]) = arranged();
    // Categories named like the program's own headings.
    for (id, name) in [(a, "Pinned"), (b, "No category"), (c, "Today"), (d, "Today")] {
        s.mailbox.message_mut(id).unwrap().categories = vec![name.into()];
    }
    let rows = s.rows();
    let owners: Vec<&str> = rows.iter().filter(|r| r.category).filter_map(|r| r.group.as_deref()).collect();
    assert_eq!(owners, ["No category", "Pinned", "Today"]);
    // The program's own heading for what has none is not the owner's.
    let none = rows.iter().find(|r| r.group.as_deref() == Some(crate::list::NO_CATEGORY) && !r.category);
    assert!(none.is_some());
    assert!(rows.iter().filter(|r| r.group.as_deref() == Some("Pinned")).any(|r| !r.category), "the pinned heading");
    assert!(rows.iter().filter(|r| r.group.is_some()).all(|r| r.group.as_deref() != Some("Today") || r.category));
}

/// The headings of the four messages each given one list of categories.
fn named(cats: [&[&str]; 4]) -> (Vec<String>, usize) {
    let (mut s, ids) = arranged();
    for (id, names) in ids.iter().zip(cats) {
        s.mailbox.message_mut(*id).unwrap().categories = names.iter().map(|c| c.to_string()).collect();
    }
    // Another session, so that the list is built afresh.
    let mut s = Session::at(s.mailbox.clone(), now(), 0);
    run(&mut s, "folder.select", json!({"kind": "inbox"}));
    run(&mut s, "view.sort", json!({"by": "categories"}));
    let rows = s.rows();
    let first = rows.iter().filter(|r| r.id == ids[0]).count();
    (headings(&rows).into_iter().filter(|h| *h != "No category").map(String::from).collect(), first)
}

#[test]
fn capitals_make_no_second_heading() {
    // Newest first: the first spelling met is the heading's, unless the mailbox's list has one.
    assert_eq!(named([&["Blue"], &["blue"], &["Été"], &["été"]]).0, ["Blue", "Été"]);
    let (headings, rows_of_first) = named([&["BLUE CATEGORY", "blue Category"], &["blue category"], &["Été"], &["été"]]);
    assert_eq!(headings, ["Blue category", "Été"]);
    assert_eq!(rows_of_first, 1, "one category in two spellings");
}
