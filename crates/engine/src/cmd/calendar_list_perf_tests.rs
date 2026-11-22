// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's List view (#845) with a lot of events: arranging by categories and sorting by
//! text take a moment, not seconds, however many categories there are, and the one-pass grouping
//! gives what filtering for each category gave. Every event is invented.

use std::time::{Duration, Instant};

use mailcraft_model::{Event, Mailbox};
use mailcraft_time::{Date, DateTime};
use serde_json::json;

use super::calendar_list::{self, Group, Listing, Row, Section, cached, group, listing, rows, sort};
use crate::{ListArrange, ListColumn, Session};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// Long enough that a slow shared machine passes, short enough that the old per-category filter
/// (13.7 seconds for 3,000 events under 16 categories) fails by a wide margin.
const BOUND: Duration = Duration::from_secs(2);

/// `count` events, each under `per` of `names` distinct categories (a different set each), with
/// titles that sort in no order of their start.
fn session(count: usize, names: usize, per: usize) -> Session {
    let mut s = Session::at(Mailbox::with_local_account("Ada", "ada@example.org"), NOW, 0);
    s.mailbox.events.clear();
    for n in 0..count {
        let day = Date::new(2026, 1, 1).unwrap().add_days(i64::try_from(n % 700).unwrap());
        let start = DateTime::new(day, u8::try_from(n % 24).unwrap(), 0, 0).unwrap();
        let categories = (0..per).map(|k| format!("Category {:02}", (n * 7 + k * 5) % names)).collect();
        // (The case of the first letter varies: "Subject" and "subject" sort together.)
        let title = format!("{}ubject {}", if n % 3 == 0 { "S" } else { "s" }, (n * 7919) % 10_007);
        s.mailbox.add_event(Event { title, start, end: start.add_minutes(30), categories, recurrence: None, ..Default::default() }).unwrap();
    }
    s.run("view.list", &json!({})).unwrap();
    s
}

fn within(what: &str, work: impl FnOnce()) {
    let started = Instant::now();
    work();
    assert!(started.elapsed() < BOUND, "{what} took {:?}", started.elapsed());
}

#[test]
fn arranged_by_categories_thousands_of_events_under_many_categories_take_a_moment() {
    let mut s = session(3_000, 16, 16);
    s.run("calendar.listArrange", &json!({"by": "categories"})).unwrap();
    let mut got = None;
    within("3,000 events under 16 categories each", || got = Some(listing(&s)));
    let got = got.unwrap();
    assert_eq!(got.sections.len(), 16);
    assert!(got.sections.iter().all(|section| section.rows.len() == 3_000), "every event is under every category");
}

#[test]
fn arranged_by_categories_the_most_the_list_holds_takes_a_moment_too() {
    let mut s = session(calendar_list::MAX_EVENTS + 500, 64, 3);
    s.run("calendar.listArrange", &json!({"by": "categories"})).unwrap();
    let mut got = None;
    within("a full list under 64 categories", || got = Some(listing(&s)));
    let got = got.unwrap();
    assert_eq!(got.items, calendar_list::MAX_EVENTS);
    assert_eq!(got.places(), 3 * calendar_list::MAX_EVENTS);
    assert_eq!(got.left_out, 500);
}

/// The most events under the most categories each: every event is held once, however many groups
/// it is in (320,000 places, 20,000 rows), so the memory is not 16 times the rows.
#[test]
fn at_the_caps_each_event_is_held_once_and_not_once_for_each_category() {
    let mut s = session(calendar_list::MAX_EVENTS, 16, 16);
    s.run("calendar.listArrange", &json!({"by": "categories"})).unwrap();
    let mut got = None;
    within("the most events under the most categories", || got = Some(listing(&s)));
    let got: Listing = got.unwrap();
    assert_eq!(got.rows.len(), calendar_list::MAX_EVENTS);
    assert_eq!(got.items, calendar_list::MAX_EVENTS);
    assert_eq!(got.sections.len(), 16);
    assert_eq!(got.places(), 16 * calendar_list::MAX_EVENTS);
    // A group is places, in the order of the rows, each a row that is there.
    for section in &got.sections {
        assert_eq!(section.rows.len(), calendar_list::MAX_EVENTS);
        assert!(section.rows.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(section.rows.last().is_some_and(|last| *last < got.rows.len()));
    }
    // What a group holds is its places, so the whole costs the rows and a number for each place.
    let places = std::mem::size_of::<usize>() * got.places();
    let rows = std::mem::size_of::<Row>() * got.rows.len();
    assert!(places + rows < 16 * 1024 * 1024, "{places} + {rows} bytes of places and rows");
}

/// A page of `calendar.listRows` does not sort and group the whole list again: the session keeps
/// it for as long as the mailbox and the settings are as they were.
#[test]
fn pages_of_rows_come_from_one_listing_kept_while_nothing_changes() {
    let mut s = session(2_000, 8, 2);
    s.run("calendar.listArrange", &json!({"by": "categories"})).unwrap();
    let first = cached(&s);
    for offset in [0, 100, 1_000] {
        let page = s.run("calendar.listRows", &json!({"offset": offset, "limit": 50})).unwrap();
        assert_eq!(page["rows"].as_array().map(Vec::len), Some(50));
        assert_eq!(page["total"], 4_000);
        assert!(std::sync::Arc::ptr_eq(&first, &cached(&s)), "page at {offset}");
    }
    // Another arrangement, or a change to the mailbox, is a new one.
    s.run("calendar.listArrange", &json!({"by": "none"})).unwrap();
    let none = cached(&s);
    assert!(!std::sync::Arc::ptr_eq(&first, &none));
    s.run("calendar.new", &json!({"title": "More", "start": first.rows[0].start.iso(), "end": first.rows[0].end.iso()})).unwrap();
    assert_eq!(cached(&s).items, 2_001);
}

/// More events than the list holds: the list says how many it leaves out, to the script too, and
/// which are left out is the same each time.
#[test]
fn events_past_the_most_the_list_holds_are_counted_and_the_latest_are_the_ones_left_out() {
    let mut s = session(calendar_list::MAX_EVENTS + 5, 4, 1);
    s.run("calendar.listArrange", &json!({"by": "none"})).unwrap();
    let page = s.run("calendar.listRows", &json!({"limit": 1})).unwrap();
    assert_eq!(page["total"], calendar_list::MAX_EVENTS);
    assert_eq!(page["leftOut"], 5);
    let (first, again) = (rows(&s), rows(&s));
    assert_eq!(first, again);
    let kept: std::collections::HashSet<_> = first.iter().map(|r| r.event).collect();
    let latest = s.mailbox.events.values().filter(|e| !kept.contains(&e.id)).map(|e| e.start).min();
    let earliest_kept = first.iter().map(|r| r.start).max();
    assert!(latest >= earliest_kept, "{latest:?} {earliest_kept:?}");
    // With room for all of them there is nothing to say.
    let mut small = session(10, 2, 1);
    assert!(small.run("calendar.listRows", &json!({})).unwrap().get("leftOut").is_none());
}

#[test]
fn sorted_by_text_the_most_the_list_holds_takes_a_moment() {
    let mut s = session(calendar_list::MAX_EVENTS, 8, 1);
    for (column, descending) in [(ListColumn::Subject, false), (ListColumn::Subject, true), (ListColumn::Location, false)] {
        s.run("calendar.listSort", &json!({"by": column, "descending": descending})).unwrap();
        s.run("calendar.listArrange", &json!({"by": "none"})).unwrap();
        let mut got = None;
        within("a full list sorted by text", || got = Some(listing(&s)));
        assert_eq!(got.unwrap().places(), calendar_list::MAX_EVENTS);
    }
}

/// The order a list sorted by Subject came to: A to Z without regard to case, empty last.
#[test]
fn sorted_by_text_the_order_is_by_the_text_without_its_case_then_by_start() {
    let s = session(2_000, 8, 1);
    let mut all = rows(&s);
    sort(&mut all, ListColumn::Subject, false);
    let keys: Vec<String> = all.iter().map(|row| row.subject.to_lowercase()).collect();
    assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]));
    for pair in all.windows(2) {
        if pair[0].subject.to_lowercase() == pair[1].subject.to_lowercase() {
            assert!((pair[0].start, pair[0].event) <= (pair[1].start, pair[1].event));
        }
    }
    sort(&mut all, ListColumn::Subject, true);
    let keys: Vec<String> = all.iter().map(|row| row.subject.to_lowercase()).collect();
    assert!(keys.windows(2).all(|pair| pair[0] >= pair[1]));
}

/// What `group` gave for the categories before it worked in one pass: a filter of every row for
/// each distinct name.
fn by_filtering(rows: &[Row]) -> Vec<Section> {
    let places = |keep: &dyn Fn(&Row) -> bool| -> Vec<usize> { rows.iter().enumerate().filter(|(_, r)| keep(r)).map(|(at, _)| at).collect() };
    let mut names: Vec<&str> = rows.iter().flat_map(|r| r.categories.iter().map(String::as_str)).collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let none = Section { group: Group::Category(None), rows: places(&|r| r.categories.is_empty()) };
    let named = names.iter().map(|name| Section {
        group: Group::Category(Some(name.to_string())),
        rows: places(&|r| r.categories.iter().any(|c| c.eq_ignore_ascii_case(name))),
    });
    std::iter::once(none).chain(named).filter(|section| !section.rows.is_empty()).collect()
}

#[test]
fn one_pass_groups_are_the_groups_a_filter_for_each_category_gives() {
    let mut s = session(600, 12, 3);
    // Some without a category, some under a name in another case.
    let ids: Vec<_> = s.mailbox.events.keys().copied().collect();
    for (n, id) in ids.into_iter().enumerate() {
        let e = s.mailbox.event_mut(id).unwrap();
        match n % 5 {
            0 => e.categories.clear(),
            1 => e.categories.push("CATEGORY 03".into()),
            2 => e.categories.push("category 07".into()),
            _ => {}
        }
    }
    let mut all = rows(&s);
    sort(&mut all, ListColumn::Start, false);
    let new = group(&all, ListArrange::Categories);
    assert_eq!(new, by_filtering(&all));
    assert!(new.iter().any(|section| section.group == Group::Category(None)));
    // A letter that is not ASCII is another category when its case differs (`Row::of` lists names
    // by their ASCII case), each once, though they are met alternately.
    let mut accented = rows(&s);
    for (n, row) in accented.iter_mut().enumerate().take(6) {
        row.categories = vec![(if n % 2 == 0 { "Écoles" } else { "écoles" }).to_string()];
    }
    let names: Vec<String> = group(&accented, ListArrange::Categories)
        .into_iter()
        .filter_map(|section| if let Group::Category(Some(name)) = section.group { Some(name) } else { None })
        .filter(|name| name.to_lowercase() == "écoles")
        .collect();
    assert_eq!(names, ["Écoles", "écoles"]);
    // And by recurrence, as it was.
    assert_eq!(group(&all, ListArrange::Recurrence).len(), 1);
    assert_eq!(group(&all, ListArrange::None).len(), 1);
}

#[test]
fn the_items_are_the_events_not_the_rows_under_their_categories() {
    let mut s = session(100, 4, 2);
    s.run("calendar.listArrange", &json!({"by": "categories"})).unwrap();
    let got = listing(&s);
    assert_eq!(got.items, 100);
    assert_eq!(got.places(), 200);
    assert_eq!(calendar_list::list_settings(&s), (ListColumn::Start, false, ListArrange::Categories));
}
