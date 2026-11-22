// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Regressions from the independent engine source review, on the same synthetic calendar the
//! other batch tests use: a one-item typed set taken by Delete, the mail question a legacy change
//! asks, Select All's cancelled, hidden and capped items, and a move that needs one queue slot.
//!
//! Synthetic events on `example.org` only, with a fixed clock and zone; no files, servers or
//! accounts with a real server. These are integration tests of the public engine API.

use mailcraft_engine::time::{Date, DateTime};
use mailcraft_engine::{CalView, SelectedEvent, Session, cmd::calendar};
use mailcraft_model::remote::{EventChange, MAX_EVENT_CHANGES};
use mailcraft_model::{Address, Attendee, Calendar, Event, Freq, Origin, Recurrence, Rgb, Rights};
use serde_json::json;

/// Wednesday 2026-10-14 16:00 UTC: the fixed clock the engine's calendar tests use.
const NOW: i64 = 1_791_993_600;

fn session() -> Session {
    Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
}

/// The sample mailbox with nothing in its calendars: the cases here count what they put there.
fn empty_calendar() -> Session {
    let mut s = session();
    s.mailbox.events.clear();
    s
}

fn d(year: i32, month: u8, day: u8) -> Date {
    Date::new(year, month, day).expect("a date that exists")
}

fn at(day: Date, hour: u8, minute: u8) -> DateTime {
    DateTime::new(day, hour, minute, 0).expect("a time of day that exists")
}

fn add_event(s: &mut Session, mut event: Event) -> u64 {
    event.calendar = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    s.mailbox.add_event(event).expect("a local calendar takes an event")
}

/// An event in the calendar `calendar` of the mailbox.
fn add_event_in(s: &mut Session, calendar: u64, mut event: Event) -> u64 {
    event.calendar = calendar;
    s.mailbox.add_event(event).expect("a calendar takes an event")
}

fn attendee() -> Attendee {
    Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Default::default() }
}

/// A second account, so that its id can be named as the server of an event without any server.
fn account(s: &mut Session) -> u64 {
    let added = s
        .run("account.add", &json!({"email": "zoe@example.org", "name": "Zoe", "incoming": {"host": "imap.example.org"}}))
        .expect("an account is added");
    added["id"].as_u64().expect("an account id")
}

/// A calendar that keeps an account's server's events, kept on this computer only here.
fn server_calendar(s: &mut Session, account: u64) -> u64 {
    let id = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar {
        id,
        name: "The account's".into(),
        color: Rgb(0x2E, 0x8B, 0x57),
        visible: true,
        origin: Origin { account, ..Default::default() },
        ..Default::default()
    });
    id
}

/// A calendar the window doesn't show, or one that only says when its owner is busy.
fn calendar_beside(s: &mut Session, name: &str, visible: bool, rights: Rights) -> u64 {
    let id = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar { id, name: name.into(), color: Rgb(0xD1, 0x3B, 0x3B), visible, rights, ..Default::default() });
    id
}

/// A meeting the account's server keeps, with one person invited.
fn kept_meeting(s: &mut Session) -> u64 {
    let account = account(s);
    let start = at(d(2026, 11, 9), 9, 0);
    add_event(
        s,
        Event {
            title: "Planning".into(),
            start,
            end: start.add_minutes(60),
            organizer: Some(Address::new("Zoe Example", "zoe@example.org")),
            attendees: vec![attendee()],
            origin: Origin { account, id: "server-1".into(), ..Default::default() },
            ..Default::default()
        },
    )
}

/// A weekly series whose occurrences the sample calendar shows.
fn weekly_series(s: &mut Session) -> u64 {
    let start = at(d(2026, 11, 4), 9, 0);
    let end = start.add_minutes(30);
    add_event(s, Event { title: "Weekly sync".into(), start, end, recurrence: Some(Recurrence::every(Freq::Weekly)), ..Default::default() })
}

#[test]
fn one_typed_occurrence_is_what_delete_takes_not_its_whole_series() {
    let mut s = session();
    let series = weekly_series(&mut s);
    let day = d(2026, 11, 11);
    s.run("calendar.select", &json!({"items": [{"id": series, "day": day.iso()}]})).expect("one day of the series is selected");
    assert_eq!(s.sel.events, vec![SelectedEvent { id: series, day: Some(day) }], "a typed set of one item");
    let steps = s.undo_steps_moved();

    // A set of one in focus is the older Delete of the focus (benandez on #1665): told that the
    // day is meant, as the window's "Just This One" tells it, it takes that day alone. (Unscoped it
    // is the entire series, which the window asks about first: `agent::delete_form_tests`.)
    let said = s.run("calendar.delete", &json!({"scope": "occurrence"})).expect("Delete takes the one day that is selected");
    let after = s.mailbox.event(series).expect("the series still stands");
    assert_eq!(after.exceptions, vec![day], "only the selected day is cancelled");
    assert_eq!(said["occurrence"], json!(day.iso()));
    assert_eq!(s.undo_steps_moved(), steps + 1, "one undo step");
}

#[test]
fn a_stray_target_key_never_hides_the_mail_a_legacy_change_writes() {
    // `calendar.edit` takes an event by `id`: a caller that also sends `ids` or `items` (a key
    // this command doesn't read) is either refused whole or changes the event and writes to the
    // people invited. It is never described to the owner as writing to nobody.
    for stray in [json!({"ids": [1]}), json!({"ids": []}), json!({"items": [{}]}), json!({"items": "an item"})] {
        let mut s = session();
        let meeting = kept_meeting(&mut s);
        let mut payload = json!({"id": meeting, "start": "2026-11-20T09:00", "send": true});
        for (key, value) in stray.as_object().expect("an object").clone() {
            payload.as_object_mut().expect("an object").insert(key, value);
        }
        let described = calendar::mail_of(&s, "calendar.edit", &payload);
        let refused = s.run("calendar.edit", &payload).is_err();
        assert!(
            refused || described == Some(calendar::Mail::Update(1)),
            "a change that writes to the people invited is asked about: {payload}: {described:?}"
        );
    }
}

#[test]
fn select_all_leaves_out_a_cancelled_appointment() {
    let mut s = empty_calendar();
    let calendar = s.mailbox.default_calendar().expect("a local calendar");
    let day = d(2026, 10, 14);
    let live = add_event_in(&mut s, calendar, Event { title: "Live".into(), start: at(day, 9, 0), end: at(day, 9, 30), ..Default::default() });
    let off_at = at(day, 10, 0);
    let cancelled_event = Event { title: "Called off".into(), start: off_at, end: off_at.add_minutes(30), cancelled: true, ..Default::default() };
    let cancelled = add_event_in(&mut s, calendar, cancelled_event);
    s.view.calendar = CalView::Month;
    s.view.date = day;

    let said = s.run("calendar.selectAll", &json!({})).expect("select all");
    assert_eq!((said["selected"].clone(), said["capped"].clone()), (json!(1), json!(false)));
    assert_eq!(s.sel.events, vec![SelectedEvent { id: live, day: None }], "what the calendar shows, and only that");

    // And what it leaves is a set Delete can take: a cancelled appointment's would be refused.
    s.run("calendar.delete", &json!({})).expect("the selected set is taken");
    assert!(s.mailbox.event(live).is_none(), "the appointment the calendar shows went");
    assert!(s.mailbox.event(cancelled).is_some(), "the cancelled one was never selected, so never deleted");
}

#[test]
fn select_all_keeps_the_earliest_two_hundred_and_says_it_was_capped() {
    let mut s = empty_calendar();
    s.view.calendar = CalView::Month;
    s.view.date = d(2026, 10, 14);
    let calendar = s.mailbox.default_calendar().expect("a local calendar");
    // Three hundred appointments of half past one another, taken from the last to the first so
    // that the order they were made in is not the order the calendar shows them in.
    let first = at(d(2026, 10, 1), 0, 0);
    for n in (0..300i64).rev() {
        let start = first.add_minutes(n.saturating_mul(90));
        add_event_in(&mut s, calendar, Event { title: format!("Slot {n}"), start, end: start.add_minutes(30), ..Default::default() });
    }
    // And what Select All must leave out beside them: a calendar the window doesn't show, one
    // that only says when its owner is busy, and an appointment that was called off.
    let busy_start = at(d(2026, 10, 6), 9, 0);
    let hidden = calendar_beside(&mut s, "Not shown", false, Rights::Edit);
    let busy = calendar_beside(&mut s, "Somebody's busy times", true, Rights::FreeBusy);
    for (beside, title) in [(hidden, "hidden"), (busy, "busy")] {
        for n in 0..5i64 {
            let start = busy_start.add_days(n);
            add_event_in(&mut s, beside, Event { title: format!("{title} {n}"), start, end: start.add_minutes(30), ..Default::default() });
        }
    }
    let off = at(d(2026, 10, 7), 12, 0);
    let off_end = off.add_minutes(30);
    add_event_in(&mut s, calendar, Event { title: "Called off".into(), start: off, end: off_end, cancelled: true, ..Default::default() });

    // What the calendar shows in the range it is in, in the order it lists it: the answer must be
    // the earliest two hundred of these, not the first two hundred met.
    let (from, to) = (d(2026, 10, 1), d(2026, 10, 31));
    let here = |instant: i64| s.offset_at(instant);
    let shown: Vec<u64> = s.mailbox.calendars.iter().filter(|c| c.visible && c.rights != Rights::FreeBusy).map(|c| c.id).collect();
    let mut every: Vec<(DateTime, DateTime, u64, Option<Date>)> = Vec::new();
    for event in s.mailbox.events.values() {
        if event.cancelled || !shown.contains(&event.calendar) {
            continue;
        }
        let repeats = event.recurrence.is_some();
        for occurrence in event.occurrences_in(from, to, &here) {
            every.push((occurrence.start, occurrence.end, occurrence.event, repeats.then_some(occurrence.day)));
        }
    }
    every.sort();
    assert_eq!(every.len(), 300, "fixture: three hundred appointments the calendar shows");
    let want: Vec<SelectedEvent> = every.iter().take(200).map(|(_, _, id, day)| SelectedEvent { id: *id, day: *day }).collect();

    let said = s.run("calendar.selectAll", &json!({})).expect("select all");
    assert_eq!(said["selected"], json!(200), "a selection holds two hundred");
    assert_eq!(said["capped"], json!(true), "and the range holds more than that");
    assert_eq!(s.sel.events, want, "the earliest two hundred, in the order the calendar lists them");
}

#[test]
fn a_same_account_move_fits_in_one_free_queue_slot() {
    let mut s = session();
    let account = account(&mut s);
    let mate = server_calendar(&mut s, account);
    let start = at(d(2026, 11, 9), 9, 0);
    let event = add_event(
        &mut s,
        Event {
            title: "Kickoff".into(),
            start,
            end: start.add_minutes(45),
            origin: Origin { account, id: "server-1".into(), ..Default::default() },
            ..Default::default()
        },
    );
    // One slot left in what waits to go to the server: making the appointment again in another
    // calendar of the same account is one change, so it fits.
    while s.mailbox.event_changes.len().saturating_add(1) < MAX_EVENT_CHANGES {
        s.mailbox.event_changes.push(EventChange::default());
    }

    let made = s
        .run("calendar.paste", &json!({"items": [{"id": event}], "start": "2026-11-20T09:00", "cut": true, "calendar": mate}))
        .expect("one change is what the move is, and one slot is free");
    assert_eq!(made["ids"], json!([event]));
    assert_eq!(s.mailbox.event(event).map(|e| e.calendar), Some(mate), "the appointment is in the account's other calendar");
    assert_eq!(s.mailbox.event_changes.len(), MAX_EVENT_CHANGES, "it took the one free slot, and no more");
}
