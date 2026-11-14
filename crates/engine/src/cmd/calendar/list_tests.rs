// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `calendar.list` by `calendars`: the calendars named, shown or not, each occurrence with its
//! calendar, UID, private, cancelled and the owner's own answer; anything that isn't a list of
//! calendar ids is refused, never taken for the calendars shown. Invented people at `.example`.

use mailcraft_model::{Address, Attendee, Event, Freq, Id, Recurrence, Response};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::{Session, sample};

/// The sample mailbox on Wednesday 2026-10-14, 16:00 UTC.
fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn at(day: u8, hour: u8) -> DateTime {
    DateTime::new(Date::new(2026, 10, day).unwrap(), hour, 0, 0).unwrap()
}

/// A calendar of the test's own, shown or hidden.
fn calendar(s: &mut Session, name: &str, visible: bool) -> Id {
    let id = run(s, "calendar.addCalendar", json!({"name": name}))["id"].as_u64().unwrap();
    for c in &mut s.mailbox.calendars {
        if c.id == id {
            c.visible = visible;
        }
    }
    id
}

fn add(s: &mut Session, calendar: Id, title: &str, day: u8) -> Id {
    s.mailbox.add_event(Event { calendar, title: title.into(), start: at(day, 9), end: at(day, 10), ..Default::default() }).unwrap()
}

fn titles(listed: &Value) -> Vec<String> {
    listed["events"].as_array().unwrap().iter().map(|e| e["title"].as_str().unwrap().to_owned()).collect()
}

#[test]
fn a_hidden_calendar_is_listed_when_named_and_only_the_calendars_named_are() {
    let mut s = session();
    let hidden = calendar(&mut s, "Work", false);
    let shown = calendar(&mut s, "Home", true);
    add(&mut s, hidden, "Hidden one", 20);
    add(&mut s, shown, "Shown one", 20);

    let by_default = run(&mut s, "calendar.list", json!({"from": "2026-10-19", "to": "2026-10-23"}));
    assert!(!titles(&by_default).contains(&"Hidden one".to_owned()), "a hidden calendar stays out by default");
    assert!(titles(&by_default).contains(&"Shown one".to_owned()));

    let named = run(&mut s, "calendar.list", json!({"from": "2026-10-19", "to": "2026-10-23", "calendars": [hidden]}));
    assert_eq!(titles(&named), ["Hidden one"]);
    assert_eq!(named["events"][0]["calendar"], json!(hidden));

    // One named twice counts once.
    let twice = run(&mut s, "calendar.list", json!({"from": "2026-10-19", "to": "2026-10-23", "calendars": [hidden, hidden, shown]}));
    assert_eq!(titles(&twice), ["Hidden one", "Shown one"]);
}

#[test]
fn each_occurrence_of_a_series_comes_with_its_calendar_uid_and_marks() {
    let mut s = session();
    let work = calendar(&mut s, "Work", false);
    let series = s
        .mailbox
        .add_event(Event {
            calendar: work,
            uid: "series-1@example".into(),
            title: "Stand-up".into(),
            start: at(19, 9),
            end: at(19, 10),
            recurrence: Some(Recurrence { freq: Freq::Daily, ..Default::default() }),
            private: true,
            ..Default::default()
        })
        .unwrap();
    let listed = run(&mut s, "calendar.list", json!({"from": "2026-10-19", "to": "2026-10-21", "calendars": [work]}));
    let events = listed["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    for (e, day) in events.iter().zip(["2026-10-19", "2026-10-20", "2026-10-21"]) {
        assert_eq!(e["id"], json!(series));
        assert_eq!(e["calendar"], json!(work));
        assert_eq!(e["uid"], "series-1@example");
        assert_eq!(e["private"], true);
        assert_eq!(e["cancelled"], false);
        assert_eq!(e["response"], Value::Null, "no people invited: no answer");
        assert!(e["start"].as_str().unwrap().starts_with(day));
    }
}

#[test]
fn the_owners_own_answer_and_a_cancellation_are_said() {
    let mut s = session();
    let work = calendar(&mut s, "Work", false);
    let me = s.mailbox.accounts[0].email.clone();
    let meeting = |title: &str, response: Response, cancelled: bool, day: u8| Event {
        calendar: work,
        title: title.into(),
        start: at(day, 9),
        end: at(day, 10),
        cancelled,
        organizer: Some(Address::new("Ann", "ann@example.example")),
        attendees: vec![
            Attendee { address: Address::new("Me", me.to_uppercase()), optional: false, response },
            Attendee { address: Address::new("Bob", "bob@example.example"), optional: false, response: Response::Accepted },
        ],
        ..Default::default()
    };
    s.mailbox.add_event(meeting("Declined", Response::Declined, false, 20)).unwrap();
    s.mailbox.add_event(meeting("Tentative", Response::Tentative, false, 21)).unwrap();
    s.mailbox.add_event(meeting("Called off", Response::Accepted, true, 22)).unwrap();
    let listed = run(&mut s, "calendar.list", json!({"from": "2026-10-19", "to": "2026-10-23", "calendars": [work]}));
    let events = listed["events"].as_array().unwrap();
    assert_eq!(events[0]["response"], "declined");
    assert_eq!(events[1]["response"], "tentative");
    assert_eq!(events[2]["response"], "accepted");
    assert_eq!(events[2]["cancelled"], true);
}

#[test]
fn anything_but_a_list_of_calendar_ids_is_refused() {
    let mut s = session();
    let work = calendar(&mut s, "Work", false);
    let too_many: Vec<Id> = vec![work; 65];
    for wrong in
        [json!(null), json!([]), json!(work), json!([work.to_string()]), json!([0]), json!([1.5]), json!([-1]), json!(too_many), json!([987_654_321])]
    {
        let asked = json!({"from": "2026-10-19", "to": "2026-10-23", "calendars": wrong});
        assert!(s.run("calendar.list", &asked).is_err(), "{asked} must be refused");
    }
}
