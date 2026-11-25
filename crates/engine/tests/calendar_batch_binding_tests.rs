// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The parts of a calendar batch's agent guard that the staged `calendar_batch_agent` regressions
//! leave open: the flags are read as written, a batch is classed by its own items and never by the
//! window's focus, and the owner's yes is pinned to the items *and* the calendar they would go to.
//!
//! Fixtures only: synthetic people on `example.org`, the engine calendar tests' fixed clock, and
//! events added through the public `Mailbox::add_event`. No files, servers or wall clock.

use mailcraft_engine::Session;
use mailcraft_engine::agent::{self, Access, Class, Verdict};
use mailcraft_engine::model::{Address, Attendee, Calendar, Event, Freq, Id, Origin, Recurrence};
use mailcraft_engine::time::{Date, DateTime};
use serde_json::{Value, json};

/// Wednesday 2026-10-14 16:00 UTC: the fixed clock the engine's calendar tests use.
const NOW: i64 = 1_791_993_600;
/// Where a paste is asked to put its items.
const START: &str = "2026-12-01T09:00";

fn d(year: i32, month: u8, day: u8) -> Date {
    Date::new(year, month, day).expect("a date that exists")
}

fn at(day: Date, hour: u8, minute: u8) -> DateTime {
    DateTime::new(day, hour, minute, 0).expect("a time of day that exists")
}

/// The appointments a batch is made of, and the ones around them.
struct Fixture {
    s: Session,
    /// Wednesday 2026-11-04 09:00–09:30, weekly.
    series: Id,
    /// A meeting of its own: Alice is invited.
    kickoff: Id,
    /// Another meeting: Bo is invited.
    review: Id,
    /// What the owner may have focused instead: no batch here names it.
    unrelated: Id,
    /// A calendar of another account, kept on that account's server.
    remote: Id,
}

fn add(s: &mut Session, title: &str, start: DateTime, minutes: i64, who: &[(&str, &str)]) -> Id {
    let calendar = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    let attendees: Vec<Attendee> =
        who.iter().map(|(name, address)| Attendee { address: Address::new(*name, *address), ..Default::default() }).collect();
    let event = Event { title: title.to_string(), start, end: start.add_minutes(minutes), calendar, attendees, ..Default::default() };
    s.mailbox.add_event(event).expect("a local calendar takes an event")
}

fn fixture() -> Fixture {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let calendar = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    let start = at(d(2026, 11, 4), 9, 0);
    let series = s
        .mailbox
        .add_event(Event {
            title: "Weekly sync".into(),
            start,
            end: start.add_minutes(30),
            calendar,
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            ..Default::default()
        })
        .expect("a local calendar takes a series");
    let kickoff = add(&mut s, "Kickoff", at(d(2026, 11, 9), 9, 0), 45, &[("Alice Example", "alice@example.org")]);
    let review = add(&mut s, "Review", at(d(2026, 11, 11), 10, 15), 45, &[("Bo Example", "bo@example.org")]);
    let unrelated = add(&mut s, "Unrelated dental", at(d(2026, 11, 10), 14, 0), 30, &[("Eve Example", "eve@example.org")]);
    // A second account, and a calendar its server keeps.
    let mut second = s.mailbox.accounts[0].clone();
    second.id = second.id.saturating_add(1_000);
    second.email = "second@example.org".into();
    second.name = "second@example.org".into();
    let account = second.id;
    s.mailbox.accounts.push(second);
    s.mailbox.repair();
    let id = s.mailbox.calendars.iter().map(|c| c.id).max().unwrap_or(0).saturating_add(1);
    let origin = Origin { account, id: "c-second".into(), ..Default::default() };
    s.mailbox.calendars.push(Calendar { id, name: "Second account".into(), visible: true, origin, ..Default::default() });
    Fixture { s, series, kickoff, review, unrelated, remote: id }
}

/// A flag that isn't `true` or `false` is refused, never read as `false`: writing a flag wrong
/// must not make the owner's question smaller than what the command would do.
#[test]
fn a_flag_that_isnt_true_or_false_refuses_a_batch_whatever_it_was_granted() {
    let f = fixture();
    let s = &f.s;
    for bad in [
        json!({"items": [{"id": f.kickoff}], "start": START, "cut": "yes"}),
        json!({"items": [{"id": f.kickoff}], "start": START, "cut": 1}),
        json!({"items": [{"id": f.kickoff}], "start": START, "send": "yes"}),
        json!({"items": [{"id": f.kickoff}], "start": START, "send": null}),
    ] {
        assert!(matches!(agent::check(s, "calendar.paste", &bad, &[]), Verdict::Refuse(_)), "`calendar.paste` with {bad} must be refused");
        assert!(
            matches!(agent::check(s, "calendar.paste", &bad, &Class::ALL), Verdict::Refuse(_)),
            "no grant makes a flag that isn't true or false mean false: {bad}"
        );
        assert_ne!(agent::classify(s, "calendar.paste", &bad), Access::Safe, "and it is never classed harmless: {bad}");
    }
    for bad in [json!({"items": [{"id": f.kickoff}], "send": "yes"}), json!({"items": [{"id": f.kickoff}], "decline": "no"})] {
        assert!(
            matches!(agent::check(s, "calendar.delete", &bad, &Class::ALL), Verdict::Refuse(_)),
            "`calendar.delete` with {bad} must be refused, never read as a plain delete"
        );
    }
}

/// A batch is classed by the items it names, never by the appointment the owner has focused: one
/// that doesn't read as a batch has no class that lets it run, and a cut of a series' occurrence
/// is asked about by the day the series names.
#[test]
fn a_batch_is_classed_by_its_own_items_never_by_the_focus() {
    let mut f = fixture();
    f.s.run("calendar.select", &json!({"id": f.kickoff})).expect("the owner has the first appointment selected");
    let s = &f.s;
    for bad in [json!({"ids": [f.kickoff], "start": START}), json!({"start": START}), json!({}), json!({"items": [{"id": 999_999}], "start": START})]
    {
        assert!(matches!(agent::check(s, "calendar.paste", &bad, &Class::ALL), Verdict::Refuse(_)), "`calendar.paste` with {bad} must be refused");
        assert_ne!(agent::classify(s, "calendar.paste", &bad), Access::Safe, "{bad}");
    }
    // Declining on the owner's behalf is a delete whichever item is named — never the focus's.
    assert_eq!(agent::classify(s, "calendar.delete", &json!({"items": [{"id": f.unrelated}], "decline": true})), Access::Owner(Class::Delete));
    // A cut is a move, and the question says so, naming the occurrence it takes.
    let cut = json!({"items": [{"id": f.kickoff}, {"id": f.review}], "start": START, "cut": true});
    let said = agent::describe(s, "calendar.paste", &cut);
    assert!(said.starts_with("move ") && said.contains("out of their calendars"), "{said}");
    let copy = agent::describe(s, "calendar.paste", &json!({"items": [{"id": f.kickoff}], "start": START}));
    assert!(copy.starts_with("copy "), "{copy}");
    let day = json!({"items": [{"id": f.series, "day": "2026-11-11"}], "start": START, "cut": true});
    assert_eq!(agent::classify(s, "calendar.paste", &day), Access::Owner(Class::Delete));
    let said = agent::describe(s, "calendar.paste", &day);
    assert!(said.contains("Weekly sync") && said.contains("2026-11-11"), "{said}");
}

/// The owner's yes reaches the items the question showed and the calendar it showed them going to:
/// a destination calendar that went refuses the whole batch.
#[test]
fn an_approval_is_pinned_to_the_destination_calendar_too() {
    let mut f = fixture();
    let remote = f.remote;
    let there: Value = json!({"items": [{"id": f.kickoff}], "start": START, "calendar": remote, "cut": true});
    let what = agent::describe(&f.s, "calendar.paste", &there);
    let shown = agent::bind::Shown::new(&f.s, "calendar.paste", &there, &what).expect("the paste names its item and where it goes");
    // The calendar it would go to goes while the owner is being asked.
    f.s.mailbox.calendars.retain(|c| c.id != remote);
    assert!(shown.check(&f.s, "calendar.paste", &there).is_err(), "a destination calendar that went must be shown again, not silently approved");
}

/// A delete of a batch is pinned as tightly as a paste: a member that changed, and one that went,
/// refuse the whole batch.
#[test]
fn an_approval_for_a_delete_refuses_a_changed_or_missing_member() {
    let mut f = fixture();
    let batch = json!({"items": [{"id": f.kickoff}, {"id": f.review}]});
    let what = agent::describe(&f.s, "calendar.delete", &batch);
    let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &batch, &what).expect("a delete is about the appointments it names");
    let event = f.s.mailbox.event_mut(f.review).expect("the review is there");
    event.title = "Review (moved)".into();
    assert!(shown.check(&f.s, "calendar.delete", &batch).is_err(), "an item retitled while the owner was asked must be shown again");

    let mut f = fixture();
    let batch = json!({"items": [{"id": f.kickoff}, {"id": f.review}]});
    let what = agent::describe(&f.s, "calendar.delete", &batch);
    let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &batch, &what).expect("a delete is about the appointments it names");
    f.s.mailbox.remove_event(f.kickoff).expect("the appointment is there");
    assert!(shown.check(&f.s, "calendar.delete", &batch).is_err(), "one of two gone refuses the whole batch, never a smaller one");
}

/// A delete that names no batch keeps its one-item meaning — the owner's selection — and the yes
/// to it is bound to that, as it always was: changing the selection refuses it.
#[test]
fn a_delete_that_names_no_batch_is_still_bound_to_the_selection() {
    let mut f = fixture();
    f.s.run("calendar.select", &json!({"id": f.kickoff})).expect("the owner has an appointment selected");
    let what = agent::describe(&f.s, "calendar.delete", &json!({}));
    let shown = agent::bind::Shown::new(&f.s, "calendar.delete", &json!({}), &what).expect("a delete is about the appointment in focus");
    f.s.run("calendar.select", &json!({"id": f.unrelated})).expect("the owner may select another");
    assert!(
        shown.check(&f.s, "calendar.delete", &json!({})).is_err(),
        "a delete that named nothing was about the appointment in focus, and the owner has changed it"
    );
}

#[test]
fn a_paste_question_says_the_requested_time_and_destination_calendar() {
    let f = fixture();
    let params = json!({"items": [{"id": f.kickoff}], "start": START, "calendar": f.remote, "cut": true});
    let what = agent::describe(&f.s, "calendar.paste", &params);
    assert!(what.contains(START), "the owner must see where in time the items will move: {what}");
    assert!(what.contains("Second account"), "the owner must see the destination calendar: {what}");
}

#[test]
fn an_approval_does_not_authorize_a_different_paste_time_or_command() {
    let f = fixture();
    let params = json!({"items": [{"id": f.kickoff}], "start": START, "cut": true});
    let what = agent::describe(&f.s, "calendar.paste", &params);
    let shown = agent::bind::Shown::new(&f.s, "calendar.paste", &params, &what).unwrap();
    let mut changed = params.clone();
    changed["start"] = json!("2026-12-20T17:00");
    assert!(shown.check(&f.s, "calendar.paste", &changed).is_err(), "the same items at another time require a new yes");
    assert!(shown.check(&f.s, "calendar.delete", &params).is_err(), "a paste approval is not a delete approval");
}

#[test]
fn a_cut_affecting_unsaved_owner_work_is_asked_even_with_every_class_granted() {
    let f = fixture();
    let params = json!({"items": [{"id": f.kickoff}], "start": START, "cut": true, "unsaved": true});
    assert_eq!(agent::classify(&f.s, "calendar.paste", &params), Access::Asked(Class::Delete));
    assert!(matches!(agent::check(&f.s, "calendar.paste", &params, &Class::ALL), Verdict::Ask(..)));
}

#[path = "calendar_batch_binding_tests/review_tests.rs"]
mod review_tests;

#[path = "calendar_batch_binding_tests/implicit_recipient_tests.rs"]
mod implicit_recipient_tests;

#[path = "calendar_batch_binding_tests/guest_recipient_tests.rs"]
mod guest_recipient_tests;

#[path = "calendar_batch_binding_tests/legacy_shown_tests.rs"]
mod legacy_shown_tests;

#[path = "calendar_batch_binding_tests/legacy_zone_tests.rs"]
mod legacy_zone_tests;

#[path = "calendar_batch_binding_tests/legacy_clock_tests.rs"]
mod legacy_clock_tests;

#[path = "calendar_batch_binding_tests/anonymous_clock_tests.rs"]
mod anonymous_clock_tests;

#[path = "calendar_batch_binding_tests/coderabbit_tests.rs"]
mod coderabbit_tests;

#[path = "calendar_batch_binding_tests/notice_plan_tests.rs"]
mod notice_plan_tests;

#[path = "calendar_batch_binding_tests/review03_tests.rs"]
mod review03_tests;
