// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What this crate writes to Microsoft Graph's calendar, held against a stand-in that takes only
//! what the documented service takes ([`standin`]). Nothing here leaves this machine.

use mailcraft_model::remote::{EventReminder, EventRepeat, EventWall};
use mailcraft_model::{Freq, Recurrence, ShowAs};
use mailcraft_time::{Date, Weekday};
use serde_json::json;

use super::check::{STEPS, check_calendar_at};
use super::standin::{StandIn, Stored, TOKEN, zone_offset};
use super::*;

fn at(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> Timestamp {
    DateTime::new(Date::new(year, month, day).unwrap(), hour, minute, 0).unwrap().to_timestamp(0)
}

fn clock(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> DateTime {
    DateTime::new(Date::new(year, month, day).unwrap(), hour, minute, 0).unwrap()
}

const MARK: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const EASTERN: &str = "Eastern Standard Time";

fn timed(start: Timestamp, minutes: i64) -> Option<EventTime> {
    Some(EventTime { start, end: start + minutes * 60, all_day: false })
}

/// Everything of a new appointment, as the mailbox sends one.
fn appointment(title: &str, start: Timestamp, minutes: i64) -> EventFields {
    EventFields {
        title: Some(title.into()),
        location: Some(String::new()),
        description: Some(String::new()),
        time: timed(start, minutes),
        show_as: Some(ShowAs::Busy),
        private: Some(false),
        reminder: Some(EventReminder { on: true, minutes: 15 }),
        ..Default::default()
    }
}

fn create(mark: &str, fields: EventFields) -> EventOp {
    EventOp { kind: EventChangeKind::Create, event: 7, token: mark.into(), fields, ..Default::default() }
}

fn update(remote: &str, fields: EventFields) -> EventOp {
    EventOp { kind: EventChangeKind::Update, event: 8, remote: remote.into(), fields, ..Default::default() }
}

fn delete(remote: &str) -> EventOp {
    EventOp { kind: EventChangeKind::Delete, event: 9, remote: remote.into(), ..Default::default() }
}

/// Send `ops` to the stand-in, in the owner's zone `zone`.
fn push(server: &StandIn, ops: Vec<EventOp>, zone: &str) -> Vec<EventOutcome> {
    let upload = EventsUpload { account: 3, email: "owner@example.org".into(), ops, ..Default::default() };
    let uploaded = push_events_at(&server.url(), TOKEN, &upload, &EventOptions { zone: zone.into(), ..Default::default() });
    assert!(!uploaded.token_refused);
    uploaded.outcomes
}

/// The one outcome of sending `op`, which the stand-in took.
fn done(server: &StandIn, op: EventOp, zone: &str) -> EventOutcome {
    let mut outcomes = push(server, vec![op], zone);
    assert_eq!(outcomes.len(), 1);
    let outcome = outcomes.remove(0);
    assert_eq!(outcome.result, EventResult::Done, "{}: {:?}", outcome.error, server.state().requests.last());
    outcome
}

/// October 2026 of a calendar of the stand-in's, as this crate reads it.
fn october(server: &StandIn, calendar: &str) -> EventsDownload {
    let query =
        EventsQuery { calendar: calendar.into(), from: at(2026, 10, 1, 0, 0), to: at(2026, 11, 1, 0, 0), series: Vec::new(), shared: String::new() };
    events_in_at(&server.url(), TOKEN, &query).unwrap()
}

/// The statuses of everything the stand-in answered so far that wasn't a yes.
fn refusals(server: &StandIn) -> Vec<(String, String, u16)> {
    server
        .state()
        .requests
        .iter()
        .filter(|asked| asked.status >= 400)
        .map(|asked| (asked.method.clone(), asked.target.clone(), asked.status))
        .collect()
}

#[test]
fn an_appointment_is_made_moved_changed_and_deleted_as_the_service_takes_it() {
    let server = StandIn::start().unwrap();
    // Made: 201, with an id that is one piece of an address only when it is written as one.
    let made = done(&server, create(MARK, appointment("Lunch", at(2026, 10, 15, 16, 0), 30)), EASTERN);
    let id = made.remote.clone();
    assert!(id.len() > 100 && ["=", "-", "_", "/", "+"].iter().all(|c| id.contains(c)), "{id}");
    assert!(!made.change_key.is_empty() && !made.uid.is_empty() && made.modified > 0);
    assert_eq!(server.state().event("Lunch").map(|e| (e.start, e.end, e.all_day)), Some((at(2026, 10, 15, 16, 0), at(2026, 10, 15, 16, 30), false)));
    // Sent a second time with the same mark (the first answer was lost): the same event.
    let again = done(&server, create(MARK, appointment("Lunch", at(2026, 10, 15, 16, 0), 30)), EASTERN);
    assert_eq!((again.remote.as_str(), server.state().events.len()), (id.as_str(), 1));

    // Moved (a drag): the change goes to the event by its id, and the service has the new time.
    let moved = done(&server, update(&id, EventFields { time: timed(at(2026, 10, 16, 13, 0), 30), ..Default::default() }), EASTERN);
    assert_eq!(moved.remote, id);
    assert!(moved.change_key != made.change_key && moved.modified > made.modified);
    assert_eq!(server.state().event("Lunch").map(|e| e.start), Some(at(2026, 10, 16, 13, 0)));
    // Made longer (a resize), renamed, shown otherwise, with another reminder and a place.
    let longer = EventFields {
        time: timed(at(2026, 10, 16, 13, 0), 90),
        title: Some("Long lunch".into()),
        location: Some("Canteen".into()),
        description: Some("Bring\nthe numbers.".into()),
        show_as: Some(ShowAs::OutOfOffice),
        private: Some(true),
        reminder: Some(EventReminder { on: false, minutes: 0 }),
        categories: Some(vec!["Team".into()]),
        ..Default::default()
    };
    done(&server, update(&id, longer), EASTERN);
    let have = server.state().event("Long lunch").cloned().unwrap();
    assert_eq!(
        (have.end - have.start, have.location.as_str(), have.show_as.as_str(), have.sensitivity.as_str()),
        (5_400, "Canteen", "oof", "private")
    );
    assert_eq!((have.reminder_on, have.categories.as_slice(), have.body.as_str()), (false, &["Team".to_string()][..], "Bring\nthe numbers."));

    // What comes down is what went up: under the same id, at the same time.
    let down = october(&server, "");
    assert!(down.complete && down.has_categories);
    let event = down.events.iter().find(|event| event.id == id).expect("the event comes down under its id");
    assert_eq!(
        (event.title.as_str(), event.start, event.end, event.all_day),
        ("Long lunch", at(2026, 10, 16, 13, 0), at(2026, 10, 16, 14, 30), false)
    );
    assert_eq!((event.show_as, event.private, event.reminder_minutes, event.token.as_str()), (ShowAs::OutOfOffice, true, None, MARK));
    assert_eq!(
        (event.description.as_str(), event.location.as_str(), event.categories.as_slice()),
        ("Bring\nthe numbers.", "Canteen", &["Team".to_string()][..])
    );

    // Deleted: 204, and it is gone; deleted again, the service no longer has it.
    assert_eq!(done(&server, delete(&id), EASTERN).remote, "");
    assert!(server.state().events.is_empty());
    assert_eq!(push(&server, vec![delete(&id)], EASTERN)[0].result, EventResult::Gone);
    assert_eq!(push(&server, vec![update(&id, EventFields { title: Some("x".into()), ..Default::default() })], EASTERN)[0].result, EventResult::Gone);
    // Nothing else was refused on the way: every body, header and address was one the service takes.
    assert_eq!(refusals(&server).iter().map(|(method, _, status)| (method.as_str(), *status)).collect::<Vec<_>>(), [("DELETE", 404), ("PATCH", 404)]);
    // And every request named the event by its id as one piece of the address.
    let writes = server.state().writes();
    assert!(
        writes.iter().filter(|(method, _)| method != "POST").all(|(_, path)| path.as_slice() == ["v1.0", "me", "events", id.as_str()]),
        "{writes:?}"
    );
}

#[test]
fn whole_days_and_series_go_in_the_owners_zone() {
    let server = StandIn::start().unwrap();
    // A whole day: midnight to midnight in the owner's zone, which is a day there.
    let day = EventFields {
        time: Some(EventTime { start: at(2026, 10, 31, 0, 0), end: at(2026, 11, 2, 0, 0), all_day: true }),
        show_as: Some(ShowAs::Free),
        ..appointment("Offsite", 0, 0)
    };
    let made = done(&server, create("11111111-d9cb-469f-a165-70867728950e", day.clone()), EASTERN);
    let have = server.state().event("Offsite").cloned().unwrap();
    // (Midnight in New York is four in the morning in UTC in October, five in November.)
    assert_eq!((have.all_day, have.start, have.end, have.zone.as_str()), (true, at(2026, 10, 31, 4, 0), at(2026, 11, 2, 5, 0), EASTERN));
    // Moved to other days, and made a timed event again.
    let later =
        EventFields { time: Some(EventTime { start: at(2026, 10, 20, 0, 0), end: at(2026, 10, 21, 0, 0), all_day: true }), ..Default::default() };
    done(&server, update(&made.remote, later), EASTERN);
    assert_eq!(server.state().event("Offsite").map(|e| (e.start, e.end)), Some((at(2026, 10, 20, 4, 0), at(2026, 10, 21, 4, 0))));
    done(&server, update(&made.remote, EventFields { time: timed(at(2026, 10, 20, 13, 0), 60), ..Default::default() }), EASTERN);
    assert_eq!(server.state().event("Offsite").map(|e| (e.all_day, e.start)), Some((false, at(2026, 10, 20, 13, 0))));
    // No zone is known: the days are days in UTC, and the service takes that too.
    done(&server, create("22222222-d9cb-469f-a165-70867728950e", EventFields { title: Some("Elsewhere".into()), ..day }), "");
    assert_eq!(server.state().event("Elsewhere").map(|e| (e.start, e.zone.clone())), Some((at(2026, 10, 31, 0, 0), "UTC".to_string())));

    // A series: nine in the morning on Mondays and Wednesdays, by the clock in New York.
    let rule = Recurrence { by_day: vec![Weekday::Mon, Weekday::Wed], ..Recurrence::every(Freq::Weekly) };
    let first = Date::new(2026, 10, 5).unwrap();
    let series = |title: &str| EventFields {
        wall: Some(EventWall { start: clock(2026, 10, 5, 9, 0), end: clock(2026, 10, 5, 9, 30) }),
        repeat: Some(EventRepeat { rule: Some(rule.clone()), first }),
        ..appointment(title, at(2026, 10, 5, 13, 0), 30)
    };
    let made = done(&server, create("33333333-d9cb-469f-a165-70867728950e", series("Stand-up")), EASTERN);
    let master = server.state().event("Stand-up").cloned().unwrap();
    assert_eq!((master.start, master.zone.as_str(), master.recurrence.is_some()), (at(2026, 10, 5, 13, 0), EASTERN, true));
    // It comes down as its occurrences and as itself, with its rule; after the clocks go back
    // it is still at nine there, which is an hour later in UTC.
    let query =
        EventsQuery { calendar: String::new(), from: at(2026, 10, 26, 0, 0), to: at(2026, 11, 8, 0, 0), series: Vec::new(), shared: String::new() };
    let down = events_in_at(&server.url(), TOKEN, &query).unwrap();
    let starts: Vec<Timestamp> =
        down.events.iter().filter(|e| e.kind == RemoteEventKind::Occurrence && e.series == made.remote).map(|e| e.start).collect();
    assert_eq!(starts, [at(2026, 10, 26, 13, 0), at(2026, 10, 28, 13, 0), at(2026, 11, 2, 14, 0), at(2026, 11, 4, 14, 0)]);
    let itself = down.events.iter().find(|e| e.kind == RemoteEventKind::SeriesMaster).expect("the series comes as itself too");
    assert_eq!((itself.id.as_str(), itself.recurrence.as_ref(), itself.series_day), (made.remote.as_str(), Some(&rule), Some(first)));

    // The same series with no zone to say it in: it is told in UTC, and its days are the days
    // it has there. Eight in the evening on Mondays in New York is Tuesday in UTC.
    let evening = EventFields {
        wall: Some(EventWall { start: clock(2026, 10, 5, 20, 0), end: clock(2026, 10, 5, 21, 0) }),
        repeat: Some(EventRepeat { rule: Some(Recurrence { by_day: vec![Weekday::Mon], ..Recurrence::every(Freq::Weekly) }), first }),
        ..appointment("Evening class", at(2026, 10, 6, 0, 0), 60)
    };
    done(&server, create("44444444-d9cb-469f-a165-70867728950e", evening), "");
    let class = server.state().event("Evening class").cloned().unwrap();
    let range = &class.recurrence.as_ref().unwrap()["range"];
    assert_eq!(
        (class.start, range["startDate"].as_str(), class.recurrence.as_ref().unwrap()["pattern"]["daysOfWeek"][0].as_str()),
        (at(2026, 10, 6, 0, 0), Some("2026-10-06"), Some("tuesday"))
    );
    // The whole series is moved half an hour, and stops repeating.
    let half_later = EventFields {
        time: timed(at(2026, 10, 5, 13, 30), 30),
        wall: Some(EventWall { start: clock(2026, 10, 5, 9, 30), end: clock(2026, 10, 5, 10, 0) }),
        ..Default::default()
    };
    done(&server, update(&made.remote, half_later), EASTERN);
    assert_eq!(server.state().event("Stand-up").map(|e| e.start), Some(at(2026, 10, 5, 13, 30)));
    let once = EventFields { time: timed(at(2026, 10, 5, 13, 30), 30), repeat: Some(EventRepeat { rule: None, first }), ..Default::default() };
    done(&server, update(&made.remote, once), EASTERN);
    assert!(server.state().event("Stand-up").is_some_and(|e| e.recurrence.is_none()));
    assert_eq!(refusals(&server), []);
}

#[test]
fn one_occurrence_is_found_by_its_time_and_changed_or_deleted_by_its_own_id() {
    let server = StandIn::start().unwrap();
    let rule = Recurrence { by_day: vec![Weekday::Mon, Weekday::Wed, Weekday::Fri], ..Recurrence::every(Freq::Weekly) };
    let first = Date::new(2026, 10, 5).unwrap();
    let series = EventFields {
        wall: Some(EventWall { start: clock(2026, 10, 5, 9, 0), end: clock(2026, 10, 5, 9, 30) }),
        repeat: Some(EventRepeat { rule: Some(rule), first }),
        ..appointment("Stand-up", at(2026, 10, 5, 13, 0), 30)
    };
    let made = done(&server, create(MARK, series), EASTERN);
    // Friday the 16th an hour later: the occurrence is asked for by its time, then changed.
    let one = |kind: EventChangeKind, day: u8, fields: EventFields| EventOp {
        kind,
        event: 9,
        series: made.remote.clone(),
        occurrence: at(2026, 10, day, 13, 0),
        fields,
        ..Default::default()
    };
    let later = EventFields { time: timed(at(2026, 10, 16, 14, 0), 30), ..Default::default() };
    let changed = done(&server, one(EventChangeKind::Update, 16, later), EASTERN);
    assert!(!changed.remote.is_empty() && changed.remote != made.remote);
    // Monday the 12th is deleted.
    done(&server, one(EventChangeKind::Delete, 12, EventFields::default()), EASTERN);
    // Asked for again, neither is an occurrence the series has at that time any more.
    assert_eq!(push(&server, vec![one(EventChangeKind::Delete, 12, EventFields::default())], EASTERN)[0].result, EventResult::Gone);

    let down = october(&server, "");
    let of_it: Vec<(RemoteEventKind, Timestamp, Option<Timestamp>)> = down
        .events
        .iter()
        .filter(|e| e.series == made.remote)
        .filter(|e| (at(2026, 10, 12, 0, 0)..at(2026, 10, 17, 0, 0)).contains(&e.start))
        .map(|e| (e.kind, e.start, e.original))
        .collect();
    assert_eq!(
        of_it,
        [
            (RemoteEventKind::Occurrence, at(2026, 10, 14, 13, 0), Some(at(2026, 10, 14, 13, 0))),
            (RemoteEventKind::Exception, at(2026, 10, 16, 14, 0), Some(at(2026, 10, 16, 13, 0))),
        ]
    );
    // The exception goes by the id the occurrence had.
    assert!(down.events.iter().any(|e| e.kind == RemoteEventKind::Exception && e.id == changed.remote));
    // An occurrence can't be moved past the one beside it: the service says so, for good.
    let far = EventFields { time: timed(at(2026, 10, 21, 13, 0), 30), ..Default::default() };
    let refused = push(&server, vec![one(EventChangeKind::Update, 19, far)], EASTERN).remove(0);
    assert_eq!(
        (refused.result, refused.error.as_str()),
        (EventResult::Refused, "Microsoft Graph answered with status 400 (ErrorOccurrenceCrossingBoundary)")
    );
}

#[test]
fn an_event_of_another_calendar_is_written_through_that_calendar() {
    let server = StandIn::start().unwrap();
    let (team, shared, holidays) = {
        let mut state = server.state();
        (state.add_calendar("Team", true, false), state.add_calendar("Bo's calendar", true, true), state.add_calendar("Holidays", false, false))
    };
    // Made in a second calendar of the owner's own.
    let made = done(&server, EventOp { calendar: team.clone(), ..create(MARK, appointment("Retro", at(2026, 10, 15, 16, 0), 30)) }, EASTERN);
    assert_eq!(server.state().event("Retro").map(|e| e.calendar.clone()), Some(team.clone()));
    assert_eq!(october(&server, &team).events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), [made.remote.as_str()]);
    assert!(october(&server, "").events.is_empty());
    // Changed there: through its calendar, or (it is the owner's own) by itself.
    let moved = EventFields { time: timed(at(2026, 10, 15, 17, 0), 30), ..Default::default() };
    done(&server, EventOp { calendar: team.clone(), ..update(&made.remote, moved.clone()) }, EASTERN);
    done(&server, update(&made.remote, moved.clone()), EASTERN);

    // In the copy of a calendar that somebody else shares, an event is found through that
    // calendar and no other way: asked for by itself, the service doesn't know it.
    let theirs = server.state().add_event(&shared, "Their review", at(2026, 10, 14, 15, 0));
    assert_eq!(push(&server, vec![update(&theirs, moved.clone())], EASTERN)[0].result, EventResult::Gone);
    assert_eq!(server.state().event("Their review").map(|e| e.start), Some(at(2026, 10, 14, 15, 0)));
    done(&server, EventOp { calendar: shared.clone(), ..update(&theirs, moved.clone()) }, EASTERN);
    assert_eq!(server.state().event("Their review").map(|e| e.start), Some(at(2026, 10, 15, 17, 0)));
    done(&server, EventOp { calendar: shared.clone(), ..delete(&theirs) }, EASTERN);
    assert!(server.state().event("Their review").is_none());
    // Through a calendar it isn't in, it isn't there either.
    assert_eq!(push(&server, vec![EventOp { calendar: shared.clone(), ..delete(&made.remote) }], EASTERN)[0].result, EventResult::Gone);

    // A calendar the account may only read: the service says no, and why.
    let fixed = server.state().add_event(&holidays, "Bank holiday", at(2026, 10, 12, 0, 0));
    let denied = push(&server, vec![EventOp { calendar: holidays.clone(), ..update(&fixed, moved) }], EASTERN).remove(0);
    assert_eq!(
        (denied.result, denied.error.as_str()),
        (EventResult::Denied, "this account may not change its calendar through Microsoft Graph (ErrorAccessDenied)")
    );
    // The list of calendars says which is the main one, and which may be changed.
    let list = calendars_at(&server.url(), TOKEN, None).unwrap();
    let told: Vec<(&str, bool, bool)> = list.calendars.iter().map(|c| (c.name.as_str(), c.main, !c.rights.can_edit())).collect();
    assert_eq!(told, [("Calendar", true, false), ("Team", false, false), ("Bo's calendar", false, false), ("Holidays", false, true)]);
    // And whose each is: the signed-in person's own, or somebody else's that is shared with them.
    let whose: Vec<&str> = list.calendars.iter().map(|c| c.owner.as_ref().map_or("", |o| o.address.as_str())).collect();
    let own = server.state().owner.clone();
    assert_eq!(whose, [own.as_str(), own.as_str(), "else@example.org", own.as_str()]);
}

#[test]
fn the_stand_in_refuses_what_the_service_refuses() {
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let id = server.state().add_event(&main, "Budget review", at(2026, 10, 12, 13, 0));
    let graph = Graph::at(&server.url(), TOKEN).unwrap();
    let json = [("Content-Type", "application/json"), ("Prefer", EVENT_PREFERENCES)];
    // What Graph says to one request: the status and its name for the refusal.
    let said = |method: &str, path: &str, headers: &[(&str, &str)], body: Option<Value>| {
        let body = body.map(|body| body.to_string());
        let reply = graph.ask(&graph.url(path), method, headers, body.as_deref(), WRITE_TIMEOUT, MAX_WRITTEN).unwrap();
        let answer = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
        (reply.status, answer["error"]["code"].as_str().unwrap_or("").to_string())
    };
    let event = format!("{EVENTS}/{}", http::encode(&id));
    let utc = |text: &str| json!({"dateTime": text, "timeZone": "UTC"});
    for (what, method, path, body, status, code) in [
        // A whole day that isn't from midnight to midnight, or is less than a day, or is in two zones.
        (
            "all day, not midnight",
            "PATCH",
            event.clone(),
            json!({"start": utc("2026-10-12T09:00:00"), "end": utc("2026-10-13T09:00:00"), "isAllDay": true}),
            400,
            "ErrorPropertyValidationFailure",
        ),
        (
            "all day, no length",
            "PATCH",
            event.clone(),
            json!({"start": utc("2026-10-12T00:00:00"), "end": utc("2026-10-12T00:00:00"), "isAllDay": true}),
            400,
            "ErrorPropertyValidationFailure",
        ),
        (
            "all day, two zones",
            "PATCH",
            event.clone(),
            json!({"start": utc("2026-10-12T00:00:00"), "end": {"dateTime": "2026-10-13T00:00:00", "timeZone": EASTERN}, "isAllDay": true}),
            400,
            "ErrorPropertyValidationFailure",
        ),
        // Times that aren't written as times, zones nobody knows, an end before the start.
        ("a zone in the time", "PATCH", event.clone(), json!({"start": utc("2026-10-12T09:00:00Z")}), 400, "ErrorInvalidRequest"),
        ("no seconds", "PATCH", event.clone(), json!({"start": utc("2026-10-12T09:00")}), 400, "ErrorInvalidRequest"),
        (
            "an unknown zone",
            "PATCH",
            event.clone(),
            json!({"start": {"dateTime": "2026-10-12T09:00:00", "timeZone": "Mars/Olympus"}}),
            400,
            "TimeZoneNotSupportedException",
        ),
        ("backwards", "PATCH", event.clone(), json!({"end": utc("2026-10-12T12:00:00")}), 400, "ErrorCalendarEndDateIsEarlierThanStartDate"),
        // Properties an event doesn't have, or that can't be written, or of another type.
        ("an unknown property", "PATCH", event.clone(), json!({"title": "x"}), 400, "RequestBodyRead"),
        ("a read-only property", "PATCH", event.clone(), json!({"changeKey": "x"}), 400, "ErrorInvalidPropertySet"),
        ("the mark, in a change", "PATCH", event.clone(), json!({"transactionId": MARK}), 400, "ErrorInvalidPropertyUpdate"),
        ("a subject that is a number", "PATCH", event.clone(), json!({"subject": 7}), 400, "ErrorInvalidRequest"),
        ("a way of showing nobody has", "PATCH", event.clone(), json!({"showAs": "outOfOffice"}), 400, "ErrorInvalidRequest"),
        // A recurrence that doesn't start on the day the event starts, or lacks what its kind needs.
        (
            "a range from another day",
            "PATCH",
            event.clone(),
            json!({"recurrence": {"pattern": {"type": "daily", "interval": 1}, "range": {"type": "noEnd", "startDate": "2026-10-13"}}}),
            400,
            "ErrorPropertyValidationFailure",
        ),
        (
            "weekly without days",
            "PATCH",
            event.clone(),
            json!({"recurrence": {"pattern": {"type": "weekly", "interval": 1}, "range": {"type": "noEnd", "startDate": "2026-10-12"}}}),
            400,
            "ErrorPropertyValidationFailure",
        ),
        // A new event without times.
        ("no times", "POST", EVENTS.to_string(), json!({"subject": "x"}), 400, "ErrorPropertyValidationFailure"),
        // An id that isn't written as one piece of an address, one that names nothing, one that is none.
        ("an id with its slash as it is", "PATCH", format!("{EVENTS}/{id}"), json!({"subject": "x"}), 400, "BadRequest"),
        (
            "an id nobody has",
            "PATCH",
            format!("{EVENTS}/{}", http::encode(&format!("{}ZZZ=", id.trim_end_matches("AAA=")))),
            json!({"subject": "x"}),
            404,
            "ErrorItemNotFound",
        ),
        ("no id at all", "PATCH", format!("{EVENTS}/e-1"), json!({"subject": "x"}), 400, "ErrorInvalidIdMalformed"),
    ] {
        assert_eq!(said(method, &path, &json, Some(body)), (status, code.to_string()), "{what}");
    }
    // Nothing of all that changed the event.
    assert_eq!(server.state().event("Budget review").map(|e| (e.start, e.version)), Some((at(2026, 10, 12, 13, 0), 1)));
    // A body that doesn't say it is JSON; a token that isn't the one.
    assert_eq!(said("PATCH", &event, &[("Content-Type", "text/plain")], Some(json!({"subject": "x"}))), (415, "RequestBodyRead".to_string()));
    let stranger = Graph::at(&server.url(), "another.token-0123456789").unwrap();
    let reply = stranger.ask(&stranger.url(&event), "GET", &[], None, ASK_TIMEOUT, MAX_PAGE).unwrap();
    assert_eq!(reply.status, 401);
    // A view without its stretch, with more per page than there is, or of a property nobody has.
    for (query, code) in [
        ("", "ErrorInvalidParameter"),
        ("startDateTime=2026-10-01T00:00:00Z", "ErrorInvalidParameter"),
        ("startDateTime=2026-10-01T00:00:00Z&endDateTime=2026-11-01T00:00:00Z&%24top=5000", "ErrorInvalidRequest"),
        ("startDateTime=2026-10-01T00:00:00Z&endDateTime=2026-11-01T00:00:00Z&%24select=id,title", "BadRequest"),
    ] {
        assert_eq!(said("GET", &format!("{CALENDAR_VIEW}?{query}"), &[], None), (400, code.to_string()), "{query}");
    }
    // And what this crate asks for is all of it something an event has.
    for fields in [EVENT_FIELDS_FULL, EVENT_FIELDS, EVENT_FIELDS_PLAIN, SERIES_FIELDS] {
        let query = http::form(&[("startDateTime", "2026-10-01T00:00:00Z"), ("endDateTime", "2026-11-01T00:00:00Z"), ("$select", fields)]);
        assert_eq!(said("GET", &format!("{CALENDAR_VIEW}?{query}"), &[], None).0, 200, "{fields}");
    }
}

#[test]
fn a_view_asked_for_right_after_a_change_may_be_the_older_one() {
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let id = server.state().add_event(&main, "Budget review", at(2026, 10, 12, 13, 0));
    let start_of = |down: &EventsDownload| down.events.iter().find(|e| e.id == id).map(|e| (e.start, e.change_key.clone()));
    let before = start_of(&october(&server, "")).unwrap();
    server.state().stale_views = 1;
    let moved = done(&server, update(&id, EventFields { time: timed(at(2026, 10, 13, 13, 0), 60), ..Default::default() }), EASTERN);
    // The first view after the change is as it was before it: the old time, under the old mark.
    assert_eq!(start_of(&october(&server, "")), Some(before.clone()));
    assert!(moved.change_key != before.1);
    // The next one has caught up, and carries the mark the change was answered with.
    assert_eq!(start_of(&october(&server, "")), Some((at(2026, 10, 13, 13, 0), moved.change_key.clone())));
    // The same for an event that was just made, and for one that was just deleted.
    server.state().stale_views = 1;
    let made = done(&server, create(MARK, appointment("Lunch", at(2026, 10, 15, 16, 0), 30)), EASTERN);
    assert!(october(&server, "").events.iter().all(|e| e.id != made.remote));
    assert!(october(&server, "").events.iter().any(|e| e.id == made.remote));
    server.state().stale_views = 1;
    done(&server, delete(&made.remote), EASTERN);
    assert!(october(&server, "").events.iter().any(|e| e.id == made.remote));
    assert!(october(&server, "").events.iter().all(|e| e.id != made.remote));
}

#[test]
fn a_long_view_comes_in_pages_and_a_series_is_asked_for_in_a_batch() {
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let ids: Vec<String> = (0..25u8).map(|n| server.state().add_event(&main, &format!("Event {n}"), at(2026, 10, 1 + n, 13, 0))).collect();
    let down = october(&server, "");
    assert!(down.complete);
    assert_eq!(down.events.iter().map(|e| e.id.clone()).collect::<Vec<_>>(), ids);
    // Fifty to a page were asked for: one page.
    let views = server.state().requests.iter().filter(|asked| asked.path.last().is_some_and(|last| last == "calendarView")).count();
    assert_eq!(views, 1);
    assert_eq!(server.state().requests.first().map(|asked| asked.header("prefer").to_string()), Some(EVENT_PREFERENCES.to_string()));
}

/// How far east of UTC New York is, as the stand-in counts it.
fn eastern(instant: Timestamp) -> i32 {
    zone_offset(EASTERN, instant).unwrap()
}

#[test]
fn the_connection_check_makes_reads_changes_reads_and_deletes_one_appointment() {
    let server = StandIn::start().unwrap();
    let main = server.state().main_calendar();
    let kept = server.state().add_event(&main, "Budget review", at(2026, 10, 12, 13, 0));
    let now = at(2026, 10, 12, 8, 30);
    let check = check_calendar_at(&server.url(), TOKEN, now, EASTERN, &eastern);
    assert!(check.ok() && !check.token_refused && check.cleaned_up, "{check:?}");
    // A year from today at noon UTC.
    assert_eq!((check.start, check_start(now)), (at(2027, 10, 12, 12, 0), at(2027, 10, 12, 12, 0)));
    let said: Vec<(&str, bool, u16, &str)> = check.steps.iter().map(|step| (step.what, step.ok, step.status, step.said.as_str())).collect();
    assert_eq!(
        said,
        [
            ("Make an appointment", true, 201, "sent: 2027-10-12 12:00:00 UTC to 2027-10-12 12:30:00 UTC"),
            ("Read it back", true, 200, "the server has: 2027-10-12 12:00:00 UTC to 2027-10-12 12:30:00 UTC, as it was sent"),
            ("Change its time", true, 200, "sent: 2027-10-12 13:00:00 UTC to 2027-10-12 13:30:00 UTC"),
            ("Read it back again", true, 200, "the server has: 2027-10-12 13:00:00 UTC to 2027-10-12 13:30:00 UTC, as it was sent"),
            (
                "Change its time once more, said in your time zone",
                true,
                200,
                "sent: 2027-10-12 10:00:00 to 2027-10-12 10:30:00 in the zone \"Eastern Standard Time\", which is 2027-10-12 14:00:00 UTC to 2027-10-12 14:30:00 UTC here"
            ),
            ("Read it back a third time", true, 200, "the server has: 2027-10-12 14:00:00 UTC to 2027-10-12 14:30:00 UTC, as it was sent"),
            ("Delete it", true, 204, "deleted"),
        ]
    );
    // Seven requests, all about the one appointment it made; nothing else was read or touched.
    let asked: Vec<(String, usize)> = server.state().requests.iter().map(|asked| (asked.method.clone(), asked.path.len())).collect();
    let expected = [("POST", 3), ("GET", 4), ("PATCH", 4), ("GET", 4), ("PATCH", 4), ("GET", 4), ("DELETE", 4)];
    assert_eq!(asked, expected.map(|(method, pieces)| (method.to_string(), pieces)));
    let made = server.state().requests.first().map(|asked| asked.json()).unwrap();
    assert_eq!((made["subject"].as_str(), made["showAs"].as_str(), made["isReminderOn"].as_bool()), (Some(CHECK_TITLE), Some("free"), Some(false)));
    assert!(made.get("attendees").is_none());
    let events: Vec<Stored> = server.state().events.clone();
    assert_eq!(events.iter().map(|e| (e.id.as_str(), e.start, e.version)).collect::<Vec<_>>(), [(kept.as_str(), at(2026, 10, 12, 13, 0), 1)]);
    // With no zone to say a time in, that step and its reading are left out.
    let check = check_calendar_at(&server.url(), TOKEN, now, "", &eastern);
    assert!(check.ok());
    assert_eq!(check.steps.iter().map(|step| step.what).collect::<Vec<_>>(), [STEPS[0], STEPS[1], STEPS[2], STEPS[3], STEPS[6]]);

    // A server that says yes to a change and keeps the time it had: every reading after says
    // what was sent and what the server has instead, and what was made is still deleted.
    server.state().keeps_times = true;
    let check = check_calendar_at(&server.url(), TOKEN, now, EASTERN, &eastern);
    assert!(!check.ok() && check.cleaned_up);
    let said: Vec<(bool, &str)> = check.steps.iter().map(|step| (step.ok, step.said.as_str())).collect();
    assert_eq!(
        (said[2], said[3], said[5], said[6]),
        (
            (true, "sent: 2027-10-12 13:00:00 UTC to 2027-10-12 13:30:00 UTC"),
            (
                false,
                "sent: 2027-10-12 13:00:00 UTC to 2027-10-12 13:30:00 UTC; the server has: 2027-10-12 12:00:00 UTC to 2027-10-12 12:30:00 UTC (the start and the end differ)"
            ),
            (
                false,
                "sent: 2027-10-12 14:00:00 UTC to 2027-10-12 14:30:00 UTC; the server has: 2027-10-12 12:00:00 UTC to 2027-10-12 12:30:00 UTC (the start and the end differ)"
            ),
            (true, "deleted"),
        )
    );
    assert_eq!(server.state().events.len(), 1);
    server.state().keeps_times = false;

    // A zone that the server counts otherwise than this program does (here: no daylight saving
    // on this side): the time said in it arrives an hour off, and the check says so.
    let check = check_calendar_at(&server.url(), TOKEN, now, EASTERN, &|_| -300);
    assert!(!check.ok());
    assert_eq!(check.steps.iter().map(|step| step.ok).collect::<Vec<_>>(), [true, true, true, true, true, false, true]);
    assert_eq!(
        check.steps[5].said,
        "sent: 2027-10-12 14:00:00 UTC to 2027-10-12 14:30:00 UTC; the server has: 2027-10-12 13:00:00 UTC to 2027-10-12 13:30:00 UTC (the start and the end differ)"
    );
    // A zone the server doesn't know: it says so, and the rest still goes.
    let check = check_calendar_at(&server.url(), TOKEN, now, "Mars/Olympus", &|_| 0);
    assert_eq!((check.steps[4].ok, check.steps[4].status), (false, 400));
    assert!(check.steps[4].said.starts_with("TimeZoneNotSupportedException: A valid TimeZone value must be specified."), "{}", check.steps[4].said);
    assert!(check.cleaned_up && server.state().events.len() == 1);

    // A calendar that may not be written to: the first step says what the server said, and
    // there is nothing to read back or to delete.
    server.state().calendars[0].can_edit = false;
    let check = check_calendar_at(&server.url(), TOKEN, now, EASTERN, &eastern);
    let said: Vec<(&str, bool, u16, &str)> = check.steps.iter().map(|step| (step.what, step.ok, step.status, step.said.as_str())).collect();
    assert_eq!(said, [("Make an appointment", false, 403, "ErrorAccessDenied: Access is denied. Check credentials and try again.")]);
    assert!(check.cleaned_up && !check.ok());
    server.state().calendars[0].can_edit = true;
    // A sign-in the server doesn't take: said, and a new one is asked for.
    let check = check_calendar_at(&server.url(), "another.token-0123456789", now, EASTERN, &eastern);
    assert!(check.token_refused && !check.ok());
    assert_eq!(
        check.steps.iter().map(|step| (step.status, step.said.as_str())).collect::<Vec<_>>(),
        [(401, "InvalidAuthenticationToken: Access token validation failure.")]
    );
    // No server at all.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let check = check_calendar_at(&format!("http://127.0.0.1:{port}"), TOKEN, now, EASTERN, &eastern);
    assert_eq!((check.steps.len(), check.steps[0].ok, check.steps[0].status), (1, false, 0));
    assert!(!check.steps[0].said.is_empty() && !check.steps[0].said.contains(TOKEN));
    // Nothing of the token in what a careless server's words are shown as.
    let careless = json!({"error": {"code": "Nope", "message": format!("the token {TOKEN} is no good")}}).to_string();
    assert_eq!(super::check::refusal_words(careless.as_bytes(), TOKEN), "Nope");
}

#[test]
fn the_zones_of_the_stand_in_keep_the_clock() {
    // New York in 2026: four hours behind from March 8th, 07:00 UTC, to November 1st, 06:00 UTC.
    for (instant, minutes) in
        [(at(2026, 3, 8, 6, 59), -300), (at(2026, 3, 8, 7, 0), -240), (at(2026, 11, 1, 5, 59), -240), (at(2026, 11, 1, 6, 0), -300)]
    {
        assert_eq!(zone_offset(EASTERN, instant), Some(minutes));
        assert_eq!(zone_offset("America/New_York", instant), Some(minutes));
    }
    // Berlin: an hour ahead, two from the last Sunday of March to the last of October, 01:00 UTC.
    assert_eq!(zone_offset("Europe/Berlin", at(2026, 3, 29, 0, 59)), Some(60));
    assert_eq!(zone_offset("Europe/Berlin", at(2026, 3, 29, 1, 0)), Some(120));
    assert_eq!(zone_offset("W. Europe Standard Time", at(2026, 10, 25, 1, 0)), Some(60));
    assert_eq!((zone_offset("UTC", 0), zone_offset("Mars/Olympus", 0)), (Some(0), None));
}

#[test]
fn a_meeting_is_made_online_answered_with_another_time_and_forwarded_as_the_service_takes_it() {
    let server = StandIn::start().unwrap();
    let people = vec![Attendee { address: Address::new("Bo", "bo@example.org"), ..Default::default() }];
    // A meeting with everything this crate says of one: how much it matters, what the people
    // invited are asked for, and that it is held online. The answer has the link to join by.
    let fields = EventFields {
        attendees: Some(people.clone()),
        importance: Some(Importance::High),
        request_responses: Some(false),
        allow_proposals: Some(false),
        online: Some(OnlineProvider::Teams),
        ..appointment("Planning", at(2026, 10, 15, 14, 0), 60)
    };
    let made = done(&server, create(MARK, fields), EASTERN);
    assert_eq!(made.online, Some(true));
    assert!(made.join_url.starts_with("https://teams.example.org/l/meetup-join/"), "{}", made.join_url);
    let have = server.state().event("Planning").cloned().unwrap();
    assert_eq!(
        (have.importance.as_str(), have.no_response_requested, have.no_new_times, have.online, have.online_provider.as_str()),
        ("high", true, true, true, "teamsForBusiness")
    );
    assert_eq!(server.state().mail, ["invitation:Planning:1"]);
    // It comes down as it went up.
    let down = october(&server, "");
    let event = down.events.iter().find(|event| event.id == made.remote).expect("the meeting comes down under its id");
    assert!(down.has_options);
    assert_eq!((event.importance, event.options), (Importance::High, ResponseOptions { request_responses: false, allow_proposals: false }));
    assert_eq!(
        event.online.as_ref().map(|online| (online.provider, online.join_url.as_str())),
        Some((OnlineProvider::Teams, made.join_url.as_str()))
    );
    // How much it matters and whether answers are asked for are changed like anything else; and
    // an online meeting stays one, whatever is said about that afterwards.
    let quieter = EventFields { importance: Some(Importance::Low), request_responses: Some(true), ..Default::default() };
    done(&server, update(&made.remote, quieter), EASTERN);
    let have = server.state().event("Planning").cloned().unwrap();
    assert_eq!(
        (have.importance.as_str(), have.no_response_requested, have.online, have.join_url.as_str()),
        ("low", false, true, made.join_url.as_str())
    );
    // Whether another time may be proposed is not among what the reference lets an update say:
    // the service refuses the change. It is asked once more without that, so what was changed
    // with it holds, and the outcome says what was left out.
    let both = EventFields { title: Some("Planning day".into()), allow_proposals: Some(true), ..Default::default() };
    let outcome = push(&server, vec![update(&made.remote, both)], EASTERN).remove(0);
    assert_eq!((outcome.result, outcome.status, outcome.new_times), (EventResult::Done, 200, Some(false)));
    let have = server.state().event("Planning day").cloned().unwrap();
    assert!(have.no_new_times && have.id == made.remote);
    assert_eq!(refusals(&server).len(), 1);
    assert_eq!(server.state().mail.iter().filter(|line| line.starts_with("update:Planning day")).count(), 1, "one update, for what was taken");
    // A service that lets it be said takes it as it is.
    server.state().updates_new_times = true;
    let outcome = done(&server, update(&made.remote, EventFields { allow_proposals: Some(true), ..Default::default() }), EASTERN);
    assert_eq!((outcome.new_times, server.state().event("Planning day").map(|event| event.no_new_times)), (None, Some(false)));
    assert_eq!(refusals(&server).len(), 1);
    server.state().updates_new_times = false;

    // A calendar whose meetings can't be held online. The reference doesn't say what the
    // service makes of being asked all the same. Should it refuse, the event is made without,
    // and the outcome says so; should it make the event and leave the online meeting out
    // without a word, the outcome says so too.
    let plain = server.state().add_calendar("Plain", true, false);
    server.state().calendars.iter_mut().find(|calendar| calendar.id == plain).unwrap().online.clear();
    let offline = |title: &str, mark: &str| EventOp {
        calendar: plain.clone(),
        ..create(mark, EventFields { online: Some(OnlineProvider::Teams), ..appointment(title, at(2026, 10, 16, 14, 0), 60) })
    };
    let refused = done(&server, offline("Offline", "11111111-d9cb-469f-a165-70867728950e"), EASTERN);
    assert_eq!((refused.online, refused.join_url.as_str(), refusals(&server).len()), (Some(false), "", 2));
    assert_eq!(server.state().event("Offline").map(|event| (event.online, event.calendar.clone())), Some((false, plain.clone())));
    server.state().drops_online = true;
    let dropped = done(&server, offline("Quiet", "22222222-d9cb-469f-a165-70867728950e"), EASTERN);
    assert_eq!((dropped.online, dropped.join_url.as_str(), refusals(&server).len()), (Some(false), "", 2));

    // Somebody else's meeting is answered tentatively, with another time: the organizer hears
    // the answer and the proposal.
    let board = {
        let mut state = server.state();
        let main = state.main_calendar();
        let id = state.add_event(&main, "Board", at(2026, 10, 20, 14, 0));
        let event = state.events.iter_mut().find(|event| event.id == id).unwrap();
        (event.is_organizer, event.response) = (false, "none".into());
        id
    };
    let proposal = EventTime { start: at(2026, 10, 20, 15, 0), end: at(2026, 10, 20, 16, 0), all_day: false };
    let answer = |response: Response, send: bool| EventOp {
        kind: EventChangeKind::Respond,
        event: 5,
        remote: board.clone(),
        response,
        send,
        comment: "Later?".into(),
        proposed: Some(proposal),
        ..Default::default()
    };
    let said = |server: &StandIn| server.state().requests.last().map(|asked| (asked.path.last().cloned().unwrap_or_default(), asked.json())).unwrap();
    server.state().mail.clear();
    done(&server, answer(Response::Tentative, true), EASTERN);
    let slot =
        json!({"start": {"dateTime": "2026-10-20T15:00:00", "timeZone": "UTC"}, "end": {"dateTime": "2026-10-20T16:00:00", "timeZone": "UTC"}});
    assert_eq!(said(&server), ("tentativelyAccept".to_string(), json!({"comment": "Later?", "sendResponse": true, "proposedNewTime": slot})));
    assert_eq!(server.state().mail, ["response:Board:tentativelyAccept:Later?", "proposal:Board:2026-10-20T15:00:00Z/2026-10-20T16:00:00Z"]);
    // An answer that isn't sent proposes nothing, and neither does a yes: the service would
    // refuse the one and has no such parameter for the other, so neither is asked of it.
    done(&server, answer(Response::Tentative, false), EASTERN);
    assert_eq!(said(&server), ("tentativelyAccept".to_string(), json!({"comment": "", "sendResponse": false})));
    done(&server, answer(Response::Accepted, true), EASTERN);
    assert_eq!(said(&server), ("accept".to_string(), json!({"comment": "Later?", "sendResponse": true})));
    // An organizer who takes no proposals: the service refuses the answer, and the meeting is
    // not declined away by it.
    server.state().events.iter_mut().find(|event| event.id == board).unwrap().no_new_times = true;
    let outcome = push(&server, vec![answer(Response::Declined, true)], EASTERN).remove(0);
    assert_eq!((outcome.result, outcome.status), (EventResult::Refused, 400));
    assert!(server.state().event("Board").is_some());

    // Sent on to two people, with a note: the action, its two parameters, and 202.
    let to = vec![Address::new("Eve", "eve@example.org"), Address::new("", "dan@example.org")];
    let forward = |remote: &str, calendar: &str, send: bool| EventOp {
        kind: EventChangeKind::Forward,
        event: 6,
        remote: remote.into(),
        token: MARK.into(),
        calendar: calendar.into(),
        send,
        comment: "Do come.".into(),
        to: to.clone(),
        ..Default::default()
    };
    server.state().mail.clear();
    let sent = done(&server, forward(&made.remote, "", true), EASTERN);
    assert_eq!(sent.status, 202);
    let recipients = json!([
        {"EmailAddress": {"Address": "eve@example.org", "Name": "Eve"}},
        {"EmailAddress": {"Address": "dan@example.org", "Name": ""}},
    ]);
    assert_eq!(said(&server), ("forward".to_string(), json!({"ToRecipients": recipients, "Comment": "Do come."})));
    assert_eq!(server.state().mail, ["forward:Planning day:2:Do come."]);
    // One of another calendar goes through that calendar.
    done(&server, forward(&refused.remote, &plain, true), EASTERN);
    let through = server.state().requests.last().map(|asked| asked.path.clone()).unwrap();
    assert_eq!(through, ["v1.0", "me", "calendars", plain.as_str(), "events", refused.remote.as_str(), "forward"]);
    // Without the owner's yes the service is not asked at all.
    let asked = server.state().requests.len();
    let outcome = push(&server, vec![forward(&made.remote, "", false)], EASTERN).remove(0);
    assert_eq!((outcome.result, outcome.status, server.state().requests.len()), (EventResult::Refused, 0, asked));
    assert_eq!(server.state().mail.len(), 2);
}

/// An event with zones of its own, as the mailbox sends one.
fn in_zones(fields: EventFields, start: &str, end: &str) -> EventFields {
    EventFields { zones: Some(mailcraft_model::remote::EventZones { start: start.into(), end: end.into() }), ..fields }
}

#[test]
fn the_stand_in_takes_the_zone_names_the_reference_lists_and_no_others() {
    // The names Windows has, and the additional ones of the reference.
    for name in ["UTC", "AUS Eastern Standard Time", "Nepal Standard Time", "Australia/Sydney", "Asia/Kathmandu", "Pacific/Apia", "Europe/Kyiv"] {
        assert!(zone_offset(name, 0).is_some(), "{name}");
    }
    // Any other name of the IANA database, an offset, another spelling, nonsense: not documented.
    for name in
        ["Europe/Amsterdam", "America/Detroit", "UTC+05:30", "+02:00", "eastern standard time", "Mars/Olympus", "", "tzone://Microsoft/Custom"]
    {
        assert!(zone_offset(name, 0).is_none(), "{name}");
    }
    assert!(zone_offset(&"A".repeat(1_000_000), 0).is_none());
    // Both tables say the same of the zones that are written out here: around both changes of
    // the clock in both years, and in the middle of each season.
    for (windows, iana) in
        [("Eastern Standard Time", "America/New_York"), ("W. Europe Standard Time", "Europe/Berlin"), ("GMT Standard Time", "Europe/London")]
    {
        let known = mailcraft_time::Zone::named(iana).unwrap();
        let mut instant = at(2026, 1, 1, 0, 0);
        while instant < at(2028, 1, 1, 0, 0) {
            assert_eq!(zone_offset(windows, instant), Some(known.offset_at(instant)), "{windows} at {instant}");
            instant += 1_800;
        }
    }
    // Half and three quarters of an hour, and the south.
    assert_eq!(zone_offset("Nepal Standard Time", at(2026, 10, 15, 0, 0)), Some(345));
    assert_eq!(
        (zone_offset("Australia/Sydney", at(2026, 7, 1, 0, 0)), zone_offset("Australia/Sydney", at(2026, 12, 1, 0, 0))),
        (Some(600), Some(660))
    );
}

#[test]
fn an_event_in_zones_of_its_own_is_kept_in_them_at_the_same_moment() {
    let server = StandIn::start().unwrap();
    // A call at half past six in the evening in Pune: one o'clock in UTC, nine in New York.
    let call = in_zones(appointment("Call with Pune", at(2026, 10, 15, 13, 0), 60), "Asia/Kolkata", "");
    let made = done(&server, create(MARK, call), EASTERN);
    assert_eq!(made.zone, "India Standard Time");
    let sent = server.state().requests.last().unwrap().json();
    assert_eq!(sent["start"], json!({"dateTime": "2026-10-15T18:30:00", "timeZone": "India Standard Time"}));
    assert_eq!(sent["end"], json!({"dateTime": "2026-10-15T19:30:00", "timeZone": "India Standard Time"}));
    let have = server.state().event("Call with Pune").cloned().unwrap();
    assert_eq!(
        (have.start, have.end, have.zone.as_str(), have.end_zone()),
        (at(2026, 10, 15, 13, 0), at(2026, 10, 15, 14, 0), "India Standard Time", "India Standard Time")
    );
    // A flight that leaves London at ten and lands in Berlin at one: each end by its own clock.
    let flight = in_zones(appointment("Flight", at(2026, 10, 16, 9, 0), 120), "Europe/London", "Europe/Berlin");
    done(&server, create("22222222-d9cb-469f-a165-70867728950e", flight), EASTERN);
    let sent = server.state().requests.last().unwrap().json();
    assert_eq!(sent["start"], json!({"dateTime": "2026-10-16T10:00:00", "timeZone": "GMT Standard Time"}));
    assert_eq!(sent["end"], json!({"dateTime": "2026-10-16T13:00:00", "timeZone": "W. Europe Standard Time"}));
    let have = server.state().event("Flight").cloned().unwrap();
    assert_eq!(
        (have.start, have.end, have.zone.as_str(), have.end_zone()),
        (at(2026, 10, 16, 9, 0), at(2026, 10, 16, 11, 0), "GMT Standard Time", "W. Europe Standard Time")
    );

    // What comes down: the same moments, and the zones they were told in, as the service names them.
    let down = october(&server, "");
    let of = |title: &str| down.events.iter().find(|event| event.title == title).unwrap();
    assert_eq!(
        (of("Call with Pune").start, of("Call with Pune").start_zone.as_str(), of("Call with Pune").end_zone.as_str()),
        (at(2026, 10, 15, 13, 0), "India Standard Time", "India Standard Time")
    );
    assert_eq!((of("Flight").start, of("Flight").end), (at(2026, 10, 16, 9, 0), at(2026, 10, 16, 11, 0)));
    assert_eq!((of("Flight").start_zone.as_str(), of("Flight").end_zone.as_str()), ("GMT Standard Time", "W. Europe Standard Time"));

    // Moved to another zone (the clock time stays, the moment moves): the zone is the event's.
    let later = in_zones(EventFields { time: timed(at(2026, 10, 15, 9, 30), 60), ..Default::default() }, "Asia/Tokyo", "");
    done(&server, update(&made.remote, later), EASTERN);
    let have = server.state().event("Call with Pune").cloned().unwrap();
    assert_eq!((have.start, have.zone.as_str(), have.end_zone()), (at(2026, 10, 15, 9, 30), "Tokyo Standard Time", "Tokyo Standard Time"));
    assert_eq!(refusals(&server), []);
}

#[test]
fn a_series_in_a_zone_of_its_own_keeps_its_hour_there_across_a_change_of_the_clock() {
    let server = StandIn::start().unwrap();
    // Mondays at nine in Berlin, from the owner's desk in New York. Europe's clocks go back on
    // 25 October, a week before New York's.
    let first = Date::new(2026, 10, 19).unwrap();
    let rule = Recurrence { by_day: vec![Weekday::Mon], ..Recurrence::every(Freq::Weekly) };
    let series = in_zones(
        EventFields {
            wall: Some(EventWall { start: clock(2026, 10, 19, 9, 0), end: clock(2026, 10, 19, 9, 30) }),
            repeat: Some(EventRepeat { rule: Some(rule.clone()), first }),
            ..appointment("Berlin stand-up", at(2026, 10, 19, 7, 0), 30)
        },
        "Europe/Berlin",
        "",
    );
    let made = done(&server, create(MARK, series), EASTERN);
    assert_eq!(made.zone, "W. Europe Standard Time");
    let sent = server.state().requests.last().unwrap().json();
    assert_eq!(sent["start"], json!({"dateTime": "2026-10-19T09:00:00", "timeZone": "W. Europe Standard Time"}));
    assert_eq!(sent["recurrence"]["range"]["startDate"], json!("2026-10-19"));
    assert_eq!(sent["recurrence"]["range"]["recurrenceTimeZone"], json!("W. Europe Standard Time"));
    let query =
        EventsQuery { calendar: String::new(), from: at(2026, 10, 18, 0, 0), to: at(2026, 11, 10, 0, 0), series: Vec::new(), shared: String::new() };
    let down = events_in_at(&server.url(), TOKEN, &query).unwrap();
    let starts: Vec<Timestamp> =
        down.events.iter().filter(|e| e.kind == RemoteEventKind::Occurrence && e.series == made.remote).map(|e| e.start).collect();
    // Seven in UTC while Berlin has summer time, eight after.
    assert_eq!(starts, [at(2026, 10, 19, 7, 0), at(2026, 10, 26, 8, 0), at(2026, 11, 2, 8, 0), at(2026, 11, 9, 8, 0)]);
    let itself = down.events.iter().find(|e| e.kind == RemoteEventKind::SeriesMaster).unwrap();
    assert_eq!((itself.start_zone.as_str(), itself.recurrence.as_ref(), itself.series_day), ("W. Europe Standard Time", Some(&rule), Some(first)));
    assert_eq!(refusals(&server), []);
}

#[test]
fn a_zone_the_service_has_no_name_for_costs_the_name_and_never_the_moment() {
    let server = StandIn::start().unwrap();
    // Amsterdam has no name of Windows' own and is not among the additional ones: the service
    // says no, and the event goes as the moment it is, in UTC.
    let visit = in_zones(appointment("Visit", at(2026, 10, 15, 8, 0), 60), "Europe/Amsterdam", "");
    let made = done(&server, create(MARK, visit), EASTERN);
    assert_eq!(made.zone, "UTC");
    let have = server.state().event("Visit").cloned().unwrap();
    assert_eq!((have.start, have.end, have.zone.as_str()), (at(2026, 10, 15, 8, 0), at(2026, 10, 15, 9, 0), "UTC"));
    assert_eq!(refusals(&server).iter().map(|(method, _, status)| (method.as_str(), *status)).collect::<Vec<_>>(), [("POST", 400)]);
    // A zone that is no zone at all, and an offset: never asked about by name.
    for (mark, zone) in [("22222222-d9cb-469f-a165-70867728950e", "Mars/Olympus"), ("33333333-d9cb-469f-a165-70867728950e", "UTC+05:30")] {
        let odd = in_zones(appointment(zone, at(2026, 10, 16, 8, 0), 60), zone, "");
        assert_eq!(done(&server, create(mark, odd), EASTERN).zone, "UTC");
        assert_eq!(server.state().event(zone).map(|e| (e.start, e.zone.clone())), Some((at(2026, 10, 16, 8, 0), "UTC".to_string())));
    }
    assert_eq!(refusals(&server).len(), 1);
    // A series by that zone's clock is not sent by another clock: in UTC it would be another
    // series half the year. It is refused, and the mailbox keeps it.
    let rule = Recurrence { by_day: vec![Weekday::Thu], ..Recurrence::every(Freq::Weekly) };
    let series = in_zones(
        EventFields {
            wall: Some(EventWall { start: clock(2026, 10, 15, 10, 0), end: clock(2026, 10, 15, 11, 0) }),
            repeat: Some(EventRepeat { rule: Some(rule), first: Date::new(2026, 10, 15).unwrap() }),
            ..appointment("Weekly visit", at(2026, 10, 15, 8, 0), 60)
        },
        "Europe/Amsterdam",
        "",
    );
    let refused = push(&server, vec![create("44444444-d9cb-469f-a165-70867728950e", series)], EASTERN).remove(0);
    assert_eq!(refused.result, EventResult::Refused);
    assert!(server.state().event("Weekly visit").is_none());
}
