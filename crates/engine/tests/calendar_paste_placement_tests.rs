// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `calendar.paste`'s placement rules on the public engine API: a mixed batch of timed and
//! whole-day items anchors on its earliest timed item and keeps every other item's own offset; a
//! whole-day batch lands on the requested date and ignores its time of day; and an appointment of
//! a calendar that may only be read is copied out of it, never cut out of it.
//!
//! Synthetic events on `example.org` only, at the fixed clock and offset the engine's other
//! calendar tests use (`Session::at`, `NOW`): every expected date and minute is a literal of this
//! file or a plain `Date`/`DateTime` step, never a second call to the placement code.

use mailcraft_engine::Session;
use mailcraft_engine::time::{Date, DateTime};
use mailcraft_model::{Calendar, Event, Origin, Rgb, Rights};
use serde_json::json;

/// Wednesday 2026-10-14 16:00 UTC: the fixed clock the engine's calendar tests use.
const NOW: i64 = 1_791_993_600;

fn session() -> Session {
    Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
}

fn d(year: i32, month: u8, day: u8) -> Date {
    Date::new(year, month, day).expect("a date that exists")
}

fn at(day: Date, hour: u8, minute: u8) -> DateTime {
    DateTime::new(day, hour, minute, 0).expect("a time of day that exists")
}

/// A whole day `days` long: the midnight that opens its first day, and the one that closes its
/// last (an all-day event's end is exclusive).
fn day_span(first: Date, days: i64) -> (DateTime, DateTime) {
    (DateTime::at_midnight(first), DateTime::at_midnight(first.add_days(days)))
}

fn add_event(s: &mut Session, mut event: Event) -> u64 {
    event.calendar = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    s.mailbox.add_event(event).expect("a local calendar takes an event")
}

fn ids_of(made: &serde_json::Value) -> Vec<u64> {
    made["ids"].as_array().map(|ids| ids.iter().map(|id| id.as_u64().expect("an id")).collect()).unwrap_or_default()
}

/// A mixed batch: two timed items and one whole day, in an order that is not the placement order,
/// with the whole day before the earliest timed item — so the anchor is the earliest timed item,
/// not the first given nor the earliest by date. Timed items keep their wall-clock offsets and
/// lengths, the whole day its day offset and days; no source changes, and one Undo takes all three.
#[test]
fn a_mixed_batch_anchors_on_its_earliest_timed_item_and_keeps_each_offset() {
    let mut s = session();
    let (offsite_start, offsite_end) = day_span(d(2026, 11, 2), 2);
    let offsite = add_event(&mut s, Event { title: "Offsite".into(), start: offsite_start, end: offsite_end, all_day: true, ..Default::default() });
    let kickoff_start = at(d(2026, 11, 5), 9, 0);
    let kickoff =
        add_event(&mut s, Event { title: "Kickoff".into(), start: kickoff_start, end: kickoff_start.add_minutes(90), ..Default::default() });
    let review_start = at(d(2026, 11, 5), 14, 20);
    let review = add_event(&mut s, Event { title: "Review".into(), start: review_start, end: review_start.add_minutes(45), ..Default::default() });
    // Fixture: the whole day is earlier than the earliest timed item.
    assert!(offsite_start.date < kickoff_start.date && kickoff_start < review_start);
    let sources: Vec<(u64, Event)> =
        [review, offsite, kickoff].iter().map(|id| (*id, s.mailbox.event(*id).cloned().expect("a source just added"))).collect();

    let steps = s.undo_steps_moved();
    let made = s
        .run("calendar.paste", &json!({"items": [{"id": review}, {"id": offsite}, {"id": kickoff}], "start": "2026-12-01T13:30"}))
        .expect("a mixed batch of copies is pasted");
    let ids = ids_of(&made);
    assert_eq!(ids.len(), 3, "one id per item, in input order: {made}");
    assert_eq!(made["cut"], json!(false), "a copy is not a cut: {made}");
    assert!(
        ids.iter().all(|id| ![review, offsite, kickoff].contains(id)) && ids[0] != ids[1] && ids[1] != ids[2],
        "every copy has a fresh identity: {made}"
    );

    let copy_review = s.mailbox.event(ids[0]).cloned().expect("the review's copy");
    let copy_offsite = s.mailbox.event(ids[1]).cloned().expect("the offsite's copy");
    let copy_kickoff = s.mailbox.event(ids[2]).cloned().expect("the kickoff's copy");
    // The anchor takes the requested start though it was named last; the other timed item keeps
    // the 5 h 20 min of wall clock between 09:00 and 14:20; the whole day is three days before the
    // anchor's date, so three days before the requested date, and two days long.
    for (copy, from, to, what) in [
        (&copy_kickoff, at(d(2026, 12, 1), 13, 30), at(d(2026, 12, 1), 15, 0), "the anchor lands at the requested start"),
        (&copy_review, at(d(2026, 12, 1), 18, 50), at(d(2026, 12, 1), 19, 35), "the other keeps its wall-clock offset"),
        (&copy_offsite, DateTime::at_midnight(d(2026, 11, 28)), DateTime::at_midnight(d(2026, 11, 30)), "the whole day keeps its day offset"),
    ] {
        assert_eq!((copy.start, copy.end), (from, to), "{what}");
    }
    assert!(copy_offsite.all_day, "and stays a whole day");
    assert!(!copy_review.all_day && !copy_kickoff.all_day, "a timed item stays timed");
    let lengths: Vec<i64> = [&copy_review, &copy_offsite, &copy_kickoff].iter().map(|copy| copy.duration_minutes()).collect();
    assert_eq!(lengths, vec![45, 2 * 24 * 60, 90]);

    for (id, before) in &sources {
        assert_eq!(s.mailbox.event(*id), Some(before), "a source stands exactly as it was");
    }
    assert_eq!(s.undo_steps_moved(), steps + 1, "the whole batch is one undo step");
    assert!(s.undo(), "the paste is one step to undo");
    assert!(ids.iter().all(|id| s.mailbox.event(*id).is_none()), "one undo takes all three copies, leaving none of them behind");
    assert!(sources.iter().all(|(id, before)| s.mailbox.event(*id) == Some(before)), "and leaves every source as it was");
}

/// A whole-day batch only: no timed item to anchor on, so the earliest whole day takes the
/// requested date and each other one keeps its day offset from it, its length in days, and is
/// still a whole day. The requested time takes no part, and one Undo takes the batch away.
#[test]
fn an_all_day_batch_lands_on_the_requested_date_and_keeps_each_day_offset_and_length() {
    let mut s = session();
    let (trip_start, trip_end) = day_span(d(2026, 11, 2), 2);
    let trip = add_event(&mut s, Event { title: "Trip".into(), start: trip_start, end: trip_end, all_day: true, ..Default::default() });
    let (holiday_start, holiday_end) = day_span(d(2026, 11, 6), 1);
    let holiday = add_event(&mut s, Event { title: "Holiday".into(), start: holiday_start, end: holiday_end, all_day: true, ..Default::default() });
    let (conference_start, conference_end) = day_span(d(2026, 11, 9), 3);
    let conference =
        add_event(&mut s, Event { title: "Conference".into(), start: conference_start, end: conference_end, all_day: true, ..Default::default() });
    let sources: Vec<(u64, Event)> =
        [trip, holiday, conference].iter().map(|id| (*id, s.mailbox.event(*id).cloned().expect("a source just added"))).collect();

    let steps = s.undo_steps_moved();
    // 17:45 is deliberately not midnight: a whole day ignores the time of day it is pasted at.
    let made = s
        .run("calendar.paste", &json!({"items": [{"id": holiday}, {"id": conference}, {"id": trip}], "start": "2026-12-10T17:45"}))
        .expect("a batch of whole days is pasted");
    let ids = ids_of(&made);
    assert_eq!(ids.len(), 3, "one id per item, in input order: {made}");
    assert!(ids.iter().all(|id| ![trip, holiday, conference].contains(id)) && ids[0] != ids[1] && ids[1] != ids[2], "fresh identities: {made}");
    let copies: Vec<Event> = ids.iter().map(|id| s.mailbox.event(*id).cloned().expect("a copy")).collect();

    // The anchor is the earliest whole day, named last: 2026-11-02 onto 2026-12-10.
    assert_eq!((copies[2].start, copies[2].end), day_span(d(2026, 12, 10), 2), "the earliest whole day lands on the requested date, two days long");
    // Four days after 2026-11-02, so four after 2026-12-10; and seven, for the last one.
    assert_eq!((copies[0].start, copies[0].end), day_span(d(2026, 12, 14), 1), "the holiday keeps four days of offset and one day of length");
    assert_eq!((copies[1].start, copies[1].end), day_span(d(2026, 12, 17), 3), "the conference keeps seven days of offset and three of length");
    for copy in &copies {
        assert!(copy.all_day, "every copy is still a whole day: {copy:?}");
        assert_eq!((copy.start.hour, copy.start.minute), (0, 0), "a whole day starts at midnight, whatever time was asked for: {copy:?}");
        assert_eq!((copy.end.hour, copy.end.minute), (0, 0), "and ends at one: {copy:?}");
        assert!(copy.recurrence.is_none() && copy.exceptions.is_empty(), "a copy happens once");
        assert!(copy.attendees.is_empty() && copy.organizer.is_none() && copy.uid.is_empty(), "a copy invites nobody");
    }
    assert_eq!(copies.iter().map(|copy| copy.duration_minutes()).collect::<Vec<i64>>(), vec![24 * 60, 3 * 24 * 60, 2 * 24 * 60]);

    for (id, before) in &sources {
        assert_eq!(s.mailbox.event(*id), Some(before), "a source stands exactly as it was");
    }
    assert_eq!(s.undo_steps_moved(), steps + 1, "the whole batch is one undo step");
    assert!(s.undo(), "the paste is one step to undo");
    assert!(ids.iter().all(|id| s.mailbox.event(*id).is_none()), "one undo takes all three copies, leaving none of them behind");
    assert!(sources.iter().all(|(id, before)| s.mailbox.event(*id) == Some(before)), "and leaves every source as it was");
}

/// A calendar shared for reading (full detail, `Rights::Read`) is a source: its appointment copies
/// into a calendar that may be written, as a fresh plain appointment, while the source keeps its
/// own stamp, times and length. The same batch as a cut is refused for the source's rights.
#[test]
fn a_read_only_source_is_copied_into_a_writable_calendar_and_cut_from_it_is_refused() {
    let mut s = session();
    let account = s
        .run("account.add", &json!({"email": "zoe@example.org", "name": "Zoe", "incoming": {"host": "imap.example.org"}}))
        .expect("an account is added")["id"]
        .as_u64()
        .expect("an account id");
    let source = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar {
        id: source,
        name: "Team (read only)".into(),
        color: Rgb(0x2E, 0x8B, 0x57),
        visible: true,
        rights: Rights::Read,
        origin: Origin { account, id: "team".into(), ..Default::default() },
        ..Default::default()
    });
    let destination = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    // Setup guards: full detail but read only, and a destination that is a different calendar that
    // may be written.
    let shared = s.mailbox.calendar(source).expect("the calendar just pushed");
    assert_eq!(shared.rights, Rights::Read, "the source shows its appointments, and may only be read");
    assert_ne!(shared.rights, Rights::FreeBusy, "full detail, not a busy-times block");
    assert_ne!(source, destination, "the copy goes to another calendar");
    assert!(s.mailbox.calendar(destination).is_some_and(|calendar| calendar.can_edit()), "and one that may be written");

    let start = at(d(2026, 11, 12), 10, 0);
    let board = s
        .mailbox
        .add_event(Event {
            calendar: source,
            title: "Board review".into(),
            location: "Room 4".into(),
            description: "Quarterly numbers".into(),
            start,
            end: start.add_minutes(60),
            reminder_minutes: Some(15),
            origin: Origin { account, id: "srv-1".into(), change_key: "ck-1".into(), ..Default::default() },
            ..Default::default()
        })
        .expect("the source calendar takes an event");
    let before = s.mailbox.event(board).cloned().expect("the source appointment");
    let (pending, changes) = (s.mailbox.pending.len(), s.mailbox.event_changes.len());
    let steps = s.undo_steps_moved();

    let made = s
        .run("calendar.paste", &json!({"items": [{"id": board}], "start": "2026-12-07T10:00", "calendar": destination}))
        .expect("reading a calendar is enough to copy out of it");
    let ids = ids_of(&made);
    assert_eq!(ids.len(), 1, "one id for the one item: {made}");
    let copy = s.mailbox.event(ids[0]).cloned().expect("the copy");
    assert_ne!(copy.id, board, "the copy is its own appointment");
    assert_eq!(copy.calendar, destination, "it is in the calendar that was named");
    let words = (copy.title.as_str(), copy.location.as_str(), copy.description.as_str());
    assert_eq!(words, ("Board review", "Room 4", "Quarterly numbers"), "it says what the source says");
    assert_eq!((copy.start, copy.end), (at(d(2026, 12, 7), 10, 0), at(d(2026, 12, 7), 11, 0)), "the requested start, with the source's hour");
    assert_eq!((copy.duration_minutes(), copy.reminder_minutes), (60, Some(15)), "and its length and reminder");
    assert!(copy.origin.is_local() && copy.origin.change_key.is_empty(), "the copy carries no server's version stamp: {:?}", copy.origin);
    assert!(copy.uid.is_empty() && copy.attendees.is_empty() && copy.organizer.is_none() && copy.recurrence.is_none(), "a copy is plain");
    assert_eq!(s.mailbox.event(board), Some(&before), "the source keeps its stamp, times and people");
    assert_eq!((s.mailbox.pending.len(), s.mailbox.event_changes.len()), (pending, changes), "copying into a local calendar queues nothing");
    assert_eq!(s.undo_steps_moved(), steps + 1, "the paste is one undo step");
    assert!(s.undo());
    assert!(s.mailbox.event(ids[0]).is_none() && s.mailbox.event(board) == Some(&before), "one undo takes the copy and leaves the source");

    // The same batch as a cut would take the source away: refused for what the source allows,
    // before the mailbox, the selection or undo/redo move.
    let mailbox = s.mailbox.clone();
    let selection = s.sel.clone();
    let (steps, undo, redo) = (s.undo_steps_moved(), s.undo_label(), s.redo_label());
    let refused = s.run("calendar.paste", &json!({"items": [{"id": board}], "start": "2026-12-08T09:00", "calendar": destination, "cut": true}));
    let error = refused.expect_err("a calendar that may only be read does not let one of its appointments be cut out");
    assert!(error.to_string().contains("can only be read"), "refused for the source's rights: {error}");
    assert!(s.mailbox == mailbox, "a refused cut leaves the mailbox as it was");
    assert!(s.sel == selection, "and the selection as it was");
    assert_eq!((s.undo_steps_moved(), s.undo_label(), s.redo_label()), (steps, undo, redo), "and undo/redo as they were");
    assert_eq!(s.mailbox.event(board).map(|event| event.calendar), Some(source), "and the source still where it was");
}
