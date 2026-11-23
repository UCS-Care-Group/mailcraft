// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Regressions for typed calendar item selection and a selected-set paste.
//!
//! Nothing here is production code: these are public-API integration tests.
//!
//! Fixtures only: synthetic people on `example.org`, `Session::at`/`Session::in_zone` with a
//! fixed clock and zone, and events added through the public `Mailbox::add_event`. No files,
//! servers, real accounts or wall clock.

use mailcraft_engine::time::{Date, DateTime, Zone};
use mailcraft_engine::{SelectedEvent, Session};
use mailcraft_model::{Address, Attendee, Event, Freq, Recurrence};
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

/// Add a synthetic event to the sample mailbox's one local calendar, as the sample itself does.
fn add_event(s: &mut Session, mut event: Event) -> u64 {
    event.calendar = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    s.mailbox.add_event(event).expect("a local calendar takes an event")
}

fn attendee() -> Attendee {
    Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Default::default() }
}

/// A typed selection names the occurrence's intrinsic key (`Occurrence::day`, the day
/// `Event::exceptions` names it by), keeps the legacy focus on the displayed start date of the
/// last item, toggles each distinct identity once under `extend`, and an ordinary event
/// canonicalises to `day: None`.
///
/// The series repeats by the clock of its own zone (Europe/Berlin) while the calendar is shown
/// in America/New_York. Across the weeks when the two zones' daylight saving disagree, an
/// occurrence starts on the day after its series day key, so storing the displayed start date
/// instead of `Occurrence::day` is visible. Both values are taken from `occurrences_in`, the
/// same public model call the engine's own calendar code uses.
#[test]
fn typed_select_toggles_occurrences_by_intrinsic_day_and_keeps_the_legacy_focus() {
    let mut s =
        Session::in_zone(mailcraft_engine::sample::mailbox(NOW, 0), NOW, Zone::named("America/New_York").expect("a zone of the bundled database"));
    // First occurrence: 2026-01-07 05:00 in Berlin, which reads 2026-01-06 23:00 in New York.
    let start = at(d(2026, 1, 6), 23, 0);
    let series = add_event(
        &mut s,
        Event {
            title: "Berlin stand-up".into(),
            start,
            end: start.add_minutes(45),
            start_zone: "Europe/Berlin".into(),
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            ..Default::default()
        },
    );
    let series_before = s.mailbox.event(series).cloned().expect("the event was just added");
    let occurrences = {
        let here = |instant: i64| s.offset_at(instant);
        series_before.occurrences_in(d(2026, 2, 25), d(2026, 3, 31), &here)
    };
    let shifted = occurrences
        .iter()
        .find(|o| o.day != o.start.date)
        .copied()
        .expect("fixture: Europe/Berlin seen from America/New_York crosses the US clock change");
    let plain =
        occurrences.iter().find(|o| o.day == o.start.date && o.day != shifted.day).copied().expect("fixture: an ordinary week of the same series");

    s.run(
        "calendar.select",
        &json!({"items": [
            {"id": series, "day": plain.day.iso()},
            {"id": series, "day": shifted.day.iso()},
        ]}),
    )
    .unwrap_or_else(|error| panic!("calendar.select must take typed items: {error}"));

    assert_eq!(
        s.sel.events,
        vec![SelectedEvent { id: series, day: Some(plain.day) }, SelectedEvent { id: series, day: Some(shifted.day) },],
        "the typed set holds both intrinsic occurrence keys in click order"
    );
    assert_eq!(s.sel.event, Some(series), "the last item is the focus");
    assert_eq!(s.sel.event_day, Some(shifted.start.date), "the legacy focus keeps the displayed start date, not the intrinsic key");
    assert_ne!(shifted.day, shifted.start.date, "fixture: this occurrence's key and displayed date differ");

    // `extend` toggles: an item already in the set goes, the rest stay and keep the focus.
    s.run("calendar.select", &json!({"items": [{"id": series, "day": plain.day.iso()}], "extend": true}))
        .unwrap_or_else(|error| panic!("calendar.select must take extend: {error}"));
    assert_eq!(s.sel.events, vec![SelectedEvent { id: series, day: Some(shifted.day) }], "toggling one occurrence out leaves the other");
    assert_eq!(s.sel.event, Some(series));

    s.run("calendar.select", &json!({"items": [{"id": series, "day": shifted.day.iso()}], "extend": true}))
        .unwrap_or_else(|error| panic!("calendar.select must take extend: {error}"));
    assert!(s.sel.events.is_empty(), "toggling the last item out empties the set");
    assert_eq!((s.sel.event, s.sel.event_day), (None, None), "an empty typed set clears the legacy focus");

    s.run("calendar.select", &json!({"items": [{"id": series, "day": shifted.day.iso()}], "extend": true}))
        .unwrap_or_else(|error| panic!("calendar.select must take extend: {error}"));
    assert_eq!(s.sel.events, vec![SelectedEvent { id: series, day: Some(shifted.day) }], "and toggles it back in");

    // An ordinary event has one identity for all its pieces: day is None.
    let ordinary = add_event(
        &mut s,
        Event { title: "One-off planning".into(), start: at(d(2026, 3, 12), 10, 0), end: at(d(2026, 3, 12), 11, 0), ..Default::default() },
    );
    s.run("calendar.select", &json!({"items": [{"id": ordinary}]})).unwrap_or_else(|error| panic!("calendar.select must take typed items: {error}"));
    assert_eq!(s.sel.events, vec![SelectedEvent { id: ordinary, day: None }], "an ordinary event canonicalises its day to None");
    assert_eq!(s.sel.event, Some(ordinary));

    // Put the series back first: selecting the same ordinary item twice would not test whether
    // a legacy call clears an unrelated typed set.
    s.run("calendar.select", &json!({"items": [{"id": series, "day": shifted.day.iso()}]}))
        .unwrap_or_else(|error| panic!("calendar.select must restore the typed series selection: {error}"));
    assert_eq!(s.sel.events, vec![SelectedEvent { id: series, day: Some(shifted.day) }]);
    // An older single-event selection must not leave the series behind in the typed set.
    s.run("calendar.select", &json!({"id": ordinary})).unwrap_or_else(|error| panic!("calendar.select failed: {error}"));
    assert_eq!(s.sel.event, Some(ordinary));
    assert!(
        s.sel.events.iter().all(|item| item.id == ordinary) && s.sel.events.len() <= 1,
        "a legacy selection may fall back to one item, never keep an unrelated set: {:?}",
        s.sel.events
    );

    // Clearing clears both the typed set and the legacy focus.
    s.run("calendar.select", &json!({"id": null})).unwrap_or_else(|error| panic!("calendar.select failed: {error}"));
    assert!(s.sel.events.is_empty());
    assert_eq!((s.sel.event, s.sel.event_day), (None, None));
}

/// `calendar.paste` copies two timed events to a requested start, in input-item order, preserving
/// their relative minute offsets and durations, leaving the originals as they were, creating
/// fresh identities and no invitations, as one undo step. A refusal after that success leaves the
/// mailbox, the selection and undo/redo exactly as they were.
#[test]
fn paste_copies_two_timed_events_preserving_offsets_and_is_one_undo_step() {
    let mut s = session();
    let first =
        add_event(&mut s, Event { title: "Kickoff".into(), start: at(d(2026, 11, 9), 9, 0), end: at(d(2026, 11, 9), 9, 45), ..Default::default() });
    // A meeting: the copy keeps the existing calendar.copy rule and invites nobody.
    let second = add_event(
        &mut s,
        Event {
            title: "Review".into(),
            start: at(d(2026, 11, 11), 10, 15),
            end: at(d(2026, 11, 11), 11, 0),
            organizer: Some(Address::new("Ada Example", "ada@example.org")),
            attendees: vec![attendee()],
            ..Default::default()
        },
    );
    let first_before = s.mailbox.event(first).cloned().expect("the first source");
    let second_before = s.mailbox.event(second).cloned().expect("the second source");
    let (pending_before, changes_before) = (s.mailbox.pending.len(), s.mailbox.event_changes.len());
    let steps_before = s.undo_steps_moved();

    let made = s
        .run(
            "calendar.paste",
            &json!({
                "items": [{"id": first}, {"id": second}],
                "start": "2026-11-20T15:00",
            }),
        )
        .unwrap_or_else(|error| panic!("calendar.paste is planned but not implemented yet: {error}"));

    let ids: Vec<u64> = made["ids"].as_array().map(|ids| ids.iter().filter_map(|id| id.as_u64()).collect()).unwrap_or_default();
    assert_eq!(ids.len(), 2, "one id per input item: {made}");
    assert_eq!(made["cut"], json!(false), "a copy is not a cut: {made}");
    assert_eq!(s.undo_steps_moved(), steps_before + 1, "the whole batch is one undo step");
    assert!(ids.iter().all(|id| *id != first && *id != second) && ids[0] != ids[1], "every copy has a fresh identity: {made}");

    // Response ids follow input order, so the first names the copy of the first item.
    let copy_first = s.mailbox.event(ids[0]).cloned().expect("the first copy");
    let copy_second = s.mailbox.event(ids[1]).cloned().expect("the second copy");
    assert_eq!(
        (copy_first.title.as_str(), copy_first.start, copy_first.end),
        ("Kickoff", at(d(2026, 11, 20), 15, 0), at(d(2026, 11, 20), 15, 45)),
        "the earliest timed item lands at the requested time on the requested date"
    );
    assert_eq!(
        (copy_second.title.as_str(), copy_second.start, copy_second.end),
        ("Review", at(d(2026, 11, 22), 16, 15), at(d(2026, 11, 22), 17, 0)),
        "the other keeps its day offset from the earliest item and its time of day"
    );
    for copy in [&copy_first, &copy_second] {
        assert!(!copy.all_day && copy.duration_minutes() == 45, "timed and as long as its source");
        assert!(copy.recurrence.is_none() && copy.exceptions.is_empty(), "a copy happens once and belongs to no series");
        assert!(copy.attendees.is_empty() && copy.organizer.is_none() && copy.uid.is_empty(), "a copy is its own appointment and invites nobody");
    }

    // The sources are untouched, and nothing is queued to a server or to be sent.
    assert_eq!(s.mailbox.event(first), Some(&first_before), "the original stands as it was");
    assert_eq!(s.mailbox.event(second), Some(&second_before), "so does the meeting");
    assert_eq!(s.mailbox.pending.len(), pending_before, "pasting sends no mail");
    assert_eq!(s.mailbox.event_changes.len(), changes_before, "and queues no server change of its own");

    // One undo takes the whole batch away; one redo puts it back, identities included.
    assert!(s.undo(), "the paste is one step to undo");
    assert!(s.mailbox.event(ids[0]).is_none() && s.mailbox.event(ids[1]).is_none(), "one undo takes both copies");
    assert_eq!(s.mailbox.event(first), Some(&first_before));
    assert_eq!(s.mailbox.event(second), Some(&second_before));
    assert!(s.redo(), "and one step to redo");
    assert_eq!(s.mailbox.event(ids[0]), Some(&copy_first), "redo brings the first copy back");
    assert_eq!(s.mailbox.event(ids[1]), Some(&copy_second), "and the second");

    // Only after a paste has actually succeeded: a duplicate identity is refused as a whole.
    let mailbox_before = s.mailbox.clone();
    let selection_before = s.sel.clone();
    let (steps_before, undo_before, redo_before) = (s.undo_steps_moved(), s.undo_label(), s.redo_label());
    let refused = s.run(
        "calendar.paste",
        &json!({
            "items": [{"id": first}, {"id": first}],
            "start": "2026-12-01T09:00",
        }),
    );
    assert!(refused.is_err(), "a duplicate item is refused: {refused:?}");
    assert!(s.mailbox == mailbox_before, "a refused batch leaves the mailbox byte-identical");
    assert!(s.sel == selection_before, "and the selection as it was");
    assert_eq!((s.undo_steps_moved(), s.undo_label(), s.redo_label()), (steps_before, undo_before, redo_before), "and undo/redo as they were");
}

/// Two occurrences of one series paste as their own starts, not the series' master start, and the
/// series is not rewritten: no exceptions, no attendees added, no server change queued.
#[test]
fn pasting_two_occurrences_uses_their_own_starts_without_rewriting_the_series() {
    let mut s = session();
    // Wednesday 2026-11-04 09:00–09:30, weekly.
    let start = at(d(2026, 11, 4), 9, 0);
    let series = add_event(
        &mut s,
        Event {
            title: "Weekly sync".into(),
            start,
            end: start.add_minutes(30),
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            attendees: vec![attendee()],
            ..Default::default()
        },
    );
    let series_before = s.mailbox.event(series).cloned().expect("the series");
    let occurrences = series_before.occurrences_in(d(2026, 11, 1), d(2026, 11, 30), &|_instant: i64| 0i32);
    // Neither is the master's own start date, so a paste that resolved the series' start instead
    // of each occurrence would put both copies on one date.
    let later = occurrences.iter().find(|o| o.day == d(2026, 11, 11)).copied().expect("fixture: 2026-11-11 happens");
    let last = occurrences.iter().find(|o| o.day == d(2026, 11, 18)).copied().expect("fixture: 2026-11-18 happens");
    let (pending_before, changes_before) = (s.mailbox.pending.len(), s.mailbox.event_changes.len());

    let made = s
        .run(
            "calendar.paste",
            &json!({
                "items": [
                    {"id": series, "day": later.day.iso()},
                    {"id": series, "day": last.day.iso()},
                ],
                "start": "2026-12-16T14:00",
            }),
        )
        .unwrap_or_else(|error| panic!("calendar.paste is planned but not implemented yet: {error}"));

    let ids: Vec<u64> = made["ids"].as_array().map(|ids| ids.iter().filter_map(|id| id.as_u64()).collect()).unwrap_or_default();
    assert_eq!(ids.len(), 2, "one id per occurrence: {made}");
    assert_eq!(made["cut"], json!(false));
    let copy_later = s.mailbox.event(ids[0]).cloned().expect("the copy of the first occurrence");
    let copy_last = s.mailbox.event(ids[1]).cloned().expect("the copy of the second occurrence");
    assert_eq!(
        (copy_later.start, copy_later.end),
        (at(d(2026, 12, 16), 14, 0), at(d(2026, 12, 16), 14, 30)),
        "the earlier occurrence lands at the requested time with its own length"
    );
    assert_eq!(
        (copy_last.start, copy_last.end),
        (at(d(2026, 12, 23), 14, 0), at(d(2026, 12, 23), 14, 30)),
        "the other is a week after it, as the two occurrences are, not on the master's start"
    );
    for copy in [&copy_later, &copy_last] {
        assert!(copy.recurrence.is_none() && copy.exceptions.is_empty(), "each copy happens once");
        assert!(copy.attendees.is_empty() && copy.organizer.is_none() && copy.uid.is_empty(), "copying an occurrence invites nobody");
        assert!(copy.id != series, "and is its own appointment");
    }

    // The series is not rewritten, and nothing goes to a server or the outbox.
    assert_eq!(s.mailbox.event(series), Some(&series_before), "the series keeps its rule, days and people");
    assert_eq!(s.mailbox.pending.len(), pending_before, "no invitation or update is sent");
    assert_eq!(s.mailbox.event_changes.len(), changes_before, "and no server change is queued");
}
