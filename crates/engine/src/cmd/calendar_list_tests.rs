// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar's List view (#845): a series is one row, the order and the groups, a free/busy
//! calendar's rows, and the days chosen while it is in view. Every event is invented.

use mailcraft_model::{Calendar, Event, Freq, Id, Recurrence, Rights, ShowAs};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::calendar_list::{Group, first_from, sections};
use crate::{CalView, ListArrange, ListColumn, Session};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const BUSY_CALENDAR: Id = 9_000_000;

fn at(y: i32, m: u8, d: u8, h: u8) -> DateTime {
    DateTime::new(Date::new(y, m, d).unwrap(), h, 0, 0).unwrap()
}

fn weekly() -> Option<Recurrence> {
    Some(Recurrence { freq: Freq::Weekly, interval: 1, ..Default::default() })
}

/// An hour from `start`, in the mailbox's own calendar.
fn event(title: &str, location: &str, start: DateTime, recurrence: Option<Recurrence>, categories: &[&str]) -> Event {
    Event {
        title: title.into(),
        location: location.into(),
        start,
        end: start.add_minutes(60),
        recurrence,
        categories: categories.iter().map(|c| c.to_string()).collect(),
        ..Default::default()
    }
}

/// A session with nothing but `events`, the List view in front.
fn session(events: Vec<Event>) -> Session {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("Ada", "ada@example.org"), NOW, 0);
    s.mailbox.events.clear();
    for e in events {
        s.mailbox.add_event(e).unwrap();
    }
    s.run("view.list", &json!({})).unwrap();
    s
}

/// A calendar somebody shares as free/busy, shown beside the owner's.
fn add_free_busy(s: &mut Session, events: Vec<Event>) {
    s.mailbox.calendars.push(Calendar { id: BUSY_CALENDAR, name: "Kim".into(), visible: true, rights: Rights::FreeBusy, ..Default::default() });
    for e in events {
        s.mailbox.add_event(Event { calendar: BUSY_CALENDAR, ..e }).unwrap();
    }
}

/// The titles in the list's order, each with its group.
fn listed(s: &Session) -> Vec<(String, String)> {
    sections(s).iter().flat_map(|section| section.rows.iter().map(|r| (section.group.name().to_string(), r.subject.clone()))).collect()
}

fn titles(s: &Session) -> Vec<String> {
    listed(s).into_iter().map(|(_, title)| title).collect()
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap()
}

#[test]
fn a_series_is_one_row_under_its_first_start_and_its_rule() {
    let mut series = event("Stand-up", "Room 1", at(2026, 1, 5, 9), weekly(), &[]);
    // Its first day doesn't happen: the row is still where the series starts.
    series.exceptions = vec![Date::new(2026, 1, 5).unwrap()];
    let mut s = session(vec![series, event("Dentist", "", at(2026, 10, 20, 8), None, &[])]);
    let sections = sections(&s);
    assert_eq!(sections.iter().map(|x| x.group.clone()).collect::<Vec<_>>(), [Group::Recurrence(None), Group::Recurrence(Some(Freq::Weekly))]);
    let row = &sections[1].rows[0];
    assert_eq!((row.subject.as_str(), row.start, row.recurrence.as_str()), ("Stand-up", at(2026, 1, 5, 9), "Every week on Monday"));
    // Scripts read it the same way: one row, and the groups with their counts.
    let page = run(&mut s, "calendar.listRows", json!({}));
    assert_eq!(page["total"], 2);
    assert_eq!(page["groups"], json!([{"freq": "none", "name": "(none)", "count": 1}, {"freq": "weekly", "name": "Weekly", "count": 1}]));
    assert_eq!((page["rows"][0]["group"].as_u64(), page["rows"][1]["group"].as_u64()), (Some(0), Some(1)));
    assert_eq!((page["rows"][1]["recurrence"].as_str(), page["rows"][0]["recurrence"].is_null()), (Some("Every week on Monday"), true));
}

#[test]
fn a_heading_sorts_and_a_second_click_turns_it_round_with_empty_cells_last() {
    let mut s = session(vec![
        event("beta", "Hall", at(2026, 10, 1, 9), None, &[]),
        event("Alpha", "", at(2026, 10, 3, 9), None, &[]),
        event("", "annex", at(2026, 10, 2, 9), None, &[]),
    ]);
    run(&mut s, "calendar.listArrange", json!({"by": "none"}));
    // By start, the earliest first: Outlook's own order.
    assert_eq!(titles(&s), ["beta", "", "Alpha"]);
    assert_eq!(run(&mut s, "calendar.listSort", json!({"by": "subject"})), json!({"by": "subject", "descending": false}));
    assert_eq!(titles(&s), ["Alpha", "beta", ""]);
    assert_eq!(run(&mut s, "calendar.listSort", json!({"by": "subject"})), json!({"by": "subject", "descending": true}));
    assert_eq!(titles(&s), ["beta", "Alpha", ""]);
    // Location, both ways: the one without stays last.
    run(&mut s, "calendar.listSort", json!({"by": "location"}));
    assert_eq!(titles(&s), ["", "beta", "Alpha"]);
    run(&mut s, "calendar.listSort", json!({"by": "location", "descending": true}));
    assert_eq!(titles(&s), ["beta", "", "Alpha"]);
    run(&mut s, "calendar.listSort", json!({"by": "end", "descending": true}));
    assert_eq!(titles(&s), ["Alpha", "", "beta"]);
    // Categories don't sort, as in Outlook; nor does anything unknown.
    for by in ["categories", "colour", ""] {
        assert!(s.run("calendar.listSort", &json!({"by": by})).is_err(), "{by}");
    }
    assert_eq!((s.view.list_sort, s.view.list_descending), (ListColumn::End, true));
}

#[test]
fn by_categories_an_event_is_under_each_and_those_without_come_first() {
    let mut s = session(vec![
        event("Review", "", at(2026, 10, 1, 9), None, &["Work", "travel"]),
        event("Lunch", "", at(2026, 10, 2, 9), None, &[]),
        event("Flight", "", at(2026, 10, 3, 9), None, &["Travel"]),
    ]);
    assert_eq!(run(&mut s, "calendar.listArrange", json!({"by": "categories"})), json!({"by": "categories"}));
    let expect = [("No category", "Lunch"), ("travel", "Review"), ("travel", "Flight"), ("Work", "Review")];
    assert_eq!(listed(&s), expect.map(|(g, t)| (g.to_string(), t.to_string())));
    assert_eq!(run(&mut s, "calendar.listRows", json!({}))["total"], 4);
    assert!(s.run("calendar.listArrange", &json!({"by": "location"})).is_err());
    assert_eq!(s.view.list_arrange, ListArrange::Categories);
}

#[test]
fn a_free_busy_calendar_gives_nothing_away_by_its_order_or_groups() {
    // Two of its events that differ only in what the list doesn't show.
    let hidden = |title: &str, location: &str, category: &str, rule: Option<Recurrence>| Event {
        show_as: ShowAs::Busy,
        has_attachments: true,
        ..event(title, location, at(2026, 10, 5, 9), rule, &[category])
    };
    let mut first = session(Vec::new());
    add_free_busy(&mut first, vec![hidden("Aardvark", "Zurich", "Zoo", weekly()), hidden("Zebra", "Aachen", "Ant", None)]);
    let mut second = session(Vec::new());
    add_free_busy(&mut second, vec![hidden("Zebra", "Aachen", "Ant", weekly()), hidden("Aardvark", "Zurich", "Zoo", None)]);
    for arrange in ["recurrence", "categories", "none"] {
        for by in ["subject", "location", "recurrence", "calendar"] {
            let mut rows = Vec::new();
            for s in [&mut first, &mut second] {
                run(s, "calendar.listArrange", json!({"by": arrange}));
                run(s, "calendar.listSort", json!({"by": by, "descending": false}));
                let mut page = run(s, "calendar.listRows", json!({}));
                // (The ids are the only thing told apart, and they say nothing.)
                for row in page["rows"].as_array_mut().unwrap() {
                    row["id"] = json!(0);
                }
                rows.push(page);
            }
            assert_eq!(rows[0], rows[1], "{arrange} by {by}");
        }
    }
    let row = &run(&mut first, "calendar.listRows", json!({}))["rows"][0];
    assert_eq!((row["title"].as_str(), row["location"].as_str(), row["freeBusy"].as_bool()), (Some("Busy"), Some(""), Some(true)));
    assert_eq!((row["categories"].as_array().map(Vec::len), row["recurrence"].is_null(), row["attachment"].as_bool()), (Some(0), true, Some(false)));
}

#[test]
fn rows_come_in_pages() {
    let events = (1..=7).map(|d| event(&format!("Day {d}"), "", at(2026, 10, d, 9), None, &[])).collect();
    let mut s = session(events);
    let page = run(&mut s, "calendar.listRows", json!({"offset": 5, "limit": 10}));
    assert_eq!((page["total"].as_u64(), page["offset"].as_u64(), page["rows"].as_array().map(Vec::len)), (Some(7), Some(5), Some(2)));
    let page = run(&mut s, "calendar.listRows", json!({"offset": u64::MAX, "limit": 0}));
    assert_eq!((page["offset"].as_u64(), page["rows"].as_array().map(Vec::len)), (Some(7), Some(0)));
    assert_eq!(run(&mut s, "calendar.listRows", json!({"limit": 2}))["rows"][1]["title"], "Day 2");
}

#[test]
fn a_chosen_day_stays_in_the_list_and_a_grid_leaves_it() {
    let mut s = session(vec![
        event("Early", "", at(2026, 9, 1, 9), None, &[]),
        event("Later", "", at(2026, 11, 1, 9), weekly(), &[]),
        event("Mid", "", at(2026, 10, 20, 9), None, &[]),
    ]);
    let reveal = s.view.list_reveal;
    run(&mut s, "view.goToDate", json!({"date": "2026-10-15"}));
    // The first row on or after the day, through the groups in order: (none)'s Mid.
    assert_eq!(first_from(&sections(&s), s.view.date), Some(1));
    run(&mut s, "view.goToDate", json!({"date": "2026-10-15"}));
    run(&mut s, "view.today", json!({}));
    run(&mut s, "view.next", json!({}));
    // Every one counts, the same day again too; and the list stays.
    assert_eq!((s.view.calendar, s.view.list_reveal.wrapping_sub(reveal)), (CalView::List, 4));
    assert_eq!(s.view.date, Date::new(2026, 11, 14).unwrap());
    // After the last row starts, the last row.
    assert_eq!(first_from(&sections(&s), Date::new(2030, 1, 1).unwrap()), Some(2));
    assert_eq!(first_from(&[], s.view.date), None);
    for (grid, view) in [("view.day", CalView::Day), ("view.month", CalView::Month), ("view.week", CalView::Week)] {
        run(&mut s, "view.list", json!({}));
        run(&mut s, grid, json!({}));
        assert_eq!(s.view.calendar, view);
    }
    run(&mut s, "view.list", json!({}));
    run(&mut s, "view.days", json!({"count": 3}));
    assert_eq!(s.view.calendar, CalView::Day);
}

#[test]
fn a_row_selects_the_whole_series_and_nothing_asks_for_a_day() {
    let mut s = session(vec![event("Stand-up", "", at(2026, 1, 5, 9), weekly(), &[])]);
    let id = *s.mailbox.events.keys().next().unwrap();
    run(&mut s, "calendar.select", json!({"id": id, "series": true}));
    assert_eq!((s.sel.event, s.sel.event_day), (Some(id), None));
    // As the grid has it, a day.
    run(&mut s, "calendar.select", json!({"id": id}));
    assert_eq!(s.sel.event_day, Date::new(2026, 1, 5));
    // A calendar that isn't shown isn't listed.
    s.mailbox.calendars.iter_mut().for_each(|c| c.visible = false);
    assert!(sections(&s).iter().all(|section| section.rows.is_empty()));
}
