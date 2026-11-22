// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Batch calendar actions: a typed set deleted as one step, a selected set cut and pasted as a
//! move, and a batch that names something it can't do refused whole.
//!
//! Synthetic events on `example.org` only, with a fixed clock and zone; no files, servers or
//! accounts with a real server. These are integration tests of the public engine API.

use mailcraft_engine::time::{Date, DateTime, Zone};
use mailcraft_engine::{SelectedEvent, Session};
use mailcraft_model::remote::{EventChange, MAX_EVENT_CHANGES};
use mailcraft_model::{Address, Attendee, Calendar, Event, Freq, Origin, Recurrence, Rgb, Rights};
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

fn add_event(s: &mut Session, mut event: Event) -> u64 {
    event.calendar = s.mailbox.default_calendar().expect("the sample mailbox has a local calendar");
    s.mailbox.add_event(event).expect("a local calendar takes an event")
}

fn attendee() -> Attendee {
    Attendee { address: Address::new("Bo Example", "bo@example.org"), ..Default::default() }
}

/// A second account, so that its id can be named as the server of an event without any server.
fn account(s: &mut Session) -> u64 {
    s.run("account.add", &json!({"email": "zoe@example.org", "name": "Zoe", "incoming": {"host": "imap.example.org"}})).expect("an account is added")
        ["id"]
        .as_u64()
        .expect("an account id")
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

/// One day of a series as the series names it (`Occurrence::day`), and as it is shown here.
fn day_and_displayed(s: &Session, series: &Event, from: Date, to: Date) -> (Date, Date) {
    let here = |instant: i64| s.offset_at(instant);
    let occurrences = series.occurrences_in(from, to, &here);
    let shifted = occurrences.iter().find(|o| o.day != o.start.date).copied().expect("fixture: the two clocks disagree about a date");
    (shifted.day, shifted.start.date)
}

/// Every occurrence of `series` the calendar shows here between `from` and `to`.
fn occurrences_shown(s: &Session, series: &Event, from: Date, to: Date) -> Vec<mailcraft_model::Occurrence> {
    series.occurrences_in(from, to, &|instant| s.offset_at(instant))
}

#[test]
fn a_daily_series_occurrence_is_deleted_by_its_own_day_and_its_neighbour_stays() {
    // The series repeats by Europe/Berlin's clock, the calendar is shown by America/New_York's:
    // in the weeks between the two clocks changing, an occurrence's day of the series is the day
    // before the date it is shown on. (The weeks are March's: in January both clocks agree about
    // every date, so the disagreement this guards against can't arise then.)
    let mut s =
        Session::in_zone(mailcraft_engine::sample::mailbox(NOW, 0), NOW, Zone::named("America/New_York").expect("a zone of the bundled database"));
    let start = at(d(2026, 1, 6), 23, 0);
    let series = add_event(
        &mut s,
        Event {
            title: "Berlin stand-up".into(),
            start,
            end: start.add_minutes(30),
            start_zone: "Europe/Berlin".into(),
            recurrence: Some(Recurrence::every(Freq::Daily)),
            ..Default::default()
        },
    );
    let before = s.mailbox.event(series).cloned().expect("the series was just added");
    let (own_day, displayed) = day_and_displayed(&s, &before, d(2026, 3, 1), d(2026, 3, 31));
    assert_ne!(own_day, displayed, "fixture: the series' day and the date it shows on differ");
    // The occurrence after the chosen one, as it stands before the day is taken away.
    let neighbour = occurrences_shown(&s, &before, d(2026, 3, 1), d(2026, 4, 1))
        .into_iter()
        .find(|o| o.day > own_day)
        .expect("fixture: the chosen day has another occurrence after it");

    s.run("calendar.select", &json!({"items": [{"id": series, "day": own_day.iso()}]})).expect("a typed item is taken");
    assert_eq!(s.sel.events, vec![SelectedEvent { id: series, day: Some(own_day) }]);
    assert_eq!(s.sel.event_day, Some(displayed), "the legacy focus keeps the displayed date");

    // The same day, deleted as one explicit item: the series' own day is what goes.
    s.run("calendar.delete", &json!({"items": [{"id": series, "day": own_day.iso()}]})).expect("the day is deleted");
    let after = s.mailbox.event(series).cloned().expect("the series still stands");
    assert_eq!(after.exceptions, vec![own_day], "the series' own day is the one cancelled");
    assert_ne!(own_day, displayed, "fixture");
    // And only that day: the occurrence after it still happens, where it happened, so what was
    // shown on the deleted day's date is not what went.
    let left = occurrences_shown(&s, &after, d(2026, 3, 1), d(2026, 4, 1));
    assert!(!left.iter().any(|o| o.day == own_day), "the chosen day is gone");
    let kept = left.iter().find(|o| o.day == neighbour.day).expect("the neighbouring occurrence stays");
    assert_eq!((kept.start, kept.end), (neighbour.start, neighbour.end), "the neighbour keeps its own time");
}

#[test]
fn deleting_two_occurrences_of_one_series_is_one_undo_step() {
    let mut s = session();
    let start = at(d(2026, 11, 4), 9, 0);
    let series = add_event(
        &mut s,
        Event {
            title: "Weekly sync".into(),
            start,
            end: start.add_minutes(30),
            recurrence: Some(Recurrence::every(Freq::Weekly)),
            ..Default::default()
        },
    );
    // Two selected days of the one series, as a click on each would leave them.
    s.sel.events = vec![SelectedEvent { id: series, day: Some(d(2026, 11, 11)) }, SelectedEvent { id: series, day: Some(d(2026, 11, 18)) }];
    s.sel.event = Some(series);
    s.sel.event_day = Some(d(2026, 11, 18));
    let steps = s.undo_steps_moved();

    s.run("calendar.delete", &json!({})).expect("a selection of more than one is what Delete takes");
    let after = s.mailbox.event(series).cloned().expect("the series still stands");
    assert_eq!(after.exceptions, vec![d(2026, 11, 11), d(2026, 11, 18)], "both days are gone, and neither twice");
    assert_eq!(s.undo_steps_moved(), steps + 1, "the whole batch is one undo step");
    assert!(s.undo(), "one undo takes both days back");
    assert!(s.mailbox.event(series).map(|e| e.exceptions.is_empty()).unwrap_or(false));
}

#[test]
fn a_cut_paste_moves_the_same_appointment_and_keeps_it_one_event() {
    let mut s = session();
    let start = at(d(2026, 11, 9), 9, 0);
    let event = add_event(
        &mut s,
        Event {
            title: "Kickoff".into(),
            location: "Room 1".into(),
            description: "The first one".into(),
            start,
            end: start.add_minutes(45),
            ..Default::default()
        },
    );
    let before = s.mailbox.event(event).cloned().expect("the appointment");
    let steps = s.undo_steps_moved();

    let made = s.run("calendar.paste", &json!({"items": [{"id": event}], "start": "2026-11-20T15:00", "cut": true})).expect("a cut pastes as a move");
    assert_eq!(made["cut"], json!(true));
    assert_eq!(made["ids"], json!([event]), "a moved appointment keeps its identity");
    let moved = s.mailbox.event(event).cloned().expect("the moved appointment");
    assert_eq!((moved.start, moved.end), (at(d(2026, 11, 20), 15, 0), at(d(2026, 11, 20), 15, 45)));
    assert_eq!(
        (moved.title.as_str(), moved.location.as_str(), moved.description.as_str()),
        (before.title.as_str(), before.location.as_str(), before.description.as_str()),
        "everything else of it stays as it was"
    );
    assert_eq!(s.undo_steps_moved(), steps + 1, "the move is one step");
    assert_eq!(s.sel.events, vec![SelectedEvent { id: event, day: None }], "the pasted item is the selection");
    assert_eq!((s.sel.event, s.sel.event_day), (Some(event), Some(d(2026, 11, 20))));
    assert!(s.undo());
    assert_eq!(s.mailbox.event(event).map(|e| e.start), Some(start), "one undo puts it back");
}

#[test]
fn a_batch_that_names_something_gone_changes_nothing() {
    let mut s = session();
    let start = at(d(2026, 11, 4), 9, 0);
    let event = add_event(&mut s, Event { title: "One".into(), start, end: start.add_minutes(30), ..Default::default() });
    s.run("calendar.select", &json!({"id": event})).expect("it is selected");
    let mailbox = s.mailbox.clone();
    let selection = s.sel.clone();
    let steps = s.undo_steps_moved();

    for (id, params) in [
        ("calendar.delete", json!({"items": [{"id": event}, {"id": 999_999}]})),
        ("calendar.paste", json!({"items": [{"id": event}, {"id": 999_999}], "start": "2026-11-20T09:00"})),
        ("calendar.paste", json!({"items": [{"id": event}, {"id": event}], "start": "2026-11-20T09:00"})),
    ] {
        assert!(s.run(id, &params).is_err(), "{id} {params}");
        assert!(s.mailbox == mailbox, "a refused batch leaves the mailbox as it was: {id} {params}");
        assert_eq!(s.sel, selection, "and the selection: {id} {params}");
        assert_eq!(s.undo_steps_moved(), steps, "and undo/redo: {id} {params}");
    }
}

#[test]
fn hostile_items_are_refused_and_never_take_the_selection() {
    let mut s = session();
    let start = at(d(2026, 11, 4), 9, 0);
    let event = add_event(&mut s, Event { title: "One".into(), start, end: start.add_minutes(30), ..Default::default() });
    s.run("calendar.select", &json!({"id": event})).expect("it is selected");
    let selection = s.sel.clone();
    // (201 different appointments, so that the length of the list is the only thing refused;
    // naming one appointment twice is a case of its own.)
    let many: Vec<serde_json::Value> = (0..201i64)
        .map(|n| {
            let start = at(d(2026, 11, 5), 0, 0).add_minutes(n);
            json!({"id": add_event(&mut s, Event { title: format!("Many {n}"), start, end: start.add_minutes(1), ..Default::default() })})
        })
        .collect();
    let payloads: Vec<(&str, serde_json::Value)> = vec![
        ("calendar.select", json!({"items": [{}]})),
        ("calendar.select", json!({"items": "an item"})),
        ("calendar.select", json!({"items": []})),
        ("calendar.select", json!({"items": [{"id": 0}]})),
        ("calendar.select", json!({"items": [{"id": event}], "id": event})),
        ("calendar.select", json!({"items": [{"id": event, "day": "not-a-day"}]})),
        ("calendar.select", json!({"items": [{"id": event, "day": 7}]})),
        ("calendar.select", json!({"items": [{"id": event}], "extend": "yes"})),
        ("calendar.select", json!({"items": [{"id": event}, {"id": event}]})),
        ("calendar.select", json!({"items": many.clone()})),
        ("calendar.delete", json!({"items": [{}]})),
        ("calendar.delete", json!({"items": "an item"})),
        ("calendar.delete", json!({"items": many.clone()})),
        ("calendar.paste", json!({"items": [{"id": event}]})),
        ("calendar.paste", json!({"items": [{"id": event}], "start": "2026-11-20T09:00", "cut": "yes"})),
        ("calendar.paste", json!({"items": [{"id": event}], "start": "2026-11-20T09:00", "send": "yes"})),
        ("calendar.paste", json!({"items": many, "start": "2026-11-20T09:00"})),
    ];
    for (id, params) in payloads {
        assert!(s.run(id, &params).is_err(), "{id} {params}");
        assert_eq!(s.sel, selection, "a refused payload never takes the selection: {id} {params}");
        assert!(s.mailbox.event(event).is_some(), "{id} {params}");
    }
}

#[test]
fn a_free_busy_block_is_neither_copied_nor_deleted() {
    let mut s = session();
    let calendar = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar {
        id: calendar,
        name: "Somebody's busy times".into(),
        color: Rgb(0xD1, 0x3B, 0x3B),
        visible: true,
        rights: Rights::FreeBusy,
        ..Default::default()
    });
    let start = at(d(2026, 11, 4), 9, 0);
    let event = s
        .mailbox
        .add_event(Event { calendar, title: "Busy".into(), start, end: start.add_minutes(30), ..Default::default() })
        .expect("a block is added");
    s.run("calendar.select", &json!({"id": event})).expect("a block is selected");
    let mailbox = s.mailbox.clone();
    for (id, params) in
        [("calendar.paste", json!({"items": [{"id": event}], "start": "2026-11-20T09:00"})), ("calendar.delete", json!({"items": [{"id": event}]}))]
    {
        let error = s.run(id, &params).expect_err("a busy block is not an appointment");
        assert!(error.to_string().contains("busy"), "{id}: {error}");
        assert!(s.mailbox == mailbox, "{id}");
    }
}

#[test]
fn a_meeting_cut_into_a_server_calendar_invites_nobody_without_a_yes() {
    let mut s = session();
    let account = account(&mut s);
    let target = server_calendar(&mut s, account);
    let start = at(d(2026, 11, 9), 9, 0);
    let meeting = add_event(
        &mut s,
        Event {
            title: "Planning".into(),
            start,
            end: start.add_minutes(60),
            organizer: Some(Address::new("Zoe Example", "zoe@example.org")),
            attendees: vec![attendee()],
            ..Default::default()
        },
    );
    let mailbox = s.mailbox.clone();
    let refused = s.run("calendar.paste", &json!({"items": [{"id": meeting}], "start": "2026-11-20T09:00", "cut": true, "calendar": target}));
    let error = refused.expect_err("moving a meeting into the server's calendar invites people");
    assert!(error.to_string().contains("send"), "{error}");
    assert!(s.mailbox == mailbox, "nothing is moved without the yes");

    // With the yes, it is one move, one queued change, and the mail is approved once.
    let made = s
        .run("calendar.paste", &json!({"items": [{"id": meeting}], "start": "2026-11-20T09:00", "cut": true, "calendar": target, "send": true}))
        .expect("the move is made");
    assert_eq!(made["ids"], json!([meeting]));
    assert_eq!(s.mailbox.event(meeting).map(|e| e.calendar), Some(target));
    let sent: Vec<&EventChange> = s.mailbox.event_changes.iter().filter(|c| c.event == meeting && c.send).collect();
    assert_eq!(sent.len(), 1, "one change carries the owner's yes: {:?}", s.mailbox.event_changes);
}

#[test]
fn a_batch_delete_of_two_meetings_asks_about_both_at_once() {
    let mut s = session();
    let account = account(&mut s);
    let start = at(d(2026, 11, 9), 9, 0);
    let mut ids = Vec::new();
    for (n, remote) in ["server-1", "server-2"].into_iter().enumerate() {
        let event = add_event(
            &mut s,
            Event {
                title: format!("Review {n}"),
                start: start.add_days(n as i64),
                end: start.add_minutes(60).add_days(n as i64),
                organizer: Some(Address::new("Zoe Example", "zoe@example.org")),
                // (Somebody else to each: a batch counts each person once.)
                attendees: vec![Attendee { address: Address::new("Bo Example", format!("bo{n}@example.org")), ..Default::default() }],
                origin: Origin { account, id: remote.into(), ..Default::default() },
                ..Default::default()
            },
        );
        ids.push(event);
    }
    let mailbox = s.mailbox.clone();
    let items = json!([{"id": ids[0]}, {"id": ids[1]}]);
    s.run("calendar.select", &json!({"id": ids[0]})).expect("one of them is selected");
    let refused = s.run("calendar.delete", &json!({"items": items.clone()}));
    let error = refused.expect_err("cancelling two meetings tells two people");
    assert!(error.to_string().contains("people"), "one question counts them all: {error}");
    assert!(s.mailbox == mailbox, "nothing is cancelled without the yes");

    s.run("calendar.delete", &json!({"items": items, "send": true})).expect("both are cancelled");
    assert!(s.mailbox.event(ids[0]).is_none() && s.mailbox.event(ids[1]).is_none());
    let approved: Vec<&EventChange> =
        s.mailbox.event_changes.iter().filter(|c| c.send && matches!(c.kind, mailcraft_model::remote::EventChangeKind::Delete)).collect();
    assert_eq!(approved.len(), 2, "one cancellation each, both approved: {:?}", s.mailbox.event_changes);
}

#[test]
fn a_calendar_with_no_room_for_more_changes_takes_no_paste() {
    let mut s = session();
    let account = account(&mut s);
    let target = server_calendar(&mut s, account);
    let start = at(d(2026, 11, 9), 9, 0);
    let event = add_event(&mut s, Event { title: "Kickoff".into(), start, end: start.add_minutes(45), ..Default::default() });
    while s.mailbox.event_changes.len() < MAX_EVENT_CHANGES {
        s.mailbox.event_changes.push(EventChange::default());
    }
    let mailbox = s.mailbox.clone();
    let refused = s.run("calendar.paste", &json!({"items": [{"id": event}], "start": "2026-11-20T09:00", "calendar": target}));
    let error = refused.expect_err("a full queue of changes refuses one more");
    assert!(error.to_string().contains("Too many calendar changes"), "{error}");
    assert!(s.mailbox == mailbox);
}

/// #209: a calendar subscribed to from the web, with the one event its feed brought. Its events
/// are written by its refresh and by nothing else.
fn subscribed(s: &mut Session) -> (u64, u64) {
    let address = "https://cal.example.org/fixtures/club.ics";
    let calendar = s.run("calendar.subscribe", &json!({"address": address})).expect("a calendar on the web is subscribed to")["id"]
        .as_u64()
        .expect("the subscribed calendar's id");
    let feed = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Club fixtures\r\nBEGIN:VEVENT\r\nUID:match@example.org\r\nSUMMARY:Home match\r\n\
                DTSTART:20261114T150000Z\r\nDTEND:20261114T160000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let outcome = json!({"kind": "changed", "text": feed, "etag": "", "lastModified": ""});
    s.run("sync.applySubscription", &json!({"calendar": calendar, "outcome": outcome})).expect("the feed is taken in");
    let event = s.mailbox.events.values().find(|e| e.calendar == calendar).map(|e| e.id).expect("the feed's event");
    (calendar, event)
}

/// #209 and a typed set: a batch Delete (by `items`, alone or beside an appointment of the
/// owner's own, or of the owner's typed selection), a cut out of a subscribed calendar and a paste
/// into one are refused as the single `calendar.delete {"id"}` is, for what they are, and leave
/// the calendar, its event and the owner's own appointment as they were. Each form is tried on a
/// fresh session.
#[test]
fn no_batch_form_changes_a_subscribed_calendar() {
    let fresh = || {
        let mut s = session();
        let (calendar, event) = subscribed(&mut s);
        let start = at(d(2026, 11, 20), 10, 0);
        let own = add_event(&mut s, Event { title: "Dentist".into(), start, end: start.add_minutes(30), ..Default::default() });
        (s, calendar, event, own)
    };
    type Params = fn(u64, u64, u64) -> serde_json::Value;
    let forms: [(&str, &str, Params); 6] = [
        ("one by id", "calendar.delete", |_, event, _| json!({"id": event})),
        ("items", "calendar.delete", |_, event, _| json!({"items": [{"id": event}]})),
        ("items with the owner's own", "calendar.delete", |_, event, own| json!({"items": [{"id": own}, {"id": event}]})),
        ("typed selection", "calendar.delete", |_, _, _| json!({})),
        ("cut out of it", "calendar.paste", |_, event, _| json!({"items": [{"id": event}], "start": "2026-11-21T10:00", "cut": true})),
        ("pasted into it", "calendar.paste", |calendar, _, own| json!({"items": [{"id": own}], "start": "2026-11-21T10:00", "calendar": calendar})),
    ];
    for (name, command, params) in forms {
        let (mut s, calendar, event, own) = fresh();
        if name == "typed selection" {
            s.run("calendar.select", &json!({"items": [{"id": own}, {"id": event}]})).expect("both are selected");
        }
        let mailbox = s.mailbox.clone();
        let error = s.run(command, &params(calendar, event, own)).expect_err(name);
        assert!(error.to_string().contains("is a copy of one on the web"), "{name}: refused for what it is: {error}");
        assert!(s.mailbox == mailbox, "{name}: nothing was changed");
        assert!(s.mailbox.event(event).is_some() && s.mailbox.event(own).is_some(), "{name}: both are still there");
    }
}

/// A cut pasted at the very time it is at, but into another calendar, is a move to that calendar,
/// not a step that changes nothing (CodeRabbit on #1665, selection.rs:463); into its own calendar
/// at its own time it still is nothing to do.
#[test]
fn a_cut_into_another_calendar_at_the_same_time_moves_it_there() {
    let mut s = session();
    let other = s.mailbox.alloc_id();
    s.mailbox.calendars.push(Calendar { id: other, name: "Other".into(), color: Rgb(0x2E, 0x8B, 0x57), visible: true, ..Default::default() });
    let start = at(d(2026, 11, 20), 10, 0);
    let id = add_event(&mut s, Event { title: "Dentist".into(), start, end: start.add_minutes(30), ..Default::default() });
    let home = s.mailbox.event(id).expect("the appointment").calendar;
    let mailbox = s.mailbox.clone();
    let same = json!({"items": [{"id": id}], "start": "2026-11-20T10:00", "cut": true});
    assert!(s.run("calendar.paste", &same).is_err(), "a cut put back where it is moves nothing");
    assert!(s.mailbox == mailbox, "and changes nothing");
    let there = json!({"items": [{"id": id}], "start": "2026-11-20T10:00", "cut": true, "calendar": other});
    let made = s.run("calendar.paste", &there).expect("a cut into another calendar at the same time is a move");
    assert_eq!(made["ids"], json!([id]), "the appointment itself moved: {made}");
    let moved = s.mailbox.event(id).expect("still one appointment");
    assert_eq!((moved.calendar, moved.start, moved.end), (other, start, start.add_minutes(30)), "into the other calendar, at its own time");
    assert_ne!(home, other);
}

/// A cut that names no `start` is refused by the command before anything is changed or sent, so
/// no question about mail is asked for it (CodeRabbit on #1665, selection.rs:876).
#[test]
fn a_cut_without_a_start_asks_about_no_mail() {
    let mut s = session();
    let account = account(&mut s);
    let calendar = server_calendar(&mut s, account);
    let start = at(d(2026, 11, 12), 10, 0);
    let meeting = s
        .mailbox
        .add_event(Event {
            calendar,
            title: "Kept meeting".into(),
            start,
            end: start.add_minutes(30),
            attendees: vec![attendee()],
            origin: Origin { account, id: "srv-meeting".into(), ..Default::default() },
            ..Default::default()
        })
        .expect("a server calendar takes a meeting");
    let with = json!({"items": [{"id": meeting}], "start": "2026-11-13T10:00", "cut": true});
    assert!(mailcraft_engine::cmd::calendar::mail_of(&s, "calendar.paste", &with).is_some(), "fixture: a cut of the meeting tells its people");
    for without in [json!({"items": [{"id": meeting}], "cut": true}), json!({"items": [{"id": meeting}], "cut": true, "start": "soon"})] {
        let mut probe = Session::at(s.mailbox.clone(), NOW, 0);
        assert!(probe.run("calendar.paste", &without).is_err(), "fixture: the command refuses {without}");
        assert_eq!(mailcraft_engine::cmd::calendar::mail_of(&s, "calendar.paste", &without), None, "no mail is asked about for {without}");
    }
}

/// A read-only calendar of a server (`Rights::Read`), and a shared mailbox's calendar the owner
/// has no edit rights to (#70), are refused whole by every batch form, as by the single
/// `calendar.delete {"id"}`, and nothing of the owner's is changed either (review-02 N2).
#[test]
fn no_batch_form_changes_a_read_only_or_shared_calendar() {
    /// (session, the read-only calendar's event, the owner's own, the words of the refusal)
    fn fixture(shared: bool) -> (Session, u64, u64, &'static str) {
        let mut s = session();
        let account = account(&mut s);
        if shared {
            let owner = s.mailbox.accounts.iter().find(|a| a.id != account).map(|a| a.id).expect("the sample's own account");
            s.mailbox.account_mut(account).expect("the added account").shared_of = Some(owner);
        }
        let calendar = s.mailbox.alloc_id();
        s.mailbox.calendars.push(Calendar {
            id: calendar,
            name: "Team".into(),
            color: Rgb(0x2E, 0x8B, 0x57),
            visible: true,
            rights: Rights::Read,
            origin: Origin { account, id: "team".into(), ..Default::default() },
            ..Default::default()
        });
        let start = at(d(2026, 11, 12), 10, 0);
        let read_only = s
            .mailbox
            .add_event(Event {
                calendar,
                title: "Board review".into(),
                start,
                end: start.add_minutes(60),
                origin: Origin { account, id: "srv-1".into(), ..Default::default() },
                ..Default::default()
            })
            .expect("the calendar holds the event it was given");
        let own_start = at(d(2026, 11, 20), 10, 0);
        let own = add_event(&mut s, Event { title: "Dentist".into(), start: own_start, end: own_start.add_minutes(60), ..Default::default() });
        let words = if shared { "may only read the shared mailbox" } else { "can only be read" };
        (s, read_only, own, words)
    }
    type Params = fn(u64, u64) -> serde_json::Value;
    let forms: [(&str, &str, Params); 6] = [
        ("one by id", "calendar.delete", |ro, _| json!({"id": ro})),
        ("items", "calendar.delete", |ro, _| json!({"items": [{"id": ro}]})),
        ("items, the owner's own first", "calendar.delete", |ro, own| json!({"items": [{"id": own}, {"id": ro}]})),
        ("items, the owner's own last", "calendar.delete", |ro, own| json!({"items": [{"id": ro}, {"id": own}]})),
        ("typed selection", "calendar.delete", |_, _| json!({})),
        ("cut out of it", "calendar.paste", |ro, own| json!({"items": [{"id": own}, {"id": ro}], "start": "2026-11-21T10:00", "cut": true})),
    ];
    for shared in [false, true] {
        for (name, command, params) in forms {
            let (mut s, read_only, own, words) = fixture(shared);
            if name == "typed selection" {
                s.run("calendar.select", &json!({"items": [{"id": own}, {"id": read_only}]})).expect("both are selected");
            }
            let mailbox = s.mailbox.clone();
            let error = s.run(command, &params(read_only, own)).expect_err(name);
            assert!(error.to_string().contains(words), "shared={shared} {name}: refused for what it is: {error}");
            assert!(s.mailbox == mailbox, "shared={shared} {name}: nothing was changed");
        }
    }
}
