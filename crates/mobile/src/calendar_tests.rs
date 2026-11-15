// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The calendar reads of the binding (calendars, one event whole, the range's new fields) and the
//! commands the Calendar tab runs through `execute`: make, change, answer, import an .ics.

use serde_json::{Value, json};

use crate::fake_tests::{block_on, sample_core};
use crate::{Core, CoreError};

fn exec(core: &Core, id: &str, params: Value) -> Result<Value, CoreError> {
    block_on(core.execute(id.to_string(), params.to_string())).map(|s| serde_json::from_str(&s).unwrap())
}

#[test]
fn the_calendars_list_with_their_account_colour_and_rights() {
    let core = sample_core();
    let list = block_on(core.calendars()).unwrap();
    assert!(!list.calendars.is_empty());
    assert!(list.calendars.iter().any(|c| c.id == list.current));
    assert!(list.calendars.iter().all(|c| !c.name.is_empty()));
    assert!(list.calendars.iter().any(|c| c.editable));
}

#[test]
fn an_event_is_read_whole_and_one_that_is_not_there_is_refused() {
    let core = sample_core();
    let week = block_on(core.calendar("2026-10-12".into(), "2026-10-18".into())).unwrap();
    let first = &week.events[0];
    let detail = block_on(core.event(first.event)).unwrap();
    assert_eq!((detail.event, detail.calendar), (first.event, first.calendar));
    assert_eq!(detail.title, first.title);
    assert_eq!(detail.rev, week.rev);
    for bad in [0, u64::MAX] {
        assert!(matches!(block_on(core.event(bad)), Err(CoreError::BadRequest { .. })));
    }
}

#[test]
fn an_event_made_by_the_editor_reads_back_with_its_recurrence_reminder_and_people() {
    let core = sample_core();
    let made = exec(
        &core,
        "calendar.new",
        json!({
            "title": "Planning", "start": "2026-11-02T09:00", "end": "2026-11-02T10:00", "location": "Room 4",
            "reminder": 15, "recurrence": {"freq": "weekly", "interval": 2, "byDay": ["mon", "wed"], "count": 6},
        }),
    )
    .unwrap();
    let id = made["id"].as_u64().unwrap();
    let e = block_on(core.event(id)).unwrap();
    assert_eq!((e.title.as_str(), e.location.as_str()), ("Planning", "Room 4"));
    assert_eq!((e.start.as_str(), e.end.as_str(), e.all_day), ("2026-11-02T09:00:00", "2026-11-02T10:00:00", false));
    assert_eq!(e.reminder_minutes, Some(15));
    let r = e.recurrence.unwrap();
    assert_eq!((r.freq.as_str(), r.interval, r.count), ("weekly", 2, Some(6)));
    assert_eq!(r.by_day, ["mon", "wed"]);
    assert!(e.editable && !e.invited && !e.meeting);
    // It shows in the range, where it repeats.
    let range = block_on(core.calendar("2026-11-01".into(), "2026-11-30".into())).unwrap();
    assert!(range.events.iter().filter(|x| x.event == id).all(|x| x.repeats && !x.invited && x.answer == "none"));
    assert!(range.events.iter().filter(|x| x.event == id).count() >= 3);
}

#[test]
fn a_whole_day_event_is_read_as_its_days_and_a_change_goes_through_the_command() {
    let core = sample_core();
    let made = exec(&core, "calendar.new", json!({"title": "Offsite", "start": "2026-11-10", "end": "2026-11-12", "allDay": true})).unwrap();
    let id = made["id"].as_u64().unwrap();
    let e = block_on(core.event(id)).unwrap();
    assert!(e.all_day);
    assert_eq!((e.start.as_str(), e.end.as_str()), ("2026-11-10", "2026-11-12"));
    exec(&core, "calendar.edit", json!({"id": id, "title": "Offsite 2", "location": "Hall"})).unwrap();
    assert_eq!(block_on(core.event(id)).unwrap().title, "Offsite 2");
    exec(&core, "calendar.delete", json!({"id": id})).unwrap();
    assert!(block_on(core.event(id)).is_err());
}

fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(A[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

const ICS: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\nBEGIN:VEVENT\r\nUID:ics-1@example.org\r\nDTSTAMP:20261001T000000Z\r\nDTSTART:20261120T100000Z\r\nDTEND:20261120T110000Z\r\nSUMMARY:Review\r\nLOCATION:Room 2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

#[test]
fn an_ics_file_is_imported_into_the_calendar_and_junk_is_refused() {
    let core = sample_core();
    let before = block_on(core.calendar("2026-11-19".into(), "2026-11-21".into())).unwrap().events.len();
    let said = exec(&core, "file.import", json!({"name": "invite.ics", "data": base64(ICS.as_bytes())})).unwrap();
    assert_eq!(said["format"], "ics");
    assert_eq!(said["events"], 1);
    let after = block_on(core.calendar("2026-11-19".into(), "2026-11-21".into())).unwrap();
    assert_eq!(after.events.len(), before + 1);
    assert!(after.events.iter().any(|e| e.title == "Review" && e.location == "Room 2"));
    // The same file again does not make a second event.
    exec(&core, "file.import", json!({"name": "invite.ics", "data": base64(ICS.as_bytes())})).unwrap();
    assert_eq!(block_on(core.calendar("2026-11-19".into(), "2026-11-21".into())).unwrap().events.len(), before + 1);
    // Data that is not base64 at all is refused outright; what is not a calendar is an error too, and
    // the calendar stays as it was.
    assert!(matches!(
        exec(&core, "file.import", json!({"name": "x.ics", "data": "!!!!"})),
        Err(CoreError::BadRequest { .. } | CoreError::Failed { .. })
    ));
    for junk in [base64(b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\n"), base64(&[0xff, 0xfe, 0, 1]), String::new()] {
        let r = exec(&core, "file.import", json!({"name": "x.ics", "data": junk}));
        assert!(!matches!(r, Err(CoreError::Internal)));
    }
    assert_eq!(block_on(core.calendar("2026-11-19".into(), "2026-11-21".into())).unwrap().events.len(), before + 1);
}

#[test]
fn the_zone_the_calendar_is_shown_in_follows_what_the_phone_says() {
    let core = sample_core();
    exec(&core, "view.timeZone", json!({"zone": "Europe/Berlin"})).unwrap();
    let made = exec(&core, "calendar.new", json!({"title": "Zoned", "start": "2026-03-29T01:30", "end": "2026-03-29T04:30"})).unwrap();
    let e = block_on(core.event(made["id"].as_u64().unwrap())).unwrap();
    // The night the clocks go forward: the times are by the clock of the zone asked for.
    assert_eq!(e.start, "2026-03-29T01:30:00");
    assert_eq!(e.end, "2026-03-29T04:30:00");
    assert!(exec(&core, "view.timeZone", json!({"zone": "Not/AZone"})).is_err());
}

/// A core whose calendar holds a meeting somebody else organizes, for which the owner is invited.
fn invited_core() -> (std::sync::Arc<Core>, u64) {
    use mailcraft_engine::{Session, sample};
    use mailcraft_model::{Address, Attendee, Event, Origin};
    use mailcraft_time::{Date, DateTime};

    let now = crate::fake_tests::now();
    let mut mailbox = sample::mailbox(now, 0);
    let me = mailbox.accounts.first().map(|a| a.email.clone()).unwrap_or_default();
    let calendar = mailbox.calendars.first().map_or(0, |c| c.id);
    let day = Date::new(2026, 10, 20).unwrap();
    let event = Event {
        calendar,
        title: "Board review".into(),
        start: DateTime::new(day, 14, 0, 0).unwrap(),
        end: DateTime::new(day, 15, 0, 0).unwrap(),
        organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
        attendees: vec![Attendee { address: Address::new("Me", &me), ..Default::default() }],
        origin: Origin { guest: true, ..Default::default() },
        ..Default::default()
    };
    let id = mailbox.add_event(event).unwrap();
    (Core::start(crate::fake_tests::FakePlatform::new(), move || Session::at(mailbox, now, 0)).unwrap(), id)
}

#[test]
fn an_invitation_shows_as_to_answer_and_each_answer_is_kept() {
    let (core, id) = invited_core();
    let day = || block_on(core.calendar("2026-10-20".into(), "2026-10-20".into())).unwrap();
    let e = day().events.into_iter().find(|e| e.event == id).unwrap();
    assert!((e.invited, e.meeting, e.answer.as_str()) == (true, true, "none"));
    let detail = block_on(core.event(id)).unwrap();
    assert!(!detail.editable && detail.invited);
    assert_eq!(detail.organizer.as_ref().map(|o| o.address.as_str()), Some("bo@example.org"));
    for (response, shown) in [("accept", "accepted"), ("tentative", "tentative"), ("decline", "declined")] {
        // A kept answer: nobody is written to until a sync sends it.
        let r = exec(&core, "calendar.respond", json!({"id": id, "response": response, "send": false}));
        assert!(!matches!(r, Err(CoreError::Internal)));
        if response != "decline" {
            assert_eq!(block_on(core.event(id)).unwrap().answer, shown);
        }
    }
}

#[test]
fn a_long_description_is_cut_for_the_editor_and_a_save_that_leaves_it_alone_keeps_it_whole() {
    let core = sample_core();
    let long = "Notes. ".repeat(4_000);
    let made = exec(&core, "calendar.new", json!({"title": "Review", "start": "2026-11-02T09:00", "description": long})).unwrap();
    let id = made["id"].as_u64().unwrap();
    let e = block_on(core.event(id)).unwrap();
    assert_eq!(e.description.chars().count(), 20_000);
    // The editor sends only what the owner changed: here the location, with no description.
    exec(&core, "calendar.edit", json!({"id": id, "allDay": false, "start": "2026-11-02T09:00", "end": "2026-11-02T09:30", "location": "Room 5"}))
        .unwrap();
    let whole = exec(&core, "calendar.get", json!({"id": id})).unwrap();
    assert_eq!(whole["description"].as_str(), Some(long.as_str()));
    assert_eq!(whole["location"].as_str(), Some("Room 5"));
}

#[test]
fn optional_people_sent_apart_stay_optional_and_a_save_without_people_keeps_them() {
    let core = sample_core();
    let made = exec(
        &core,
        "calendar.new",
        json!({"title": "Sync", "start": "2026-11-02T09:00", "attendees": "ada@example.org", "optional": "bo@example.org", "send": false}),
    )
    .unwrap();
    let id = made["id"].as_u64().unwrap();
    let roles = |core: &Core| {
        let e = block_on(core.event(id)).unwrap();
        e.attendees.iter().map(|a| (a.address.clone(), a.optional)).collect::<Vec<_>>()
    };
    let both = vec![("ada@example.org".to_string(), false), ("bo@example.org".to_string(), true)];
    assert_eq!(roles(&core), both);
    // A title-only save sends no people: their roles stay as they were.
    exec(&core, "calendar.edit", json!({"id": id, "title": "Weekly sync"})).unwrap();
    assert_eq!(roles(&core), both);
}

#[test]
fn a_list_of_people_longer_than_is_handed_over_says_so() {
    let core = sample_core();
    let few = (0..3).map(|i| format!("p{i}@example.org")).collect::<Vec<_>>().join("; ");
    let made = exec(&core, "calendar.new", json!({"title": "Few", "start": "2026-11-02T09:00", "attendees": few, "send": false})).unwrap();
    let e = block_on(core.event(made["id"].as_u64().unwrap())).unwrap();
    assert_eq!((e.attendees.len(), e.people_cut), (3, false));
    // One more than the editor is given: the list it has is not the whole list.
    let many = (0..=crate::text::MAX_PEOPLE_IN_ITEM).map(|i| format!("p{i}@example.org")).collect::<Vec<_>>().join("; ");
    let made = exec(&core, "calendar.new", json!({"title": "Many", "start": "2026-11-02T09:00", "attendees": many, "send": false})).unwrap();
    let id = made["id"].as_u64().unwrap();
    let e = block_on(core.event(id)).unwrap();
    assert_eq!((e.attendees.len(), e.people_cut), (crate::text::MAX_PEOPLE_IN_ITEM, true));
    // A save that leaves the people alone (as the editor does for a cut list) keeps all of them.
    exec(&core, "calendar.edit", json!({"id": id, "title": "Many more"})).unwrap();
    let whole = exec(&core, "calendar.get", json!({"id": id})).unwrap();
    assert_eq!(whole["attendees"].as_array().map(Vec::len), Some(crate::text::MAX_PEOPLE_IN_ITEM + 1));
}

#[test]
fn one_time_of_a_series_is_read_with_its_series_in_one_look() {
    let core = sample_core();
    let made = exec(
        &core,
        "calendar.new",
        json!({"title": "Standup", "start": "2026-11-02T09:00", "end": "2026-11-02T09:30", "recurrence": {"freq": "weekly", "count": 4}}),
    )
    .unwrap();
    let id = made["id"].as_u64().unwrap();
    let got = block_on(core.event_on(id, "2026-11-09".into())).unwrap();
    assert_eq!((got.start.as_deref(), got.end.as_deref()), (Some("2026-11-09T09:00:00"), Some("2026-11-09T09:30:00")));
    // The series baseline is the one read with it.
    assert_eq!(got.detail.start, "2026-11-02T09:00:00");
    assert_eq!(got.detail.rev, block_on(core.calendar("2026-11-09".into(), "2026-11-09".into())).unwrap().rev);
    // A day with no time of it has none; a bad id or day is refused.
    let none = block_on(core.event_on(id, "2026-11-10".into())).unwrap();
    assert_eq!((none.start, none.end), (None, None));
    assert!(matches!(block_on(core.event_on(u64::MAX, "2026-11-09".into())), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.event_on(id, "nonsense".into())), Err(CoreError::BadRequest { .. })));
}

#[test]
fn a_day_whose_time_of_a_long_series_is_skipped_has_no_time_not_the_one_before() {
    let core = sample_core();
    // Each time runs 36 hours, so the time of the 3rd still overlaps the 4th.
    let made = exec(
        &core,
        "calendar.new",
        json!({"title": "Long", "start": "2026-11-02T09:00", "end": "2026-11-03T21:00", "recurrence": {"freq": "daily", "count": 5}}),
    )
    .unwrap();
    let id = made["id"].as_u64().unwrap();
    let on = block_on(core.event_on(id, "2026-11-04".into())).unwrap();
    assert_eq!(on.start.as_deref(), Some("2026-11-04T09:00:00"));
    exec(&core, "calendar.delete", json!({"id": id, "scope": "occurrence", "occurrence": "2026-11-04"})).unwrap();
    let gone = block_on(core.event_on(id, "2026-11-04".into())).unwrap();
    assert_eq!((gone.start, gone.end), (None, None));
    let next = block_on(core.event_on(id, "2026-11-05".into())).unwrap();
    assert_eq!(next.start.as_deref(), Some("2026-11-05T09:00:00"));
}
