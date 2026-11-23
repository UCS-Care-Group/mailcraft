// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Hostile target naming and recurring cut placement found during integration review.

use mailcraft_engine::{Session, cmd::calendar::selected_items};
use mailcraft_model::{Event, Freq, Recurrence};
use mailcraft_time::{Date, DateTime, Zone};
use serde_json::{Value, json};

const NOW: i64 = 1_791_993_600;

fn date(month: u8, day: u8) -> Date {
    Date::new(2026, month, day).unwrap()
}

fn at(day: Date, hour: u8) -> DateTime {
    DateTime::new(day, hour, 0, 0).unwrap()
}

fn session() -> (Session, u64) {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let calendar = s.mailbox.default_calendar().unwrap();
    let start = at(date(11, 4), 9);
    let id = s
        .mailbox
        .add_event(Event { calendar, title: "Synthetic edge case".into(), start, end: start.add_minutes(30), ..Default::default() })
        .unwrap();
    (s, id)
}

#[test]
fn fractional_item_ids_never_select_the_truncated_valid_event() {
    let (mut s, id) = session();
    let before = s.sel.clone();
    let result = s.run("calendar.select", &json!({"items": [{"id": id as f64 + 0.5}]}));
    assert!(result.is_err(), "a fractional identity must not silently select event {id}: {result:?}");
    assert_eq!(s.sel, before);
}

#[test]
fn a_null_legacy_target_never_combines_with_explicit_items() {
    let (mut s, id) = session();
    let before = s.mailbox.clone();
    for command in ["calendar.select", "calendar.paste"] {
        let result = s.run(command, &json!({"id": null, "items": [{"id": id}], "start": "2026-12-02T09:00"}));
        assert!(result.is_err(), "mixed target forms must refuse, including a null id: {command}: {result:?}");
        assert!(s.mailbox == before);
    }
}

#[test]
fn malformed_explicit_destination_never_falls_back_to_the_source_calendar() {
    let (mut s, id) = session();
    let before = s.mailbox.clone();
    for destination in [Value::Null, json!(0), json!("somewhere"), json!(-1)] {
        let result = s.run("calendar.paste", &json!({"items": [{"id": id}], "start": "2026-12-02T09:00", "calendar": destination}));
        assert!(result.is_err(), "an explicit malformed calendar must refuse, not create in the source's calendar: {result:?}");
        assert!(s.mailbox == before);
    }
}

#[test]
fn the_shared_target_reader_refuses_an_explicit_null_id_instead_of_the_focus() {
    let (mut s, id) = session();
    s.run("calendar.select", &json!({"id": id})).unwrap();
    assert!(selected_items(&s, &json!({"id": null})).is_err());
}

#[test]
fn cutting_a_zoned_occurrence_lands_at_the_requested_time_across_dst() {
    let mut s = Session::in_zone(mailcraft_engine::sample::mailbox(NOW, 0), NOW, Zone::named("America/New_York").unwrap());
    let calendar = s.mailbox.default_calendar().unwrap();
    let start = DateTime::new(date(1, 6), 23, 0, 0).unwrap();
    let id = s
        .mailbox
        .add_event(Event {
            calendar,
            title: "Berlin occurrence cut".into(),
            start,
            end: start.add_minutes(30),
            start_zone: "Europe/Berlin".into(),
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            ..Default::default()
        })
        .unwrap();
    let source = s.mailbox.event(id).cloned().unwrap();
    let here = |instant| s.offset_at(instant);
    let shifted = source.occurrences_in(date(3, 1), date(3, 31), &here).into_iter().find(|o| o.day != o.start.date).unwrap();
    let target = at(date(3, 3), 15);
    let made = s.run("calendar.paste", &json!({"items": [{"id": id, "day": shifted.day.iso()}], "start": target.iso(), "cut": true})).unwrap();
    let moved = s.mailbox.event(made["ids"][0].as_u64().unwrap()).unwrap();
    assert_eq!((moved.start, moved.end), (target, target.add_minutes(30)), "master-time arithmetic must not shift the chosen occurrence by an hour");
    assert_eq!(s.mailbox.event(id).unwrap().exceptions, vec![shifted.day]);
}

#[test]
fn two_cuts_of_one_series_cannot_overflow_its_cancelled_day_limit() {
    let (mut s, id) = session();
    let first = Date::new(1990, 1, 1).unwrap();
    let event = s.mailbox.event_mut(id).unwrap();
    event.recurrence = Some(Recurrence::every(Freq::Weekly));
    event.exceptions = (0..4999).map(|offset| first.add_days(offset)).collect();
    let before = s.mailbox.clone();
    let result = s.run(
        "calendar.paste",
        &json!({"items": [{"id": id, "day": "2026-11-11"}, {"id": id, "day": "2026-11-18"}], "start": "2026-12-02T09:00", "cut": true}),
    );
    assert!(result.is_err(), "both cuts need exception slots in the same series: {result:?}");
    assert!(s.mailbox == before, "no partial move or over-cap series is left behind");
}
