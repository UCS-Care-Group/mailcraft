// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #209: a subscribed calendar's events are written by the refresh and by nothing else. One
//! sweep tries every command against a subscribed calendar and one of its events; the rest are
//! the ways in that are not a command's params (a file, an invitation, an undo, an account that
//! goes) and what the review of the first version found.

use mailcraft_model::{Address, Attachment, FolderKind, Id, Message};
use serde_json::{Value, json};

use crate::Session;

const NOW: i64 = 1_791_993_600;
const ADDRESS: &str = "https://cal.example.org/private/T0KEN-s3cret-9f2a/basic.ics";

fn session() -> Session {
    Session::at(crate::sample::mailbox(NOW, 0), NOW, 0)
}

fn feed(events: &[(&str, &str)]) -> String {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nX-WR-CALNAME:Club fixtures\r\n");
    for (uid, summary) in events {
        text.push_str(&format!(
            "BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\nDTSTART:20261114T150000Z\r\nDTEND:20261114T160000Z\r\nEND:VEVENT\r\n"
        ));
    }
    text.push_str("END:VCALENDAR\r\n");
    text
}

fn came(s: &mut Session, calendar: Id, text: &str) {
    let outcome = json!({"kind": "changed", "text": text, "etag": "", "lastModified": ""});
    s.run("sync.applySubscription", &json!({"calendar": calendar, "outcome": outcome})).unwrap();
}

/// A subscribed calendar with one event from its feed.
fn subscribed(s: &mut Session) -> (Id, Id) {
    let calendar = s.run("calendar.subscribe", &json!({"address": ADDRESS})).unwrap()["id"].as_u64().unwrap();
    came(s, calendar, &feed(&[("a@example.org", "Home match")]));
    let event = s.mailbox.events.values().find(|e| e.calendar == calendar).map(|e| e.id).unwrap();
    (calendar, event)
}

/// Everything there is of subscribed calendars: the calendars and every event in them.
fn held(s: &Session) -> Value {
    let calendars: Vec<_> = s.mailbox.calendars.iter().filter(|c| c.is_subscribed()).collect();
    let ids: Vec<Id> = calendars.iter().map(|c| c.id).collect();
    let events: Vec<_> = s.mailbox.events.values().filter(|e| ids.contains(&e.calendar)).collect();
    json!({"calendars": calendars, "events": events})
}

/// What a command could be given to name the calendar or its event, however it spells it.
fn params(calendar: Id, event: Id) -> Vec<Value> {
    let words = json!({
        "title": "Changed", "start": "2026-11-20T10:00", "end": "2026-11-20T11:00", "location": "Elsewhere", "description": "x",
        "response": "accept", "send": false, "value": true, "name": "Renamed", "category": "Red category", "categories": ["Red category"],
        "minutes": 10, "reminder": 10, "showAs": "free", "private": true, "scope": "series", "keep": true, "force": true,
    });
    let mut all = Vec::new();
    for (id, to) in [(event, calendar), (calendar, calendar), (event, 0), (0, calendar)] {
        let mut one = words.clone();
        one["id"] = json!(id);
        one["ids"] = json!([id]);
        one["event"] = json!(event);
        one["calendar"] = json!(to);
        one["to"] = json!(to);
        one["calendarId"] = json!(to);
        one["target"] = json!(to);
        one["from"] = json!(calendar);
        all.push(one);
    }
    all
}

#[test]
fn no_command_puts_an_event_into_a_subscribed_calendar_or_changes_one_there() {
    // A fresh session for every try, so that no command works on what another left: the feed's
    // event selected, its calendar the one being worked in, and an event of the owner's own.
    let fresh = || {
        let mut s = session();
        let (calendar, event) = subscribed(&mut s);
        let own = s.run("calendar.new", &json!({"title": "Dentist", "start": "2026-11-20T10:00"})).unwrap()["id"].as_u64().unwrap();
        s.run("calendar.setCurrent", &json!({"id": calendar})).unwrap();
        s.run("calendar.select", &json!({"id": event})).unwrap();
        (s, calendar, event, own)
    };
    let (s, calendar, event, own) = fresh();
    let before = held(&s);
    // What a refresh, the owner's choice of a name and interval, and the calendar's being shown
    // are for: not events, and the refresh is the one way in.
    let mine = [
        "calendar.subscribe",
        "calendar.unsubscribe",
        "calendar.refreshSubscribed",
        "calendar.subscriptionSettings",
        "sync.applySubscription",
        "calendar.showCalendar",
    ];
    let ids: Vec<&'static str> = s.registry.all().iter().map(|c| c.id).filter(|id| !id.starts_with("file.") && !mine.contains(id)).collect();
    let mut shapes = vec![json!({}), json!({"calendar": calendar}), json!({"id": event, "calendar": calendar}), json!({"id": calendar})];
    shapes.extend(params(calendar, event));
    shapes.extend(params(calendar, own));
    let mut tried = 0;
    for id in ids {
        for p in &shapes {
            let (mut s, ..) = fresh();
            let _ = s.run(id, p);
            tried += 1;
            assert_eq!(held(&s), before, "{id} {p}");
        }
    }
    assert!(tried > 3_000, "the sweep ran {tried} calls");
}

#[test]
fn an_ics_file_does_not_replace_what_a_feed_brought_and_never_goes_into_the_calendar() {
    let mut s = session();
    let (calendar, event) = subscribed(&mut s);
    let before = held(&s);
    // The same uid, the same (no) organizer: an import that finds its match among the owner's own.
    let file = feed(&[("a@example.org", "Mine, imported")]);
    crate::io::import(&mut s, "club.ics", file.as_bytes(), None).unwrap();
    assert_eq!(held(&s), before);
    let imported = s.mailbox.events.values().find(|e| e.title == "Mine, imported").unwrap();
    assert_ne!((imported.id, imported.calendar), (event, calendar));
    assert!(!s.mailbox.calendar(imported.calendar).unwrap().is_subscribed());
}

#[test]
fn an_invitation_with_the_uid_of_a_feeds_event_is_not_that_event() {
    let mut s = session();
    let account = s.run("account.add", &json!({"email": "ada@example.org", "name": "Ada", "incoming": {"host": "imap.example.org"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    subscribed(&mut s);
    let before = held(&s);
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\nUID:a@example.org\r\nSUMMARY:Design review\r\nDTSTART:20261114T150000Z\r\nDTEND:20261114T160000Z\r\nORGANIZER;CN=Bo:mailto:bo@example.org\r\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:ada@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let inbox = s.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let message = Message {
        folder: inbox,
        message_id: "invite@example.org".into(),
        from: Address::new("Bo", "bo@example.org"),
        to: vec![Address::new("Ada", "ada@example.org")],
        subject: "Design review".into(),
        attachments: vec![Attachment::new("invite.ics", "text/calendar", ics.as_bytes().to_vec())],
        ..Default::default()
    };
    let message = s.mailbox.add_message(message).unwrap();
    // It isn't on the calendar as the feed's event: nothing is found to answer for or take out.
    let read = s.run("calendar.invitation", &json!({"message": message})).unwrap();
    assert!(read["event"].is_null(), "{read}");
    for response in ["accept", "tentative", "decline"] {
        let _ = s.run("calendar.respond", &json!({"message": message, "response": response}));
        assert_eq!(held(&s), before, "{response}");
    }
    let _ = s.run("calendar.removeCancelled", &json!({"message": message}));
    assert_eq!(held(&s), before);
}

#[test]
fn undoing_leaves_subscribed_calendars_alone_in_either_order() {
    // An undo of the subscribing doesn't take the calendar (and the address kept for it) away.
    let mut s = session();
    let (calendar, event) = subscribed(&mut s);
    let name = s.mailbox.calendar(calendar).unwrap().subscription.as_ref().unwrap().secret_name(calendar);
    s.run("calendar.new", &json!({"title": "Dentist", "start": "2026-11-20T10:00"})).unwrap();
    while s.run("edit.undo", &json!({})).is_ok() {}
    let held_still = s.mailbox.calendar(calendar).unwrap().subscription.as_ref().unwrap().secret_name(calendar);
    assert_eq!(held_still, name);
    assert!(s.mailbox.event(event).is_some());
    while s.run("edit.redo", &json!({})).is_ok() {}
    assert!(s.mailbox.calendar(calendar).is_some() && s.mailbox.event(event).is_some());

    // An undo of the unsubscribing doesn't bring one back, without its address.
    let mut s = session();
    let calendars = s.mailbox.calendars.len();
    let (calendar, _) = subscribed(&mut s);
    s.run("calendar.unsubscribe", &json!({"id": calendar})).unwrap();
    while s.run("edit.undo", &json!({})).is_ok() {}
    assert!(s.mailbox.calendars.iter().all(|c| !c.is_subscribed()), "an unsubscribed calendar came back");
    assert!(s.mailbox.events.values().all(|e| s.mailbox.calendar(e.calendar).is_some()));
    assert_eq!(s.mailbox.calendars.len(), calendars);
}

#[test]
fn the_account_that_goes_does_not_send_its_events_into_a_subscribed_calendar() {
    let mut s = session();
    subscribed(&mut s);
    let account = s.run("account.add", &json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    // A calendar of that account's with an appointment the owner made, and no calendar of the
    // mailbox's own but the subscribed one: the only home the appointment could be given.
    let served = s.mailbox.alloc_id();
    let origin = mailcraft_model::Origin { account, id: "main".into(), ..Default::default() };
    s.mailbox.calendars.retain(|c| c.is_subscribed());
    s.mailbox.calendars.push(mailcraft_model::Calendar { id: served, name: "Calendar".into(), origin, visible: true, ..Default::default() });
    let made = mailcraft_model::Event { title: "Dentist".into(), calendar: served, ..Default::default() };
    let made = s.mailbox.add_event(made).unwrap();
    let before = held(&s);
    s.run("account.remove", &json!({"id": account})).unwrap();
    assert_eq!(held(&s), before);
    let home = s.mailbox.event(made).unwrap().calendar;
    assert!(s.mailbox.calendar(home).is_some_and(|c| !c.is_subscribed()));
}

#[test]
fn new_appointments_never_go_where_the_owner_is_looking_at_a_subscribed_calendar() {
    let mut s = session();
    let (calendar, _) = subscribed(&mut s);
    // The owner looks at it (which picks it as the current one), and makes an appointment.
    let _ = s.run("calendar.setCurrent", &json!({"id": calendar}));
    let made = s.run("calendar.new", &json!({"title": "Dentist", "start": "2026-11-20T10:00"})).unwrap();
    let event = s.mailbox.event(made["id"].as_u64().unwrap()).unwrap();
    assert_ne!(event.calendar, calendar);
    assert_ne!(crate::cmd::calendar::current_calendar(&s), calendar);
}

#[test]
fn what_a_feed_says_is_one_line_without_control_or_bidirectional_characters() {
    let mut s = session();
    let calendar = s.run("calendar.subscribe", &json!({"address": ADDRESS})).unwrap()["id"].as_u64().unwrap();
    let text = "BEGIN:VCALENDAR\r\nX-WR-CALNAME:Club \u{202e}fixtures\u{1b}[31m\r\nBEGIN:VEVENT\r\nUID:a\u{1b}b\u{202e}c@example.org\r\n\
                SUMMARY:Match\u{85}\u{202e}gnp.exe\u{0}\r\nLOCATION:Pitch\u{9c}\u{2066}1\r\nCATEGORIES:Home\u{202e},Cup\u{1b}\r\n\
                DESCRIPTION:Line one\\nLine\u{202e} two\u{1b}[2J\r\nDTSTART:20261114T150000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    came(&mut s, calendar, text);
    let bad = |t: &str| t.chars().any(|c| c.is_control() && c != '\n' && c != '\t' || mailcraft_names::is_format(c));
    let e = s.mailbox.events.values().find(|e| e.calendar == calendar).unwrap();
    for t in [&e.title, &e.location, &e.uid, &e.description].into_iter().chain(e.categories.iter()) {
        assert!(!bad(t), "{t:?}");
    }
    assert_eq!(e.title, "Matchgnp.exe");
    assert!(e.description.contains("Line one\nLine"));
    assert!(!bad(&s.mailbox.calendar(calendar).unwrap().name));
}

#[test]
fn a_file_that_stops_short_is_not_taken_in() {
    let mut s = session();
    let (calendar, event) = subscribed(&mut s);
    let before = held(&s);
    let whole = feed(&[("a@example.org", "Changed"), ("b@example.org", "More")]);
    let cut = &whole[..whole.len() - "END:VCALENDAR\r\n".len() - 20];
    let outcome = json!({"kind": "changed", "text": cut, "etag": "\"new\"", "lastModified": "x"});
    let said = s.run("sync.applySubscription", &json!({"calendar": calendar, "outcome": outcome})).unwrap();
    assert!(said["failure"].is_string(), "{said}");
    assert_eq!(held(&s)["events"], before["events"]);
    assert!(s.mailbox.event(event).is_some());
    // And the marks of what wasn't taken are not kept.
    let sub = s.mailbox.calendar(calendar).unwrap().subscription.as_ref().unwrap();
    assert!(sub.etag.is_empty() && sub.last_modified.is_empty());
}

#[test]
fn undo_and_redo_still_take_back_the_colour_and_visibility_a_subscribed_calendar_was_given() {
    let mut s = session();
    let (calendar, event) = subscribed(&mut s);
    let (color, visible) = {
        let c = s.mailbox.calendar(calendar).unwrap();
        (c.shown_color(), c.visible)
    };
    s.run("calendar.color", &json!({"id": calendar, "color": "#112233"})).unwrap();
    let painted = s.mailbox.calendar(calendar).unwrap().shown_color();
    assert_ne!(painted, color);
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.calendar(calendar).unwrap().shown_color(), color, "the colour came back");
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.calendar(calendar).unwrap().shown_color(), painted);

    s.run("calendar.showCalendar", &json!({"id": calendar, "value": !visible})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.calendar(calendar).unwrap().visible, visible, "it is shown as it was");
    s.run("edit.redo", &json!({})).unwrap();
    assert_eq!(s.mailbox.calendar(calendar).unwrap().visible, !visible);
    // The subscription and the feed's events were never the undo's.
    assert!(s.mailbox.calendar(calendar).unwrap().is_subscribed() && s.mailbox.event(event).is_some());
}

#[test]
fn a_file_ends_with_its_last_line_not_with_a_text_a_note_holds() {
    let mut s = session();
    let (calendar, _) = subscribed(&mut s);
    // Pages of blank lines after the end don't make it a file that stopped short.
    let padded = format!("{}{}", feed(&[("a@example.org", "Padded")]), "\r\n".repeat(3_000));
    came(&mut s, calendar, &padded);
    assert!(s.mailbox.events.values().any(|e| e.title == "Padded"));
    // A file cut inside a note that says END:VCALENDAR is cut, not whole.
    let cut = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:z\r\nSUMMARY:Cut\r\nDTSTART:20261114T150000Z\r\nDESCRIPTION:see END:VCALENDAR";
    let outcome = json!({"kind": "changed", "text": cut, "etag": "", "lastModified": ""});
    let said = s.run("sync.applySubscription", &json!({"calendar": calendar, "outcome": outcome})).unwrap();
    assert!(said["failure"].is_string(), "{said}");
    assert!(s.mailbox.events.values().any(|e| e.title == "Padded") && s.mailbox.events.values().all(|e| e.title != "Cut"));
}
