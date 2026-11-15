// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #209: subscribed calendars: the commands, what a look from the host does to the calendar, and
//! that the address (which may hold a secret) is never kept.

use mailcraft_model::subscription::{MAX_EVENTS, MAX_EVENTS_TOTAL, MAX_TEXT};
use mailcraft_model::{Calendar, Id};
use proptest::prelude::*;
use serde_json::{Value, json};

use crate::Session;
use crate::agent::{Access, Class, access};

const NOW: i64 = 1_791_993_600;
const SECRET: &str = "T0KEN-s3cret-9f2a";
const ADDRESS: &str = "webcal://cal.example.org/private/T0KEN-s3cret-9f2a/basic.ics";

fn session() -> Session {
    Session::at(crate::sample::mailbox(NOW, 0), NOW, 0)
}

fn ics(events: &[(&str, &str, &str)]) -> String {
    let mut text = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\nX-WR-CALNAME:Club fixtures\r\n");
    for (uid, summary, start) in events {
        text.push_str(&format!("BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\nDTSTART:{start}\r\nDTEND:{start}\r\nEND:VEVENT\r\n"));
    }
    text.push_str("END:VCALENDAR\r\n");
    text
}

fn subscribe(s: &mut Session) -> Id {
    s.run("calendar.subscribe", &json!({"address": ADDRESS})).unwrap()["id"].as_u64().unwrap()
}

fn came(s: &mut Session, calendar: Id, text: &str, etag: &str) -> Value {
    let outcome = json!({"kind": "changed", "text": text, "etag": etag, "lastModified": "Tue, 03 Nov 2026 10:00:00 GMT"});
    s.run("sync.applySubscription", &json!({"calendar": calendar, "outcome": outcome})).unwrap()
}

fn titles(s: &Session, calendar: Id) -> Vec<String> {
    let mut titles: Vec<String> = s.mailbox.events.values().filter(|e| e.calendar == calendar).map(|e| e.title.clone()).collect();
    titles.sort();
    titles
}

fn id_of(s: &Session, calendar: Id, title: &str) -> Id {
    s.mailbox.events.values().find(|e| e.calendar == calendar && e.title == title).map(|e| e.id).unwrap()
}

#[test]
fn a_subscription_is_a_read_only_calendar_under_other_calendars_and_nothing_is_fetched() {
    let mut s = session();
    let before = s.mailbox.calendars.len();
    let answer = s.run("calendar.subscribe", &json!({"address": ADDRESS})).unwrap();
    assert_eq!(
        (answer["host"].as_str(), answer["name"].as_str(), answer["pending"].as_bool()),
        (Some("cal.example.org"), Some("cal.example.org"), Some(true))
    );
    assert_eq!(s.mailbox.calendars.len(), before + 1);
    let id = answer["id"].as_u64().unwrap();
    let calendar = s.mailbox.calendar(id).unwrap();
    assert!(calendar.is_subscribed() && !calendar.can_edit() && calendar.visible);
    assert_eq!(calendar.group, "Other Calendars");
    // Its colour is its own: not the one of any calendar before it.
    assert!(s.mailbox.calendars.iter().filter(|c| c.id != id).all(|c| c.color != calendar.color));
    // It is listed under Other Calendars, read-only.
    let listed = s.run("calendar.calendars", &json!({})).unwrap();
    let group = listed["groups"].as_array().unwrap().iter().find(|g| g["title"] == "Other Calendars").unwrap();
    let row = &group["calendars"].as_array().unwrap()[0];
    assert_eq!((row["id"].as_u64(), row["readOnly"].as_bool()), (Some(id), Some(true)));
    assert_eq!(row["subscription"]["host"], "cal.example.org");
}

#[test]
fn the_address_is_checked_as_text_and_never_echoed_or_kept() {
    let mut s = session();
    let before = s.mailbox.calendars.len();
    for (address, wanted) in [
        (format!("http://cal.example.org/{SECRET}.ics"), "https://"),
        (format!("https://user:{SECRET}@cal.example.org/x.ics"), "user name"),
        (format!("ftp://cal.example.org/{SECRET}"), "https:// or webcal://"),
        (format!("https://cal.example.org/{SECRET}/{}", "a".repeat(2_100)), "too long"),
        (format!("https://cal.example.org/{SECRET} x"), "web address"),
    ] {
        let error = s.run("calendar.subscribe", &json!({"address": address})).unwrap_err().to_string();
        assert!(error.contains(wanted), "{error}");
        assert!(!error.contains(SECRET) && !error.contains("cal.example.org"), "{error}");
    }
    assert!(s.run("calendar.subscribe", &json!({})).is_err());
    assert_eq!(s.mailbox.calendars.len(), before);

    let id = subscribe(&mut s);
    came(&mut s, id, &ics(&[("a", "Home match", "20261114T150000Z")]), "\"v1\"");
    // Nowhere in what the mailbox would write, nor in what any command says back.
    let file = serde_json::to_string(&s.mailbox).unwrap();
    assert!(!file.contains(SECRET) && !file.contains("/private/"), "the address is in the mailbox file");
    assert!(file.contains("cal.example.org"));
    for command in ["calendar.calendars", "calendar.list", "engine.state"] {
        if let Ok(said) = s.run(command, &json!({})) {
            assert!(!said.to_string().contains(SECRET), "{command}");
        }
    }
}

#[test]
fn an_http_address_is_refused_with_the_offer_of_https() {
    let mut s = session();
    let error = s.run("calendar.subscribe", &json!({"address": "http://cal.example.org/x.ics"})).unwrap_err().to_string();
    assert!(error.contains("Try the same address with https://"), "{error}");
}

#[test]
fn a_file_brings_the_events_and_the_name_it_gives_itself() {
    let mut s = session();
    let id = subscribe(&mut s);
    let said = came(&mut s, id, &ics(&[("a", "Home match", "20261114T150000Z"), ("b", "Away match", "20261121T150000Z")]), "\"v1\"");
    assert_eq!(
        (said["changed"].as_bool(), said["events"].as_u64(), said["added"].as_u64(), said["failure"].is_null()),
        (Some(true), Some(2), Some(2), true)
    );
    assert_eq!(titles(&s, id), ["Away match", "Home match"]);
    let calendar = s.mailbox.calendar(id).unwrap();
    assert_eq!(calendar.name, "Club fixtures");
    let sub = calendar.subscription.as_ref().unwrap();
    assert_eq!((sub.etag.as_str(), sub.checked, sub.events, sub.failure.as_str()), ("\"v1\"", NOW, 2, ""));
    assert!(sub.last_modified.starts_with("Tue, 03 Nov"));
}

#[test]
fn a_name_the_owner_has_is_not_replaced_by_the_files() {
    let mut s = session();
    let id = s.run("calendar.subscribe", &json!({"address": ADDRESS, "name": "Fixtures"})).unwrap()["id"].as_u64().unwrap();
    came(&mut s, id, &ics(&[("a", "Home match", "20261114T150000Z")]), "");
    assert_eq!(s.mailbox.calendar(id).unwrap().name, "Fixtures");
    // Two with one name get two.
    let second = s.run("calendar.subscribe", &json!({"address": ADDRESS, "name": "fixtures"})).unwrap();
    assert_eq!(second["name"], "fixtures (2)");
}

#[test]
fn a_refresh_replaces_wholesale_and_an_event_that_is_the_same_keeps_its_id() {
    let mut s = session();
    let id = subscribe(&mut s);
    came(
        &mut s,
        id,
        &ics(&[("a", "Home match", "20261114T150000Z"), ("b", "Away match", "20261121T150000Z"), ("c", "Cup", "20261128T150000Z")]),
        "\"v1\"",
    );
    let (home, away) = (id_of(&s, id, "Home match"), id_of(&s, id, "Away match"));
    let said = came(
        &mut s,
        id,
        &ics(&[("a", "Home match", "20261114T150000Z"), ("b", "Away match (moved)", "20261122T150000Z"), ("d", "Final", "20261205T150000Z")]),
        "\"v2\"",
    );
    assert_eq!((said["added"].as_u64(), said["updated"].as_u64(), said["removed"].as_u64()), (Some(1), Some(1), Some(1)));
    assert_eq!(titles(&s, id), ["Away match (moved)", "Final", "Home match"]);
    assert_eq!(id_of(&s, id, "Home match"), home, "unchanged: the same id");
    assert_eq!(id_of(&s, id, "Away match (moved)"), away, "changed: the same event, the same id");
    // The same file again changes nothing at all.
    let again = came(
        &mut s,
        id,
        &ics(&[("a", "Home match", "20261114T150000Z"), ("b", "Away match (moved)", "20261122T150000Z"), ("d", "Final", "20261205T150000Z")]),
        "\"v2\"",
    );
    assert_eq!((again["added"].as_u64(), again["updated"].as_u64(), again["removed"].as_u64()), (Some(0), Some(0), Some(0)));
    // An empty listing is a complete one: everything goes.
    came(&mut s, id, &ics(&[]), "\"v3\"");
    assert!(titles(&s, id).is_empty());
}

#[test]
fn a_changed_instance_of_a_series_is_its_own_event_keyed_apart() {
    let mut s = session();
    let id = subscribe(&mut s);
    let file = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:s@example.org\r\nSUMMARY:Stand-up\r\nDTSTART:20261026T140000Z\r\nRRULE:FREQ=DAILY;COUNT=5\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:s@example.org\r\nRECURRENCE-ID:20261027T140000Z\r\nSUMMARY:Stand-up (moved)\r\nDTSTART:20261027T160000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let said = came(&mut s, id, file, "");
    assert_eq!(said["events"], 2);
    let keys: Vec<String> = s.mailbox.events.values().filter(|e| e.calendar == id).map(|e| e.uid.clone()).collect();
    assert!(keys.iter().any(|k| k == "s@example.org") && keys.iter().any(|k| k.starts_with("s@example.org#")), "{keys:?}");
    let before: Vec<Id> = s.mailbox.events.values().filter(|e| e.calendar == id).map(|e| e.id).collect();
    came(&mut s, id, file, "");
    let after: Vec<Id> = s.mailbox.events.values().filter(|e| e.calendar == id).map(|e| e.id).collect();
    assert_eq!(before, after);
}

#[test]
fn a_look_that_brings_nothing_keeps_what_was_there_and_says_why() {
    let mut s = session();
    let id = subscribe(&mut s);
    came(&mut s, id, &ics(&[("a", "Home match", "20261114T150000Z")]), "\"v1\"");
    s.clock.now = Some(NOW + 3_600);
    for (outcome, why) in [
        (json!({"kind": "failed", "failure": "status:404"}), "no longer there"),
        (json!({"kind": "failed", "failure": "status:403"}), "won't hand it out"),
        (json!({"kind": "failed", "failure": "tooLarge"}), "larger than 8 MB"),
        (json!({"kind": "failed", "failure": "notPublic"}), "private network"),
        (json!({"kind": "failed", "failure": "unheard-of"}), "didn't work"),
        (json!({"kind": "changed", "text": "<html><body>Please sign in</body></html>"}), "isn't a calendar"),
        (json!({"kind": "changed", "text": "x".repeat(MAX_TEXT + 1)}), "larger than 8 MB"),
    ] {
        let said = s.run("sync.applySubscription", &json!({"calendar": id, "outcome": outcome})).unwrap();
        assert_eq!(said["changed"], false);
        assert!(said["failure"].as_str().unwrap().contains(why), "{said}");
        assert_eq!(titles(&s, id), ["Home match"], "{why}");
        let sub = s.mailbox.calendar(id).unwrap().subscription.clone().unwrap();
        assert!(sub.failed().is_some() && sub.checked == NOW + 3_600 && sub.etag == "\"v1\"", "{sub:?}");
    }
    // A good look clears it.
    let ok = s.run("sync.applySubscription", &json!({"calendar": id, "outcome": {"kind": "notModified"}})).unwrap();
    assert!(ok["failure"].is_null());
    assert!(s.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().failed().is_none());
    assert_eq!(titles(&s, id), ["Home match"]);
}

#[test]
fn a_file_over_the_limit_keeps_what_fits_and_says_so() {
    let mut s = session();
    let id = subscribe(&mut s);
    let many: Vec<(String, String, String)> =
        (0..=MAX_EVENTS).map(|n| (format!("e{n}"), format!("Event {n}"), "20261114T150000Z".to_string())).collect();
    let refs: Vec<(&str, &str, &str)> = many.iter().map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str())).collect();
    assert_eq!(refs.len(), 20_001);
    let said = came(&mut s, id, &ics(&refs), "");
    assert_eq!((said["events"].as_u64(), said["capped"].as_bool()), (Some(MAX_EVENTS as u64), Some(true)));
    assert_eq!(s.mailbox.events.values().filter(|e| e.calendar == id).count(), MAX_EVENTS);
    assert!(s.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().capped);
    // Exactly the limit is no trouble, and the note goes away.
    let said = came(&mut s, id, &ics(&refs[..MAX_EVENTS]), "");
    assert_eq!((said["events"].as_u64(), said["capped"].as_bool()), (Some(MAX_EVENTS as u64), Some(false)));
}

#[test]
fn all_the_subscribed_calendars_together_have_a_limit_too() {
    let mut s = session();
    let many: Vec<(String, String, String)> =
        (0..MAX_EVENTS).map(|n| (format!("e{n}"), format!("Event {n}"), "20261114T150000Z".to_string())).collect();
    let refs: Vec<(&str, &str, &str)> = many.iter().map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str())).collect();
    let text = ics(&refs);
    let mut last = Value::Null;
    for _ in 0..=(MAX_EVENTS_TOTAL / MAX_EVENTS) {
        let id = subscribe(&mut s);
        last = came(&mut s, id, &text, "");
    }
    let held = s.mailbox.events.values().filter(|e| s.mailbox.calendar(e.calendar).is_some_and(Calendar::is_subscribed)).count();
    assert_eq!(held, MAX_EVENTS_TOTAL);
    assert_eq!((last["events"].as_u64(), last["capped"].as_bool()), (Some(0), Some(true)));
}

#[test]
fn what_a_file_adds_to_an_appointment_stays_an_appointment() {
    let mut s = session();
    let id = subscribe(&mut s);
    let file = format!(
        "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:m@example.org\r\nSUMMARY:{}\r\nDTSTART:20261114T150000Z\r\nORGANIZER:mailto:boss@example.org\r\nATTENDEE;PARTSTAT=NEEDS-ACTION:mailto:me@example.org\r\nBEGIN:VALARM\r\nTRIGGER:-PT15M\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\nBEGIN:VTODO\r\nUID:t@example.org\r\nSUMMARY:A to-do\r\nEND:VTODO\r\nEND:VCALENDAR\r\n",
        "t".repeat(2_000)
    );
    let tasks = s.mailbox.tasks.len();
    came(&mut s, id, &file, "");
    let event = s.mailbox.events.values().find(|e| e.calendar == id).unwrap();
    assert!(event.attendees.is_empty() && event.organizer.is_none() && event.reminder_minutes.is_none());
    assert_eq!(event.title.chars().count(), 500);
    assert_eq!(s.mailbox.tasks.len(), tasks, "to-dos are ignored");
}

#[test]
fn nothing_in_a_subscribed_calendar_can_be_changed_here() {
    let mut s = session();
    let id = subscribe(&mut s);
    came(&mut s, id, &ics(&[("a", "Home match", "20261114T150000Z")]), "");
    let event = id_of(&s, id, "Home match");
    for (command, params) in [
        ("calendar.delete", json!({"id": event})),
        ("calendar.edit", json!({"id": event, "title": "Mine now"})),
        ("calendar.new", json!({"title": "New", "start": "2026-11-20T10:00", "calendar": id})),
        ("calendar.removeCalendar", json!({"id": id})),
        ("calendar.moveToAccount", json!({"calendar": id})),
    ] {
        assert!(s.run(command, &params).is_err(), "{command}");
    }
    assert_eq!(titles(&s, id), ["Home match"]);
    // Appointments of the owner's own still go where they were going.
    let own = s.run("calendar.new", &json!({"title": "Dentist", "start": "2026-11-20T10:00"})).unwrap();
    let made = s.mailbox.event(own["id"].as_u64().unwrap()).unwrap();
    assert_ne!(made.calendar, id);
}

#[test]
fn unsubscribing_removes_the_calendar_and_its_events_and_names_the_secret_to_forget() {
    let mut s = session();
    let (calendars, events) = (s.mailbox.calendars.len(), s.mailbox.events.len());
    let id = subscribe(&mut s);
    came(&mut s, id, &ics(&[("a", "Home match", "20261114T150000Z"), ("b", "Away", "20261121T150000Z")]), "");
    s.run("calendar.setCurrent", &json!({"id": id})).unwrap();
    let name = s.mailbox.calendar(id).unwrap().subscription.as_ref().unwrap().secret_name(id);
    let said = s.run("calendar.unsubscribe", &json!({"id": id})).unwrap();
    assert_eq!((said["events"].as_u64(), said["secret"].as_str()), (Some(2), Some(name.as_str())));
    assert_eq!((s.mailbox.calendars.len(), s.mailbox.events.len()), (calendars, events));
    assert_eq!(s.view.in_calendar, 0);
    // Not one that isn't subscribed to.
    let own = s.mailbox.calendars[0].id;
    assert!(s.run("calendar.unsubscribe", &json!({"id": own})).is_err());
    assert!(s.run("calendar.unsubscribe", &json!({"id": id})).is_err());
    assert!(s.mailbox.calendar(own).is_some());
}

#[test]
fn a_look_is_due_when_never_made_or_the_interval_is_up_and_the_request_has_no_address() {
    let mut s = session();
    let id = subscribe(&mut s);
    let due = |s: &mut Session, all: bool| s.run("sync.subscriptionRequest", &json!({"all": all})).unwrap();
    let first = due(&mut s, false);
    assert_eq!(first["requests"], json!([{"calendar": id, "host": "cal.example.org", "etag": "", "lastModified": ""}]));
    assert!(!first.to_string().contains(SECRET) && !first.to_string().contains("private"));
    came(&mut s, id, &ics(&[("a", "Home match", "20261114T150000Z")]), "\"v1\"");
    assert_eq!(due(&mut s, false)["requests"], json!([]));
    assert_eq!(due(&mut s, true)["requests"][0]["etag"], "\"v1\"");
    // An hour on, it is due; a quarter of an hour is as soon as it can be chosen.
    s.clock.now = Some(NOW + 3_599);
    assert_eq!(due(&mut s, false)["requests"], json!([]));
    s.clock.now = Some(NOW + 3_600);
    assert_eq!(due(&mut s, false)["requests"].as_array().unwrap().len(), 1);
    s.clock.now = Some(NOW + 900);
    assert_eq!(due(&mut s, false)["requests"], json!([]));
    s.run("calendar.subscriptionSettings", &json!({"id": id, "intervalMinutes": 15})).unwrap();
    assert_eq!(due(&mut s, false)["requests"].as_array().unwrap().len(), 1);
    // Send/Receive asks for one ahead of its time; a look takes the question away.
    s.clock.now = Some(NOW + 1);
    s.run("calendar.subscriptionSettings", &json!({"id": id, "intervalMinutes": 1440})).unwrap();
    assert_eq!(due(&mut s, false)["requests"], json!([]));
    assert_eq!(s.run("calendar.refreshSubscribed", &json!({})).unwrap()["calendars"], 1);
    assert_eq!(due(&mut s, false)["requests"].as_array().unwrap().len(), 1);
    s.run("sync.applySubscription", &json!({"calendar": id, "outcome": {"kind": "notModified"}})).unwrap();
    assert_eq!(due(&mut s, false)["requests"], json!([]));
}

#[test]
fn the_interval_is_a_quarter_of_an_hour_to_a_day() {
    let mut s = session();
    let id = subscribe(&mut s);
    for minutes in [json!(0), json!(14), json!(1441), json!(-5), json!("60"), json!(null), json!(1e300)] {
        assert!(s.run("calendar.subscriptionSettings", &json!({"id": id, "intervalMinutes": minutes})).is_err(), "{minutes}");
    }
    for minutes in [15, 60, 1440] {
        let said = s.run("calendar.subscriptionSettings", &json!({"id": id, "intervalMinutes": minutes})).unwrap();
        assert_eq!(said["intervalMinutes"], minutes);
    }
}

#[test]
fn an_address_given_again_must_be_the_same_host() {
    let mut s = session();
    let id = subscribe(&mut s);
    assert!(s.run("calendar.subscriptionSettings", &json!({"id": id, "address": "https://CAL.example.org/private/other-token.ics"})).is_ok());
    for address in ["https://evil.example.net/private/T0KEN.ics", "http://cal.example.org/x.ics", "https://u:p@cal.example.org/x.ics", ""] {
        let error = s.run("calendar.subscriptionSettings", &json!({"id": id, "address": address})).unwrap_err().to_string();
        assert!(!error.contains("T0KEN"), "{error}");
    }
}

#[test]
fn the_subscription_survives_the_mailbox_file_without_its_address() {
    let mut s = session();
    let id = subscribe(&mut s);
    came(&mut s, id, &ics(&[("a", "Home match", "20261114T150000Z")]), "\"v1\"");
    s.run("calendar.subscriptionSettings", &json!({"id": id, "intervalMinutes": 30})).unwrap();
    let text = serde_json::to_string(&s.mailbox).unwrap();
    let back: mailcraft_model::Mailbox = serde_json::from_str(&text).unwrap();
    let sub = back.calendar(id).unwrap().subscription.clone().unwrap();
    assert_eq!((sub.host.as_str(), sub.interval_minutes, sub.etag.as_str(), sub.events), ("cal.example.org", 30, "\"v1\"", 1));
    assert!(!back.calendar(id).unwrap().can_edit());
    assert_eq!(back.events.values().filter(|e| e.calendar == id).count(), 1);
}

#[test]
fn agents_need_the_owners_yes_to_subscribe_and_never_see_the_host_protocol() {
    assert_eq!(access("calendar.subscribe"), Some(Access::Owner(Class::Accounts)));
    assert_eq!(access("calendar.unsubscribe"), Some(Access::Owner(Class::Accounts)));
    assert_eq!(access("calendar.subscriptionSettings"), Some(Access::Owner(Class::Accounts)));
    assert_eq!(access("calendar.refreshSubscribed"), Some(Access::Safe));
    assert_eq!(access("sync.subscriptionRequest"), Some(Access::Never));
    assert_eq!(access("sync.applySubscription"), Some(Access::Never));
}

#[test]
fn junk_params_are_refused_without_a_panic() {
    let mut s = session();
    let id = subscribe(&mut s);
    for junk in [
        json!(null),
        json!([]),
        json!("x"),
        json!({"calendar": id}),
        json!({"calendar": id, "outcome": null}),
        json!({"calendar": id, "outcome": "changed"}),
        json!({"calendar": id, "outcome": {"kind": "nonsense"}}),
        json!({"calendar": id, "outcome": {"kind": "changed", "text": 5}}),
        json!({"calendar": id, "outcome": {"kind": "changed", "text": null, "etag": [1]}}),
        json!({"calendar": id, "outcome": {"kind": "failed", "failure": 7}}),
        json!({"calendar": 0, "outcome": {"kind": "notModified"}}),
        json!({"calendar": u64::MAX, "outcome": {"kind": "notModified"}}),
        json!({"calendar": -1, "outcome": {"kind": "notModified"}}),
    ] {
        for command in [
            "sync.applySubscription",
            "sync.subscriptionRequest",
            "calendar.subscribe",
            "calendar.unsubscribe",
            "calendar.refreshSubscribed",
            "calendar.subscriptionSettings",
        ] {
            let _ = s.run(command, &junk);
        }
    }
    assert!(s.mailbox.calendar(id).is_some());
    // The first calendar the mailbox has is not a subscribed one: no command is ever applied to it.
    let own = s.mailbox.calendars[0].id;
    assert!(s.run("sync.applySubscription", &json!({"calendar": own, "outcome": {"kind": "notModified"}})).is_err());
}

fn piece() -> impl Strategy<Value = String> {
    let uid = prop::sample::select(vec!["a", "b", "c", "a@example.org", "", "x#y"]);
    let when = prop::sample::select(vec!["20261114T150000Z", "20261114", "20261114T150000", "99999999", "", "20261114T150000;TZID=Europe/Berlin"]);
    let summary = "[ -~\u{e9}\u{4e2d}]{0,30}";
    (uid, when, summary, prop::bool::ANY, prop::bool::ANY).prop_map(|(uid, when, summary, rrule, rid)| {
        let mut text = format!("BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:{summary}\r\nDTSTART:{when}\r\n");
        if rrule {
            text.push_str("RRULE:FREQ=DAILY;COUNT=3\r\n");
        }
        if rid {
            text.push_str("RECURRENCE-ID:20261115T150000Z\r\n");
        }
        text.push_str("END:VEVENT\r\n");
        text
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn any_file_is_taken_without_a_panic_and_the_same_file_twice_changes_nothing(pieces in prop::collection::vec(piece(), 0..12), junk in "\\PC{0,200}", header in prop::bool::ANY) {
        let mut s = session();
        let id = subscribe(&mut s);
        let mut text = if header { String::from("BEGIN:VCALENDAR\r\n") } else { junk.clone() };
        text.extend(pieces.iter().map(String::as_str));
        text.push_str(&junk);
        text.push_str("END:VCALENDAR\r\n");
        let outcome = json!({"kind": "changed", "text": text});
        let first = s.run("sync.applySubscription", &json!({"calendar": id, "outcome": outcome}));
        prop_assert!(first.is_ok());
        let held: Vec<Id> = s.mailbox.events.values().filter(|e| e.calendar == id).map(|e| e.id).collect();
        prop_assert!(held.len() <= MAX_EVENTS);
        let second = s.run("sync.applySubscription", &json!({"calendar": id, "outcome": outcome})).unwrap();
        let again: Vec<Id> = s.mailbox.events.values().filter(|e| e.calendar == id).map(|e| e.id).collect();
        prop_assert_eq!(&held, &again);
        prop_assert_eq!((second["added"].as_u64(), second["updated"].as_u64(), second["removed"].as_u64()), (Some(0), Some(0), Some(0)));
        // Whatever the file held, the calendar is still read-only and holds nobody's invitation or reminder.
        prop_assert!(!s.mailbox.calendar(id).unwrap().can_edit());
        prop_assert!(s.mailbox.events.values().filter(|e| e.calendar == id).all(|e| e.attendees.is_empty() && e.reminder_minutes.is_none() && e.title.chars().count() <= 500));
        // Every id is one event's.
        let mut ids = held.clone();
        ids.sort_unstable();
        ids.dedup();
        prop_assert_eq!(ids.len(), held.len());
    }
}
