// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Email Calendar, Insert › Calendar, Contact Attendees, Reply by Email, Tracking and Search
//! Calendar: a message is opened and nothing is sent; a summary says no more than the owner chose
//! and nothing of a private appointment but that the owner is busy. Invented people at `.example`.

use mailcraft_model::{Address, Attendee, Event, Id, Response, ShowAs};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::agent::{Access, Class, classify};
use crate::{CmdError, Session, sample};

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

/// A calendar of the test's own, the only one shown: the sample's appointments are hidden.
fn calendar(s: &mut Session) -> Id {
    if let Some(c) = s.mailbox.calendars.iter().find(|c| c.name == "Share tests") {
        return c.id;
    }
    let id = run(s, "calendar.addCalendar", json!({"name": "Share tests"}))["id"].as_u64().unwrap();
    for c in &mut s.mailbox.calendars {
        c.visible = c.id == id;
    }
    id
}

fn add(s: &mut Session, e: Event) -> Id {
    let calendar = calendar(s);
    s.mailbox.add_event(Event { calendar, ..e }).unwrap()
}

/// An appointment, a private one, and somebody else's meeting with three people invited, on
/// 2026-10-20.
fn week(s: &mut Session) -> (Id, Id, Id) {
    let budget = add(
        s,
        Event {
            title: "Budget review".into(),
            location: "Room 4".into(),
            description: "Bring the numbers".into(),
            start: at(20, 9),
            end: at(20, 10),
            ..Default::default()
        },
    );
    let dentist = add(
        s,
        Event { title: "Dentist".into(), location: "Harbour Street".into(), start: at(20, 11), end: at(20, 12), private: true, ..Default::default() },
    );
    let planning = add(
        s,
        Event {
            title: "Planning".into(),
            start: at(20, 14),
            end: at(20, 15),
            show_as: ShowAs::Tentative,
            organizer: Some(Address::new("Dana Fern", "dana@partner.example")),
            attendees: vec![
                Attendee { address: Address::new("Eli Moss", "eli@partner.example"), optional: false, response: Response::Accepted },
                Attendee { address: Address::new("Avery Quinn", "avery@lumen.example"), optional: false, response: Response::Tentative },
                Attendee { address: Address::new("Fay Lin", "fay@partner.example"), optional: true, response: Response::Declined },
            ],
            origin: mailcraft_model::Origin { guest: true, ..Default::default() },
            ..Default::default()
        },
    );
    (budget, dentist, planning)
}

fn body(s: &Session) -> String {
    s.compose.as_ref().unwrap().body.text().text()
}

#[test]
fn email_calendar_opens_a_message_with_the_days_and_sends_nothing() {
    let mut s = session();
    week(&mut s);
    let messages = s.mailbox.messages.len();
    let answer = run(&mut s, "calendar.email", json!({"from": "2026-10-19", "to": "2026-10-23", "detail": "full"}));
    assert_eq!(answer["events"], json!(3));
    let c = s.compose.as_ref().unwrap();
    assert!(c.subject.starts_with("Calendar: "), "{}", c.subject);
    assert!(c.to.is_empty(), "the owner says who it is for");
    let text = body(&s);
    for said in ["Budget review", "Room 4", "Bring the numbers", "Tuesday, October 20, 2026", "Tentative: Planning"] {
        assert!(text.contains(said), "{said} in {text}");
    }
    // A private appointment is the time it takes, and nothing else.
    assert!(!text.contains("Dentist") && !text.contains("Harbour"), "{text}");
    assert!(text.contains("Busy\n") || text.contains("Busy "), "{text}");
    // Nobody invited is named.
    assert!(!text.contains("Eli") && !text.contains("dana@"), "{text}");
    let c = s.compose.as_ref().unwrap();
    let ics = c.attachments.iter().find(|a| a.name == "calendar.ics").expect("the file goes along");
    let ics = String::from_utf8(ics.data.to_vec()).unwrap();
    assert!(ics.contains("BEGIN:VCALENDAR") && ics.contains("METHOD:PUBLISH") && ics.contains("SUMMARY:Budget review"), "{ics}");
    assert!(!ics.contains("Dentist") && !ics.contains("ATTENDEE") && !ics.contains("ORGANIZER"), "{ics}");
    assert_eq!(s.mailbox.messages.len(), messages, "nothing was sent or saved");
}

#[test]
fn availability_alone_is_the_default_and_limited_adds_the_titles() {
    let mut s = session();
    week(&mut s);
    run(&mut s, "calendar.email", json!({"from": "2026-10-20", "to": "2026-10-20", "ics": false}));
    let text = body(&s);
    assert!(!text.contains("Budget") && !text.contains("Planning"), "{text}");
    assert!(text.contains("Busy") && text.contains("Tentative"), "{text}");
    assert!(s.compose.as_ref().unwrap().attachments.is_empty());
    run(&mut s, "calendar.email", json!({"from": "2026-10-20", "to": "2026-10-20", "detail": "limited", "discard": true}));
    let text = body(&s);
    assert!(text.contains("Budget review") && !text.contains("Room 4") && !text.contains("Dentist"), "{text}");
}

#[test]
fn days_with_nothing_say_so() {
    let mut s = session();
    week(&mut s);
    run(&mut s, "calendar.email", json!({"from": "2026-12-01", "to": "2026-12-02"}));
    assert!(body(&s).contains("Nothing is scheduled"), "{}", body(&s));
}

#[test]
fn insert_calendar_puts_the_summary_where_the_caret_is() {
    let mut s = session();
    week(&mut s);
    assert!(s.run("insert.calendar", &json!({})).is_err(), "nothing is being written");
    run(&mut s, "mail.new", json!({"to": "gil@partner.example", "subject": "Next week"}));
    run(&mut s, "compose.type", json!({"text": "When suits you?\n"}));
    let answer = run(&mut s, "insert.calendar", json!({"from": "2026-10-20", "to": "2026-10-20", "detail": "limited"}));
    assert_eq!(answer["events"], json!(3));
    let text = body(&s);
    let asked = text.find("When suits you?").unwrap();
    let summary = text.find("Budget review").unwrap();
    assert!(asked < summary, "{text}");
    assert_eq!(s.compose.as_ref().unwrap().to, "gil@partner.example");
}

#[test]
fn a_summary_refuses_what_it_can_not_be() {
    let mut s = session();
    for params in [
        json!({"from": "2026-10-20", "to": "2026-10-19"}),
        json!({"from": "2026-01-01", "to": "2027-01-02"}),
        json!({"detail": "everything"}),
        json!({"detail": 3}),
        json!({"calendar": 999_999}),
        json!({"calendar": "1"}),
        json!({"ics": "yes"}),
        json!({"to": "someone@partner.example"}),
        json!({"send": true}),
    ] {
        assert!(s.run("calendar.email", &params).is_err(), "{params}");
        assert!(s.compose.is_none(), "{params}: nothing opened");
    }
}

#[test]
fn contact_attendees_writes_to_everyone_but_the_owner() {
    let mut s = session();
    let (budget, _, planning) = week(&mut s);
    assert!(matches!(s.run("calendar.contactAttendees", &json!({"id": budget})), Err(CmdError::Failed(_) | CmdError::Said(_))));
    let messages = s.mailbox.messages.len();
    let answer = run(&mut s, "calendar.contactAttendees", json!({"id": planning}));
    assert_eq!(answer["to"], json!(3));
    let c = s.compose.as_ref().unwrap();
    assert_eq!(c.subject, "Planning");
    for who in ["dana@partner.example", "eli@partner.example", "fay@partner.example"] {
        assert!(c.to.contains(who), "{who} in {}", c.to);
    }
    assert!(!c.to.contains("avery@"), "{}", c.to);
    assert_eq!(s.mailbox.messages.len(), messages);
}

#[test]
fn reply_by_email_writes_to_the_organizer_and_with_all_to_everyone() {
    let mut s = session();
    let (budget, _, planning) = week(&mut s);
    assert!(s.run("calendar.respondEmail", &json!({"id": budget})).is_err(), "no organizer to write to");
    run(&mut s, "calendar.respondEmail", json!({"id": planning}));
    let c = s.compose.as_ref().unwrap();
    assert_eq!((c.subject.as_str(), c.to.as_str()), ("RE: Planning", "Dana Fern <dana@partner.example>"));
    run(&mut s, "calendar.respondEmail", json!({"id": planning, "all": true}));
    let c = s.compose.as_ref().unwrap();
    assert!(c.to.contains("dana@") && c.to.contains("eli@") && c.to.contains("fay@") && !c.to.contains("avery@"), "{}", c.to);
    // The meeting isn't answered.
    assert_eq!(s.mailbox.event(planning).unwrap().attendees[1].response, Response::Tentative);
}

#[test]
fn tracking_says_who_answered_how() {
    let mut s = session();
    let (_, _, planning) = week(&mut s);
    s.sel.event = Some(planning);
    let t = run(&mut s, "calendar.tracking", json!({}));
    assert_eq!(t["counts"], json!({"accepted": 1, "tentative": 1, "declined": 1, "none": 0}));
    assert_eq!(t["organizer"]["email"], json!("dana@partner.example"));
    assert_eq!(t["organizes"], json!(false));
    assert_eq!(t["attendees"][2], json!({"name": "Fay Lin", "email": "fay@partner.example", "optional": true, "response": "declined"}));
    assert!(s.run("calendar.tracking", &json!({"id": "x"})).is_err());
}

#[test]
fn search_calendar_reads_the_search_box_language() {
    let mut s = session();
    let (budget, dentist, planning) = week(&mut s);
    let found = |s: &mut Session, q: &str| -> Vec<Id> {
        let a = run(s, "calendar.search", json!({"query": q}));
        a["events"].as_array().unwrap().iter().map(|e| e["id"].as_u64().unwrap()).collect()
    };
    assert_eq!(found(&mut s, "budget"), vec![budget]);
    assert_eq!(found(&mut s, "harbour"), vec![dentist], "where it is, as the text");
    assert_eq!(found(&mut s, "from:dana"), vec![planning]);
    assert_eq!(found(&mut s, "to:eli"), vec![planning]);
    assert_eq!(found(&mut s, "subject:planning on:2026-10-20"), vec![planning]);
    assert!(found(&mut s, "subject:planning on:2026-10-21").is_empty());
    assert!(!found(&mut s, "-budget").contains(&budget));
    assert!(s.run("calendar.search", &json!({"query": "  "})).is_err());
    assert!(s.run("calendar.search", &json!({})).is_err());
}

#[test]
fn starting_a_message_saves_the_one_being_written_first() {
    let mut s = session();
    let (_, _, planning) = week(&mut s);
    for (id, params) in
        [("calendar.email", json!({})), ("calendar.contactAttendees", json!({"id": planning})), ("calendar.respondEmail", json!({"id": planning}))]
    {
        run(&mut s, "mail.new", json!({}));
        run(&mut s, "compose.set", json!({"subject": format!("Unsaved {id}"), "body": "Half a thought"}));
        let steps = s.history.len();
        run(&mut s, id, params);
        assert!(s.mailbox.messages.values().any(|m| m.draft && m.subject == format!("Unsaved {id}")), "{id}");
        assert_eq!(s.undo_label(), Some("Save Draft"), "{id}");
        assert_eq!(s.history.len(), steps + 1, "{id}");
    }
}

#[test]
fn an_agent_may_open_these_and_is_asked_before_closing_the_owners_message() {
    let mut s = session();
    let (_, _, planning) = week(&mut s);
    for id in ["calendar.email", "calendar.contactAttendees", "calendar.respondEmail", "calendar.tracking", "calendar.search"] {
        assert_eq!(classify(&s, id, &json!({"id": planning, "query": "x"})), Access::Safe, "{id}");
    }
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Mine", "body": "Owner's words"}));
    assert_eq!(classify(&s, "calendar.contactAttendees", &json!({"id": planning})), Access::Asked(Class::Leave));
    assert_eq!(classify(&s, "insert.calendar", &json!({})), Access::Safe, "the post-run check keeps the owner's message theirs");
}
