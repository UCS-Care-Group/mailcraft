// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars: a scripted server for what is asked and how the answers are read, events of every shape for
//! the mapping, and whatever a server may say for what must never panic.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use mailcraft_model::remote::RemoteEventKind;
use mailcraft_model::{Address, Attendee, Response, ShowAs};
use proptest::prelude::*;
use serde_json::Map;

use super::event::{Span, duration};
use super::*;
use crate::connection::tests::any_json;
use crate::reader::{MAX_CHUNK, PAGE, QUERY_BYTES};
use crate::test_support::{FakeExchange, made_secret};
use crate::{CAPABILITY_CORE, CAPABILITY_MAIL, Credentials, HttpRequest, HttpResponse, Method, RetryAfter, TransportError, block_on};

const SESSION: &str = "https://jmap.example.org/jmap/session";
const API: &str = "https://jmap.example.org/jmap/api";

type Reply = std::result::Result<HttpResponse, TransportError>;

fn reply(status: u16, body: &Value) -> Reply {
    Ok(HttpResponse::new(status, serde_json::to_vec(body).unwrap()).with_field("Content-Type", "application/json"))
}

fn octets(status: u16, body: &[u8]) -> Reply {
    Ok(HttpResponse::new(status, body.to_vec()))
}

/// What the one call of a request answers.
fn answer(method: &str, arguments: Value) -> Reply {
    reply(200, &json!({"methodResponses": [[method, arguments, "c0"]], "sessionState": "1"}))
}

fn listing(calendars: Value) -> Reply {
    answer("Calendar/get", json!({"list": calendars, "notFound": []}))
}

fn query(ids: Value) -> Reply {
    let total = ids.as_array().map_or(0, Vec::len);
    answer("CalendarEvent/query", json!({"ids": ids, "position": 0, "total": total}))
}

fn get(list: Value) -> Reply {
    answer("CalendarEvent/get", json!({"list": list, "notFound": []}))
}

fn refused(kind: &str) -> Reply {
    answer("error", json!({"type": kind}))
}

/// A session whose account `A1` has mail and calendars, the server taking `max_get` objects in a `get`.
fn session_doc(max_get: u64) -> Value {
    json!({
        "capabilities": {CAPABILITY_CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": max_get}, CAPABILITY_MAIL: {}, CAPABILITY_CALENDARS: {}},
        "accounts": {"A1": {"name": "ada@example.org", "isPersonal": true, "accountCapabilities": {CAPABILITY_MAIL: {}, CAPABILITY_CALENDARS: {}}}},
        "primaryAccounts": {CAPABILITY_MAIL: "A1", CAPABILITY_CALENDARS: "A1"},
        "apiUrl": "/jmap/api",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "/jmap/upload/{accountId}/",
        "state": "1",
    })
}

fn connect<H: HttpExchange>(http: H) -> Connection<H> {
    block_on(Connection::connect(http, SESSION, Credentials::bearer(&made_secret("tk-3f9a7c")).unwrap())).unwrap()
}

/// A connection over a script: the session `doc`, then `replies`.
fn connected_with(doc: &Value, replies: Vec<Reply>) -> Connection<FakeExchange> {
    let mut script = vec![reply(200, doc)];
    script.extend(replies);
    connect(FakeExchange::new(script))
}

fn connected(replies: Vec<Reply>) -> Connection<FakeExchange> {
    connected_with(&session_doc(500), replies)
}

/// The instant a text names.
fn at(text: &str) -> Timestamp {
    mailcraft_time::iso_to_timestamp(text, 0).unwrap()
}

/// November 2026.
fn asked() -> JmapCalendarRequest {
    JmapCalendarRequest {
        account: 7,
        email: " ada@example.org".into(),
        session: SESSION.into(),
        bearer: true,
        from: at("2026-11-01T00:00:00Z"),
        to: at("2026-12-01T00:00:00Z"),
    }
}

fn run<H: HttpExchange>(connection: &Connection<H>) -> JmapCalendarResult {
    block_on(sync_calendar(connection, &asked(), &AtomicBool::new(false)))
}

/// The requests after the session's.
fn sent(connection: &Connection<FakeExchange>) -> Vec<HttpRequest> {
    connection.http().sent().into_iter().skip(1).collect()
}

/// The one call of each API request sent: its method and its arguments.
fn calls(connection: &Connection<FakeExchange>) -> Vec<(String, Value)> {
    sent(connection)
        .iter()
        .filter(|request| request.method() == Method::Post)
        .map(|request| {
            let body: Value = serde_json::from_slice(request.body()).unwrap();
            (body["methodCalls"][0][0].as_str().unwrap().to_string(), body["methodCalls"][0][1].clone())
        })
        .collect()
}

fn methods(connection: &Connection<FakeExchange>) -> Vec<String> {
    calls(connection).into_iter().map(|(method, _)| method).collect()
}

fn listed(got: &JmapCalendarResult) -> &CalendarsDownload {
    got.calendars.as_ref().expect("the calendars were listed")
}

/// The ids of the calendars listed, the main one in brackets.
fn names(got: &JmapCalendarResult) -> Vec<String> {
    listed(got).calendars.iter().map(|calendar| if calendar.main { format!("[{}]", calendar.id) } else { calendar.id.clone() }).collect()
}

fn ids(down: &EventsDownload) -> Vec<&str> {
    down.events.iter().map(|event| event.id.as_str()).collect()
}

fn calendar(id: &str) -> Value {
    json!({"id": id, "name": format!("Calendar {id}"), "isSubscribed": true, "myRights": {"mayReadItems": true}})
}

fn plain(id: &str) -> Value {
    json!({"id": id, "@type": "Event", "title": format!("Event {id}"), "utcStart": "2026-11-03T09:00:00Z", "utcEnd": "2026-11-03T10:00:00Z"})
}

/// The object as an event of the account `ada@example.org`, when it is one that can be read.
fn mapped(value: &Value) -> Option<RemoteEvent> {
    match event("E1", value, "ada@example.org") {
        Mapped::Event(event) => Some(*event),
        Mapped::Other | Mapped::Unreadable => None,
    }
}

fn meeting() -> Value {
    json!({
        "id": "E1", "@type": "Event", "uid": "0f0e-1@example.org", "title": " Review ", "description": "The plan.\nAnd the rest.",
        "descriptionContentType": "text/plain", "updated": "2026-10-14T09:00:00Z",
        "start": "2026-11-03T10:00:00", "timeZone": "Europe/Berlin", "duration": "PT1H",
        "utcStart": "2026-11-03T09:00:00Z", "utcEnd": "2026-11-03T10:00:00Z",
        "locations": {"b": {"@type": "Location", "name": "Room 2"}, "a": {"name": ""}, "c": {"name": "Elsewhere"}},
        "keywords": {"work": true, "alpha": true, "off": false, "odd": 1},
        "privacy": "private", "status": "tentative", "freeBusyStatus": "busy",
        "useDefaultAlerts": true,
        "alerts": {
            "a1": {"trigger": {"@type": "OffsetTrigger", "offset": "PT5M"}},
            "a2": {"trigger": {"@type": "AbsoluteTrigger", "when": "2026-11-03T08:00:00Z"}},
            "a3": {"trigger": {"@type": "OffsetTrigger", "offset": "-PT30M", "relativeTo": "end"}},
            "a4": {"trigger": {"@type": "OffsetTrigger", "offset": "-PT15M", "relativeTo": "start"}},
            "a5": {"trigger": {"@type": "OffsetTrigger", "offset": "-PT1H"}}},
        "participants": {
            "p1": {"name": "Grace Example", "sendTo": {"imip": "mailto:grace@example.org"}, "roles": {"owner": true, "attendee": true},
                "participationStatus": "accepted"},
            "p2": {"name": "Ada", "email": "Ada@Example.org", "roles": {"attendee": true}, "participationStatus": "accepted"},
            "p3": {"name": "Bob", "sendTo": {"imip": "MAILTO:bob%40example.org?subject=Re"}, "email": "other@example.org",
                "roles": {"attendee": true, "optional": true}, "participationStatus": "declined"},
            "p4": {"name": "Nobody", "roles": {"attendee": true}},
            "p5": {"name": "Carol", "email": "carol@example.org", "roles": {"informational": true}},
            "p6": {"name": "Dan", "calendarAddress": "mailto:dan@example.org", "roles": {"attendee": true}, "participationStatus": "tentative"},
            "p7": {"name": "Eve", "email": "eve@example.org", "roles": {"attendee": true}, "participationStatus": "needs-action"}},
    })
}

// ------------------------------------------------------------------------------------------
// Whether, and whose

#[test]
fn a_server_without_calendars_is_no_failure_and_is_asked_nothing() {
    // Not advertised at all.
    let mut doc = session_doc(500);
    doc["capabilities"].as_object_mut().unwrap().remove(CAPABILITY_CALENDARS);
    // Advertised, and no account offers them.
    let mut none = session_doc(500);
    none["accounts"]["A1"]["accountCapabilities"].as_object_mut().unwrap().remove(CAPABILITY_CALENDARS);
    none["primaryAccounts"].as_object_mut().unwrap().remove(CAPABILITY_CALENDARS);
    // Advertised, and the account named for them does not offer them, nor the one for mail.
    let mut named = session_doc(500);
    named["accounts"]["A1"]["accountCapabilities"].as_object_mut().unwrap().remove(CAPABILITY_CALENDARS);
    for doc in [doc, none, named] {
        let connection = connected_with(&doc, vec![]);
        let got = run(&connection);
        assert!(got.not_offered && got.calendars.is_none() && got.events.is_empty() && got.errors.is_empty() && !got.stopped && !got.auth_refused);
        assert_eq!((got.account, got.email.as_str(), got.session.as_str()), (7, " ada@example.org", SESSION));
        assert!(sent(&connection).is_empty());
    }
}

#[test]
fn the_account_the_session_names_for_calendars_is_read() {
    let mut doc = session_doc(500);
    doc["accounts"]["A1"]["accountCapabilities"].as_object_mut().unwrap().remove(CAPABILITY_CALENDARS);
    doc["accounts"]["C1"] = json!({"name": "diary", "isPersonal": true, "accountCapabilities": {CAPABILITY_CALENDARS: {}}});
    doc["primaryAccounts"][CAPABILITY_CALENDARS] = json!("C1");
    let connection = connected_with(&doc, vec![listing(json!([calendar("K1")])), query(json!(["E1"])), get(json!([plain("E1")]))]);
    let got = run(&connection);
    assert!(listed(&got).complete && got.errors.is_empty(), "{:?}", got.errors);
    assert_eq!(calls(&connection).len(), 3);
    assert!(calls(&connection).iter().all(|(_, args)| args["accountId"] == "C1"));
    // No account named for calendars: the one for mail, when it offers them.
    let mut doc = session_doc(500);
    doc["primaryAccounts"].as_object_mut().unwrap().remove(CAPABILITY_CALENDARS);
    let connection = connected_with(&doc, vec![listing(json!([]))]);
    let got = run(&connection);
    assert!(!got.not_offered && listed(&got).complete && listed(&got).calendars.is_empty() && got.events.is_empty() && got.errors.is_empty());
    assert_eq!(calls(&connection)[0].1["accountId"], "A1");
}

// ------------------------------------------------------------------------------------------
// The run

#[test]
fn the_calendars_are_listed_and_their_events_read_and_made_into_events() {
    let work = json!({"id": "C1", "name": " Work ", "color": "#0a8", "sortOrder": 5, "isDefault": true, "isSubscribed": true,
        "myRights": {"mayReadItems": true, "mayWriteAll": true, "mayAdmin": true}});
    let home = json!({"id": "C2", "name": "Home", "color": "#FF0000", "sortOrder": 1});
    let whole_day = json!({"id": "E2", "@type": "Event", "title": "Fair", "showWithoutTime": true, "start": "2026-11-20T00:00:00", "duration": "P2D",
        "timeZone": null, "utcStart": "2026-11-19T23:00:00Z", "utcEnd": "2026-11-21T23:00:00Z"});
    let reckoned = json!({"id": "E3", "title": "Call", "start": "2026-11-12T09:00:00", "timeZone": "Europe/Berlin", "duration": "PT1H30M"});
    let occurrence = |id: &str, day: &str| {
        json!({"id": id, "@type": "Event", "uid": "series-4@example.org", "title": "Weekly", "recurrenceId": format!("{day}T09:00:00"),
            "recurrenceRules": [{"@type": "RecurrenceRule", "frequency": "weekly"}], "start": format!("{day}T09:00:00"), "timeZone": "Etc/UTC",
            "duration": "PT45M", "utcStart": format!("{day}T09:00:00Z"), "utcEnd": format!("{day}T09:45:00Z")})
    };
    let own = json!({"id": "E5", "@type": "Event", "title": "Mine", "status": "cancelled", "freeBusyStatus": "free", "privacy": "secret",
        "utcStart": "2026-11-25T12:00:00Z", "utcEnd": "2026-11-25T13:00:00Z",
        "participants": {"me": {"email": "ADA@example.org", "roles": {"owner": true}}, "you": {"email": "bob@example.org", "roles": {"attendee": true}}}});
    let task = json!({"id": "T1", "@type": "Task", "title": "No event"});
    let floating = json!({"id": "F1", "title": "Lunch", "start": "2026-11-05T12:00:00", "duration": "PT30M"});
    let replies = vec![
        listing(json!([work, home])),
        query(json!(["E1", "E2", "E3", "E4-20261103", "E4-20261110", "E5", "T1"])),
        get(json!([meeting(), whole_day, reckoned, occurrence("E4-20261103", "2026-11-03"), occurrence("E4-20261110", "2026-11-10"), own, task])),
        query(json!(["F1"])),
        get(json!([floating])),
    ];
    let connection = connected(replies);
    let got = run(&connection);
    assert!(got.errors.is_empty() && !got.not_offered && !got.stopped && !got.auth_refused && got.busy_for.is_none(), "{:?}", got.errors);
    assert_eq!((got.account, got.email.as_str(), got.session.as_str()), (7, " ada@example.org", SESSION));

    // The calendars: read only whatever the server allows, one of them the main one.
    let list = listed(&got);
    assert_eq!((list.account, list.email.as_str(), list.complete, list.refused.len()), (7, "ada@example.org", true, 0));
    assert_eq!(names(&got), ["[C1]", "C2"]);
    let (work, home) = (&list.calendars[0], &list.calendars[1]);
    assert_eq!((work.name.as_str(), work.color, work.rights), ("Work", Some(Rgb(0x00, 0xaa, 0x88)), Rights::Read));
    assert_eq!((home.name.as_str(), home.color, home.rights), ("Home", Some(Rgb(0xff, 0, 0)), Rights::Read));
    assert!(work.group.is_empty() && work.owner.is_none() && work.online_providers.is_none());

    // The downloads: one for each calendar, in the list's order, the main one without a name.
    assert_eq!(got.events.len(), 2);
    let (first, second) = (&got.events[0], &got.events[1]);
    for down in [first, second] {
        assert_eq!((down.account, down.email.as_str(), down.from, down.to), (7, "ada@example.org", asked().from, asked().to));
        assert!(down.complete && down.gone.is_empty() && down.has_categories && !down.has_options && !down.free_busy);
        assert!(down.events.iter().all(|event| event.kind == RemoteEventKind::Single && event.series.is_empty()));
        assert!(down.events.iter().all(|event| event.recurrence.is_none() && event.original.is_none() && event.series_day.is_none()));
    }
    assert_eq!((first.calendar.as_str(), second.calendar.as_str()), ("", "C2"));
    assert_eq!(ids(first), ["E1", "E2", "E3", "E4-20261103", "E4-20261110", "E5"], "a task is no event");

    // Every property that is read.
    let meeting = &first.events[0];
    assert_eq!((meeting.uid.as_str(), meeting.title.as_str(), meeting.location.as_str()), ("0f0e-1@example.org", "Review", "Room 2"));
    assert_eq!(meeting.description, "The plan.\nAnd the rest.");
    assert_eq!((meeting.start, meeting.end, meeting.all_day), (at("2026-11-03T09:00:00Z"), at("2026-11-03T10:00:00Z"), false));
    assert_eq!((meeting.start_zone.as_str(), meeting.end_zone.as_str()), ("Europe/Berlin", "Europe/Berlin"));
    assert_eq!(meeting.modified, at("2026-10-14T09:00:00Z"));
    assert_eq!(meeting.categories, ["alpha", "work"]);
    assert_eq!((meeting.private, meeting.cancelled, meeting.show_as), (true, false, ShowAs::Tentative));
    assert_eq!(meeting.reminder_minutes, Some(15), "the first alert a time before the start");
    assert_eq!(meeting.organizer, Some(Address::new("Grace Example", "grace@example.org")));
    let invited = |name: &str, email: &str, optional: bool, response: Response| Attendee { address: Address::new(name, email), optional, response };
    assert_eq!(
        meeting.attendees,
        [
            invited("Ada", "Ada@Example.org", false, Response::Accepted),
            invited("Bob", "bob@example.org", true, Response::Declined),
            invited("Dan", "dan@example.org", false, Response::Tentative),
            invited("Eve", "eve@example.org", false, Response::None),
        ]
    );
    assert_eq!((meeting.response, meeting.is_organizer), (Response::Accepted, Some(false)));
    assert!(meeting.change_key.is_empty() && meeting.token.is_empty() && meeting.online.is_none() && !meeting.attachments);

    // A whole day: the days the event itself names, at midnight UTC, to the day after the last.
    let fair = &first.events[1];
    assert_eq!((fair.start, fair.end, fair.all_day), (at("2026-11-20T00:00:00Z"), at("2026-11-22T00:00:00Z"), true));
    assert_eq!((fair.start_zone.as_str(), fair.is_organizer, fair.reminder_minutes), ("", None, None));
    // No instants from the server: the wall clock in its zone, and the duration.
    let call = &first.events[2];
    assert_eq!((call.start, call.end, call.all_day), (at("2026-11-12T08:00:00Z"), at("2026-11-12T09:30:00Z"), false));
    assert_eq!((call.start_zone.as_str(), call.show_as, call.response), ("Europe/Berlin", ShowAs::Busy, Response::None));
    // What repeats comes as its occurrences, each a plain event under the id the server made for it.
    let (third, tenth) = (&first.events[3], &first.events[4]);
    assert_eq!((third.start, third.end), (at("2026-11-03T09:00:00Z"), at("2026-11-03T09:45:00Z")));
    assert_eq!((tenth.start, tenth.end), (at("2026-11-10T09:00:00Z"), at("2026-11-10T09:45:00Z")));
    assert_eq!((third.uid.as_str(), tenth.uid.as_str(), third.title.as_str()), ("series-4@example.org", "series-4@example.org", "Weekly"));
    // The owner's own.
    let own = &first.events[5];
    assert_eq!((own.cancelled, own.private, own.show_as, own.is_organizer), (true, true, ShowAs::Free, Some(true)));
    assert_eq!(own.organizer, Some(Address::new("", "ADA@example.org")));
    assert_eq!(own.attendees.len(), 1);
    // An event that floats is read as UTC.
    assert_eq!(ids(second), ["F1"]);
    let lunch = &second.events[0];
    assert_eq!((lunch.start, lunch.end, lunch.start_zone.as_str()), (at("2026-11-05T12:00:00Z"), at("2026-11-05T12:30:00Z"), ""));

    // What was asked: the calendars, then of each its list and its events.
    let requests = sent(&connection);
    assert_eq!(requests.len(), 5);
    assert!(requests.iter().all(|request| request.url() == API));
    let body: Value = serde_json::from_slice(requests[0].body()).unwrap();
    assert_eq!(body["using"], json!([CAPABILITY_CORE, CAPABILITY_CALENDARS]));
    assert_eq!(body["methodCalls"], json!([["Calendar/get", {"accountId": "A1", "ids": null}, "c0"]]));
    assert_eq!(requests[0].max_response_bytes(), CALENDARS_BYTES);
    let body: Value = serde_json::from_slice(requests[1].body()).unwrap();
    assert_eq!(
        body["methodCalls"],
        json!([["CalendarEvent/query", {
            "accountId": "A1",
            "filter": {"inCalendar": "C1", "after": "2026-11-01T00:00:00Z", "before": "2026-12-01T00:00:00Z"},
            "expandRecurrences": true, "position": 0, "limit": PAGE, "calculateTotal": true}, "c0"]])
    );
    assert_eq!(requests[1].max_response_bytes(), QUERY_BYTES);
    let body: Value = serde_json::from_slice(requests[2].body()).unwrap();
    let (method, args) = (&body["methodCalls"][0][0], &body["methodCalls"][0][1]);
    assert_eq!((method, &args["accountId"]), (&json!("CalendarEvent/get"), &json!("A1")));
    assert_eq!(args["ids"], json!(["E1", "E2", "E3", "E4-20261103", "E4-20261110", "E5", "T1"]));
    let properties: Vec<&str> = args["properties"].as_array().unwrap().iter().map(|name| name.as_str().unwrap()).collect();
    assert_eq!(properties, PROPERTIES);
    for name in ["id", "utcStart", "utcEnd", "start", "timeZone", "duration", "showWithoutTime", "participants", "alerts", "keywords"] {
        assert!(properties.contains(&name), "{name}");
    }
    assert_eq!(requests[2].max_response_bytes(), GET_BYTES);
    assert_eq!(calls(&connection)[3].1["filter"]["inCalendar"], "C2");
    assert_eq!(calls(&connection)[4].1["ids"], json!(["F1"]));
}

#[test]
fn one_calendar_is_the_main_one_and_those_not_read_are_passed_over() {
    let with = |id: &str, more: Value| {
        let mut calendar = calendar(id);
        calendar.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        calendar
    };
    let no_events = |count: usize| (0..count).map(|_| query(json!([]))).collect::<Vec<Reply>>();
    let listing_of = |calendars: Value, count: usize| {
        let mut replies = vec![listing(calendars)];
        replies.extend(no_events(count));
        connected(replies)
    };
    // None is the default: the first by its place in the server's order, then by its id.
    let order =
        json!([with("C3", json!({"sortOrder": 2})), with("C2", json!({"sortOrder": 1})), with("C1", json!({"sortOrder": 1})), calendar("C9")]);
    let connection = listing_of(order, 4);
    let got = run(&connection);
    assert_eq!(names(&got), ["C3", "C2", "C1", "[C9]"], "no place said is the first place");
    assert!(listed(&got).complete && got.errors.is_empty());
    assert_eq!(got.events.iter().map(|down| down.calendar.as_str()).collect::<Vec<_>>(), ["C3", "C2", "C1", ""]);
    let order = json!([with("C3", json!({"sortOrder": 2})), with("C2", json!({"sortOrder": 1})), with("C1", json!({"sortOrder": 1}))]);
    assert_eq!(names(&run(&listing_of(order, 3))), ["C3", "C2", "[C1]"]);
    // The default, wherever it stands; of two that say so, one.
    let order =
        json!([calendar("C1"), with("C2", json!({"isDefault": true, "sortOrder": 9})), with("C3", json!({"isDefault": true, "sortOrder": 3}))]);
    assert_eq!(names(&run(&listing_of(order, 3))), ["C1", "C2", "[C3]"]);
    // One the account does not subscribe to, and one whose events it may not read, are not listed and not
    // asked for, were they the default; and the list is whole without them.
    let order = json!([
        with("C1", json!({"isSubscribed": false, "isDefault": true})),
        with("C2", json!({"myRights": {"mayReadItems": false, "mayReadFreeBusy": true}})),
        with("C3", json!({"myRights": {"mayReadItems": true, "mayWriteAll": true, "mayDelete": true}, "sortOrder": 4})),
        json!({"id": "C4", "sortOrder": 7}),
    ]);
    let connection = listing_of(order, 2);
    let got = run(&connection);
    assert_eq!(names(&got), ["[C3]", "C4"]);
    assert!(listed(&got).complete && got.errors.is_empty());
    assert!(listed(&got).calendars.iter().all(|calendar| calendar.rights == Rights::Read), "no change goes up");
    assert_eq!(methods(&connection), ["Calendar/get", "CalendarEvent/query", "CalendarEvent/query"]);
    assert_eq!(calls(&connection)[1].1["filter"]["inCalendar"], "C3");
    // No calendar at all: nothing more is asked.
    let connection = listing_of(json!([with("C1", json!({"isSubscribed": false}))]), 0);
    let got = run(&connection);
    assert!(listed(&got).complete && listed(&got).calendars.is_empty() && got.events.is_empty() && got.errors.is_empty());
    assert_eq!(sent(&connection).len(), 1);
}

#[test]
fn a_colour_is_one_written_in_hex_or_none() {
    assert_eq!(color("#abc"), Some(Rgb(0xaa, 0xbb, 0xcc)));
    assert_eq!(color(" #A1b2C3 "), Some(Rgb(0xa1, 0xb2, 0xc3)));
    for none in ["", "#", "red", "abc", "aabbcc", "#abcd", "#12345", "#1234567", "#+f+f+f", "#ggg", "rgb(1,2,3)", "#\u{e9}\u{e9}\u{e9}"] {
        assert_eq!(color(none), None, "{none}");
    }
}

#[test]
fn a_list_of_events_is_read_page_by_page_and_the_events_as_many_as_the_server_takes() {
    // The server says how many there are: the list ends there.
    let page = |ids: Value, total: u64| answer("CalendarEvent/query", json!({"ids": ids, "total": total}));
    let replies = vec![
        listing(json!([calendar("C1")])),
        page(json!(["E1", "E2"]), 3),
        page(json!(["E3"]), 3),
        get(json!([plain("E1"), plain("E2")])),
        get(json!([plain("E3")])),
    ];
    let connection = connected_with(&session_doc(2), replies);
    let got = run(&connection);
    assert!(got.events[0].complete && got.errors.is_empty(), "{:?}", got.errors);
    assert_eq!(ids(&got.events[0]), ["E1", "E2", "E3"]);
    let asked = calls(&connection);
    assert_eq!(methods(&connection), ["Calendar/get", "CalendarEvent/query", "CalendarEvent/query", "CalendarEvent/get", "CalendarEvent/get"]);
    assert_eq!((&asked[1].1["position"], &asked[2].1["position"]), (&json!(0), &json!(2)));
    assert!(asked[2].1["expandRecurrences"] == true && asked[2].1["filter"]["inCalendar"] == "C1");
    assert_eq!((&asked[3].1["ids"], &asked[4].1["ids"]), (&json!(["E1", "E2"]), &json!(["E3"])));
    // It does not say: until a page is empty. An id listed twice is asked for once.
    let bare = |ids: Value| answer("CalendarEvent/query", json!({"ids": ids}));
    let replies = vec![
        listing(json!([calendar("C1")])),
        bare(json!(["E1", "E2"])),
        bare(json!(["E2", "E3"])),
        bare(json!([])),
        get(json!([plain("E1"), plain("E2"), plain("E3")])),
    ];
    let connection = connected(replies);
    let got = run(&connection);
    assert!(got.events[0].complete && got.errors.is_empty());
    assert_eq!(ids(&got.events[0]), ["E1", "E2", "E3"]);
    assert_eq!(calls(&connection)[3].1["position"], 4);
    // A list that does not end is not followed for ever.
    let same = || bare(json!(["E1", "E2"]));
    let connection = connected(vec![listing(json!([calendar("C1")])), same(), same(), get(json!([plain("E1"), plain("E2")]))]);
    let got = run(&connection);
    assert_eq!(ids(&got.events[0]), ["E1", "E2"]);
    assert!(!got.events[0].complete && listed(&got).complete);
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(got.errors[0].starts_with("Couldn't read all the events of 1 calendar: "), "{}", got.errors[0]);
    assert_eq!(sent(&connection).len(), 4);
}

#[test]
fn what_is_gone_since_the_list_is_no_trouble_and_what_is_left_out_is() {
    // Gone: said so.
    let gone = answer("CalendarEvent/get", json!({"list": [plain("E1")], "notFound": ["E2", "E9", 5]}));
    let connection = connected(vec![listing(json!([calendar("C1")])), query(json!(["E1", "E2"])), gone]);
    let got = run(&connection);
    assert!(got.events[0].complete && got.errors.is_empty());
    assert_eq!(ids(&got.events[0]), ["E1"]);
    // Left out of both lists; an event nobody asked for and an event twice are not read.
    let short = get(json!([plain("E1"), plain("E7"), plain("E1"), "E2", {"id": 2}]));
    let connection = connected(vec![listing(json!([calendar("C1")])), query(json!(["E1", "E2"])), short]);
    let got = run(&connection);
    assert_eq!(ids(&got.events[0]), ["E1"]);
    assert!(!got.events[0].complete);
    assert_eq!(got.errors, ["Couldn't read all the events of 1 calendar from the server."]);
    // An event whose time can not be told is not read, and the calendar is not whole.
    let untold = json!({"id": "E2", "title": "When?", "start": "2026-11-05T12:00:00", "timeZone": "Nowhere/At_All"});
    let connection = connected(vec![listing(json!([calendar("C1")])), query(json!(["E1", "E2"])), get(json!([plain("E1"), untold]))]);
    let got = run(&connection);
    assert_eq!(ids(&got.events[0]), ["E1"]);
    assert!(!got.events[0].complete && got.errors.len() == 1);
    // Every event of a call gone: more likely a calendar the server lost.
    let all = answer("CalendarEvent/get", json!({"list": [], "notFound": ["E1", "E2"]}));
    let connection = connected(vec![listing(json!([calendar("C1")])), query(json!(["E1", "E2"])), all]);
    let got = run(&connection);
    assert!(!got.events[0].complete && got.events[0].events.is_empty() && got.errors.len() == 1);
}

#[test]
fn a_call_for_events_that_fails_is_halved_and_the_rest_are_read() {
    let replies = vec![
        listing(json!([calendar("C1"), calendar("C2")])),
        query(json!(["E1", "E2", "E3", "E4"])),
        get(json!([plain("E1"), plain("E2")])),
        // E3 and E4 together, then E3 by itself: refused. E4 by itself comes.
        refused("invalidArguments"),
        refused("invalidArguments"),
        get(json!([plain("E4")])),
        query(json!(["F1"])),
        get(json!([plain("F1")])),
    ];
    let connection = connected_with(&session_doc(2), replies);
    let got = run(&connection);
    assert_eq!((ids(&got.events[0]), ids(&got.events[1])), (vec!["E1", "E2", "E4"], vec!["F1"]));
    assert_eq!((got.events[0].complete, got.events[1].complete), (false, true), "one event of one calendar was not read");
    assert!(listed(&got).complete);
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(got.errors[0].starts_with("Couldn't read all the events of 1 calendar: "), "{}", got.errors[0]);
    let asked: Vec<Value> =
        calls(&connection).into_iter().filter(|(method, _)| method == "CalendarEvent/get").map(|(_, args)| args["ids"].clone()).collect();
    assert_eq!(asked, [json!(["E1", "E2"]), json!(["E3", "E4"]), json!(["E3"]), json!(["E4"]), json!(["F1"])]);
    // An answer too large for its request is halved the same way.
    let padded = answer("CalendarEvent/get", json!({"list": [plain("E1"), plain("E2")], "pad": "x".repeat(GET_BYTES)}));
    let replies = vec![listing(json!([calendar("C1")])), query(json!(["E1", "E2"])), padded, get(json!([plain("E1")])), get(json!([plain("E2")]))];
    let connection = connected(replies);
    let got = run(&connection);
    assert!(got.events[0].complete && got.errors.is_empty(), "{:?}", got.errors);
    assert_eq!(ids(&got.events[0]), ["E1", "E2"]);
}

#[test]
fn a_calendar_that_can_not_be_read_leaves_the_others_read() {
    let three = || listing(json!([calendar("C1"), calendar("C2"), calendar("C3")]));
    // The list of the first is refused, the server breaks for the second: the third is read.
    let detail = json!({"detail": "no diary for ada@example.org at https://jmap.example.org/jmap/session"});
    let connection = connected(vec![three(), refused("invalidArguments"), reply(500, &detail), query(json!(["E1"])), get(json!([plain("E1")]))]);
    let got = run(&connection);
    assert_eq!(names(&got), ["[C1]", "C2", "C3"]);
    assert!(listed(&got).complete);
    assert_eq!(got.events.len(), 1, "a calendar that could not be read at all has no download");
    assert_eq!((got.events[0].calendar.as_str(), got.events[0].complete, ids(&got.events[0])), ("C3", true, vec!["E1"]));
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(got.errors[0].starts_with("Couldn't read the events of 2 calendars: "), "{}", got.errors[0]);
    assert!(!got.auth_refused && !got.stopped && got.busy_for.is_none());
    // What the server says is told without the account's address and without the session's.
    let connection = connected(vec![three(), reply(500, &detail), query(json!([])), query(json!([]))]);
    let got = run(&connection);
    assert_eq!(got.events.len(), 2);
    let line = &got.errors[0];
    assert!(line.starts_with("Couldn't read the events of 1 calendar: ") && line.contains("no diary for"), "{line}");
    assert!(!line.contains("ada@") && !line.contains("jmap.example.org") && !line.contains("C1"), "{line}");
    // The server breaks while the events are read: what came is kept, and the next calendar is asked for.
    let replies = vec![
        three(),
        query(json!(["E1", "E2"])),
        get(json!([plain("E1")])),
        octets(500, b""),
        query(json!(["F1"])),
        get(json!([plain("F1")])),
        query(json!([])),
    ];
    let connection = connected_with(&session_doc(1), replies);
    let got = run(&connection);
    assert_eq!(
        got.events.iter().map(|down| (ids(down), down.complete)).collect::<Vec<_>>(),
        [(vec!["E1"], false), (vec!["F1"], true), (vec![], true)]
    );
    assert_eq!(got.errors.len(), 1);
    assert!(got.errors[0].starts_with("Couldn't read all the events of 1 calendar: "), "{}", got.errors[0]);
    // A second page that is refused: the first is read.
    let page = answer("CalendarEvent/query", json!({"ids": ["E1"], "total": 2}));
    let connection = connected(vec![listing(json!([calendar("C1")])), page, refused("invalidArguments"), get(json!([plain("E1")]))]);
    let got = run(&connection);
    assert_eq!((ids(&got.events[0]), got.events[0].complete, got.errors.len()), (vec!["E1"], false, 1));
}

#[test]
fn a_server_that_can_not_be_had_is_not_asked_for_every_calendar() {
    let five = listing(json!([calendar("C1"), calendar("C2"), calendar("C3"), calendar("C4"), calendar("C5")]));
    // (A script that has run out answers that the server can not be reached.)
    let connection = connected(vec![five]);
    let got = run(&connection);
    assert_eq!(listed(&got).calendars.len(), 5);
    assert!(got.events.is_empty() && !got.stopped && !got.auth_refused && got.busy_for.is_none());
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    assert!(got.errors[0].starts_with("Couldn't read the events of 5 calendars: "), "{}", got.errors[0]);
    assert_eq!(sent(&connection).len(), 1 + MAX_DOWN);
    // One that was read between two that were not: the count begins again.
    let five = listing(json!([calendar("C1"), calendar("C2"), calendar("C3"), calendar("C4"), calendar("C5")]));
    let down = || octets(500, b"");
    let connection = connected(vec![five, down(), down(), query(json!([])), down(), down()]);
    let got = run(&connection);
    assert_eq!((got.events.len(), sent(&connection).len()), (1, 6));
    assert!(got.errors[0].starts_with("Couldn't read the events of 4 calendars: "), "{}", got.errors[0]);
}

#[test]
fn calendars_that_can_not_be_listed_are_said_so_and_nothing_more_is_asked() {
    let firsts = [
        refused("invalidArguments"),
        octets(500, b""),
        octets(200, b"not json"),
        answer("Calendar/get", json!({"list": "C1"})),
        answer("Calendar/get", json!({"notFound": []})),
    ];
    for first in firsts {
        let connection = connected(vec![first, query(json!([]))]);
        let got = run(&connection);
        assert!(got.calendars.is_none() && got.events.is_empty() && !got.auth_refused && got.busy_for.is_none() && !got.stopped);
        assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
        assert!(got.errors[0].starts_with("Couldn't read the calendars from the server: "), "{}", got.errors[0]);
        assert_eq!(sent(&connection).len(), 1);
    }
}

#[test]
fn a_stretch_that_is_none_is_not_asked_for() {
    let from = at("2026-11-01T00:00:00Z");
    let six_years = 6 * 366 * 86_400;
    for (from, to) in [(from, from), (from, from - 1), (from, from + six_years), (-5, 86_400), (from, i64::MAX), (i64::MIN, i64::MAX)] {
        let connection = connected(vec![listing(json!([calendar("C1")]))]);
        let request = JmapCalendarRequest { from, to, ..asked() };
        let got = block_on(sync_calendar(&connection, &request, &AtomicBool::new(false)));
        assert!(got.calendars.is_none() && got.events.is_empty() && !got.not_offered && !got.stopped);
        assert_eq!(got.errors.len(), 1);
        assert!(got.errors[0].starts_with("Couldn't read the calendars from the server: "), "{}", got.errors[0]);
        assert!(sent(&connection).is_empty());
    }
    // Five years are read.
    let connection = connected(vec![listing(json!([]))]);
    let request = JmapCalendarRequest { from, to: from + 5 * 365 * 86_400, ..asked() };
    let got = block_on(sync_calendar(&connection, &request, &AtomicBool::new(false)));
    assert!(got.errors.is_empty() && got.calendars.is_some());
}

#[test]
fn a_refused_sign_in_and_a_busy_server_end_the_run_with_their_word() {
    for status in [401, 403] {
        let connection = connected(vec![octets(status, b"")]);
        let got = run(&connection);
        assert!(got.auth_refused && got.calendars.is_none() && got.errors.is_empty() && got.busy_for.is_none() && !got.stopped);
        // While the events are read: the calendars are handed over, and no more is asked.
        let connection = connected(vec![listing(json!([calendar("C1"), calendar("C2")])), octets(status, b""), query(json!([]))]);
        let got = run(&connection);
        assert!(got.auth_refused && got.errors.is_empty() && got.events.is_empty());
        assert_eq!((listed(&got).calendars.len(), sent(&connection).len()), (2, 2));
    }
    let busy = |status: u16, wait: Option<u32>| {
        let answer = HttpResponse::new(status, Vec::new());
        Ok(match wait {
            Some(seconds) => answer.with_field("Retry-After", &seconds.to_string()),
            None => answer,
        })
    };
    for (status, wait, expected) in [(429, Some(120), 120), (503, None, 30), (429, None, 30)] {
        let connection = connected(vec![busy(status, wait)]);
        let got = run(&connection);
        assert_eq!(got.busy_for, Some(expected));
        assert!(got.calendars.is_none() && !got.auth_refused && !got.stopped && got.errors.len() == 1);
        // And while the events of the second calendar are read: the first is whole, the second is told
        // nothing of, the third is not asked for.
        let replies = vec![
            listing(json!([calendar("C1"), calendar("C2"), calendar("C3")])),
            query(json!(["E1"])),
            get(json!([plain("E1")])),
            query(json!(["F1"])),
            busy(status, wait),
            query(json!([])),
        ];
        let connection = connected(replies);
        let got = run(&connection);
        assert_eq!(got.busy_for, Some(expected));
        assert_eq!(got.events.iter().map(|down| (down.events.len(), down.complete)).collect::<Vec<_>>(), [(1, true), (0, false)]);
        assert_eq!((got.errors.len(), sent(&connection).len()), (1, 5));
        assert!(got.errors[0].starts_with("Couldn't read the calendars from the server: "), "{}", got.errors[0]);
    }
}

#[test]
fn a_sign_in_that_failed_is_said_as_a_run_would_say_it() {
    let refused = failed_calendar(&asked(), &Error::CredentialsRefused);
    assert!(refused.auth_refused && refused.errors.is_empty() && refused.calendars.is_none() && refused.events.is_empty());
    assert_eq!((refused.account, refused.email.as_str(), refused.session.as_str()), (7, " ada@example.org", SESSION));
    let busy = failed_calendar(&asked(), &Error::Busy { retry_after: None });
    assert_eq!((busy.busy_for, busy.errors.len()), (Some(30), 1));
    let waited = failed_calendar(&asked(), &Error::Busy { retry_after: Some(RetryAfter::Delay(std::time::Duration::from_secs(9))) });
    assert_eq!(waited.busy_for, Some(9));
    let down = failed_calendar(&asked(), &Error::Transport(TransportError::Unreachable));
    assert!(!down.auth_refused && down.busy_for.is_none());
    assert_eq!(down.errors.len(), 1);
    assert!(down.errors[0].starts_with("Couldn't reach the server: "), "{}", down.errors[0]);
}

/// An exchange that sets a flag when it is asked for the request numbered `at` (the session's is the
/// first), and answers from a script.
struct Flipping {
    inner: FakeExchange,
    at: usize,
    seen: Cell<usize>,
    flag: Arc<AtomicBool>,
}

impl HttpExchange for Flipping {
    async fn exchange(&self, request: &HttpRequest) -> Reply {
        self.seen.set(self.seen.get() + 1);
        if self.seen.get() == self.at {
            self.flag.store(true, Ordering::Relaxed);
        }
        self.inner.exchange(request).await
    }
}

#[test]
fn cancel_is_looked_at_before_every_request() {
    // Before anything: the server is not asked.
    let connection = connected(vec![listing(json!([calendar("C1")]))]);
    let got = block_on(sync_calendar(&connection, &asked(), &AtomicBool::new(true)));
    assert!(got.stopped && got.calendars.is_none() && got.events.is_empty() && got.errors.is_empty());
    assert!(sent(&connection).is_empty());
    // While the calendars come, while a list of events comes, while the events come, and in the next calendar.
    let cases: [(usize, Vec<(usize, bool)>); 4] = [(2, vec![]), (3, vec![(0, false)]), (4, vec![(1, true)]), (5, vec![(1, true), (0, false)])];
    for (at, read) in cases {
        let flag = Arc::new(AtomicBool::new(false));
        let script = vec![
            reply(200, &session_doc(500)),
            listing(json!([calendar("C1"), calendar("C2")])),
            query(json!(["E1"])),
            get(json!([plain("E1")])),
            query(json!(["F1"])),
            get(json!([plain("F1")])),
        ];
        let connection = connect(Flipping { inner: FakeExchange::new(script), at, seen: Cell::new(0), flag: flag.clone() });
        let got = block_on(sync_calendar(&connection, &asked(), &flag));
        assert!(got.stopped && got.errors.is_empty(), "{at}: {:?}", got.errors);
        assert_eq!(listed(&got).calendars.len(), 2, "{at}");
        assert_eq!(got.events.iter().map(|down| (down.events.len(), down.complete)).collect::<Vec<_>>(), read, "{at}");
        assert_eq!(connection.http().inner.sent().len(), at, "{at}");
    }
    // An exchange called off because of it is the run stopped, not a server that is gone.
    let flag = Arc::new(AtomicBool::new(false));
    let script = vec![reply(200, &session_doc(500)), Err(TransportError::Aborted)];
    let connection = connect(Flipping { inner: FakeExchange::new(script), at: 2, seen: Cell::new(0), flag: flag.clone() });
    let got = block_on(sync_calendar(&connection, &asked(), &flag));
    assert!(got.stopped && got.errors.is_empty() && got.calendars.is_none());
}

// ------------------------------------------------------------------------------------------
// One event

#[test]
fn the_time_of_an_event_is_the_servers_or_reckoned_from_its_own_clock() {
    let times = |value: Value| mapped(&value).map(|event| (event.start, event.end, event.all_day));
    // The server's instants, whatever else the event says.
    let both = json!({"utcStart": "2026-11-03T09:00:00Z", "utcEnd": "2026-11-03T09:20:00Z", "start": "2020-01-01T00:00:00", "duration": "PT9H"});
    assert_eq!(times(both), Some((at("2026-11-03T09:00:00Z"), at("2026-11-03T09:20:00Z"), false)));
    // Its start only: the end by the duration; and no duration is none.
    let start = json!({"utcStart": "2026-11-03T09:00:00Z", "duration": "PT20M"});
    assert_eq!(times(start), Some((at("2026-11-03T09:00:00Z"), at("2026-11-03T09:20:00Z"), false)));
    let bare = json!({"utcStart": "2026-11-03T09:00:00Z", "duration": "soon"});
    assert_eq!(times(bare), Some((at("2026-11-03T09:00:00Z"), at("2026-11-03T09:00:00Z"), false)));
    // An end before the start is the start.
    let backwards = json!({"utcStart": "2026-11-03T09:00:00Z", "utcEnd": "2026-11-03T08:00:00Z"});
    assert_eq!(times(backwards), Some((at("2026-11-03T09:00:00Z"), at("2026-11-03T09:00:00Z"), false)));
    // Its own clock in its zone, also by a name Windows uses; days count on the wall clock across a change
    // of clocks (the last Sunday of October), hours in real time.
    let berlin = json!({"start": "2026-10-24T12:00:00", "timeZone": "Europe/Berlin", "duration": "P1DT1H"});
    assert_eq!(times(berlin), Some((at("2026-10-24T10:00:00Z"), at("2026-10-25T12:00:00Z"), false)));
    let windows = json!({"start": "2026-11-12T09:00:00", "timeZone": "W. Europe Standard Time", "duration": "PT1H"});
    assert_eq!(times(windows), Some((at("2026-11-12T08:00:00Z"), at("2026-11-12T09:00:00Z"), false)));
    // No zone: UTC. A wall clock that names an offset is an instant.
    let floating = json!({"start": "2026-11-12T09:00:00", "timeZone": null, "duration": "P1W"});
    assert_eq!(times(floating), Some((at("2026-11-12T09:00:00Z"), at("2026-11-19T09:00:00Z"), false)));
    let offset = json!({"start": "2026-11-12T09:00:00+02:00", "timeZone": "Nowhere/At_All"});
    assert_eq!(times(offset), Some((at("2026-11-12T07:00:00Z"), at("2026-11-12T07:00:00Z"), false)));
    // A zone nobody knows, a zone that is no text, no start, a start that is no time: no time to tell.
    for untold in [
        json!({"start": "2026-11-12T09:00:00", "timeZone": "Nowhere/At_All"}),
        json!({"start": "2026-11-12T09:00:00", "timeZone": 5}),
        json!({"title": "When?"}),
        json!({"start": "tomorrow"}),
        json!({"start": 17, "utcStart": {"at": 1}}),
        json!({"utcStart": "2026-13-41T09:00:00Z"}),
        json!({"start": format!("2026-11-12T09:00:00{}", " ".repeat(100))}),
    ] {
        assert!(matches!(event("E1", &untold, ""), Mapped::Unreadable), "{untold}");
    }
    // A whole day: the day its own start names, whatever the instants; a day at the least, every day touched.
    let day = |more: Value| {
        let mut event = json!({"showWithoutTime": true, "start": "2026-11-20T00:00:00"});
        event.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        times(event)
    };
    let twentieth = at("2026-11-20T00:00:00Z");
    assert_eq!(day(json!({})), Some((twentieth, twentieth + 86_400, true)));
    assert_eq!(day(json!({"duration": "P1D"})), Some((twentieth, twentieth + 86_400, true)));
    assert_eq!(day(json!({"duration": "P3D", "timeZone": "Pacific/Auckland"})), Some((twentieth, twentieth + 3 * 86_400, true)));
    assert_eq!(day(json!({"duration": "P1W"})), Some((twentieth, twentieth + 7 * 86_400, true)));
    assert_eq!(day(json!({"duration": "PT36H"})), Some((twentieth, twentieth + 2 * 86_400, true)));
    assert_eq!(day(json!({"duration": "PT0S", "utcStart": "2026-11-19T11:00:00Z"})), Some((twentieth, twentieth + 86_400, true)));
    // No start of its own: the day the server's instant falls on.
    let instant = json!({"showWithoutTime": true, "utcStart": "2026-11-20T11:00:00Z", "duration": "P1D"});
    assert_eq!(times(instant), Some((twentieth, twentieth + 86_400, true)));
    assert!(matches!(event("E1", &json!({"showWithoutTime": true}), ""), Mapped::Unreadable));
}

#[test]
fn a_duration_is_read_as_the_standard_writes_it_or_not_at_all() {
    let span = |days: i64, seconds: i64| Some(Span { negative: false, days, seconds });
    assert_eq!(duration("PT1H30M"), span(0, 5_400));
    assert_eq!(duration("P1D"), span(1, 0));
    assert_eq!(duration("P2W"), span(14, 0));
    assert_eq!(duration("P1W2DT3H4M5S"), span(9, 3 * 3_600 + 4 * 60 + 5));
    assert_eq!(duration("PT0S"), span(0, 0));
    assert_eq!(duration("PT1.5S"), span(0, 1));
    assert_eq!(duration("+PT90M"), span(0, 5_400));
    assert_eq!(duration("P7DT"), span(7, 0));
    assert_eq!(duration("-PT15M"), Some(Span { negative: true, days: 0, seconds: 900 }));
    assert_eq!(duration("-P1D"), Some(Span { negative: true, days: 1, seconds: 0 }));
    assert_eq!(duration("P40000D"), span(40_000, 0));
    assert_eq!(Span { negative: true, days: 2, seconds: 30 }.total(), 2 * 86_400 + 30);
    for none in [
        "",
        "P",
        "PT",
        "T1H",
        "1H",
        "PT1",
        "P1H",
        "PT1D",
        "PT1W",
        "P1M",
        "P1Y",
        "PT1M1H",
        "P1D1W",
        "PT1H1H",
        "PT1.5M",
        "PT.5S",
        "PT1.S.",
        "P-1D",
        "--P1D",
        "+-P1D",
        "p1d",
        "PT1h",
        "P 1D",
        "P1D ",
        "PT1HT1M",
        "P1.5D",
        "P40001D",
        "P5715W",
        "PT99999999999999999999S",
        "PT3456000001S",
        "P1DT99999999999999999999999999999999999S",
        "P\u{661}D",
    ] {
        assert_eq!(duration(none), None, "{none}");
    }
}

#[test]
fn the_people_of_an_event_are_its_organizer_those_invited_and_the_owner() {
    let with = |participants: Value| mapped(&json!({"utcStart": "2026-11-03T09:00:00Z", "participants": participants})).unwrap();
    // Nobody: nothing is said of who organizes it.
    let nobody = with(json!({}));
    assert_eq!((nobody.organizer, nobody.attendees.len(), nobody.response, nobody.is_organizer), (None, 0, Response::None, None));
    // People, and none of them its owner: the same.
    let guests = with(json!({"a": {"email": "ada@example.org", "roles": {"attendee": true}, "participationStatus": "declined"}}));
    assert_eq!((guests.organizer, guests.attendees.len(), guests.response, guests.is_organizer), (None, 1, Response::Declined, None));
    // An organizer with a name and no address is named, and does not say whose the event is.
    let named = with(json!({"a": {"name": "Grace", "roles": {"owner": true}}}));
    assert_eq!((named.organizer, named.is_organizer), (Some(Address::new("Grace", "")), None));
    // Of two owners the first by key is the organizer; the other is one of those invited when invited.
    let two = with(json!({
        "b": {"email": "bob@example.org", "roles": {"owner": true, "attendee": true}},
        "a": {"email": "ada@example.org", "roles": {"owner": true, "attendee": true}, "participationStatus": "tentative"}}));
    assert_eq!((two.organizer, two.is_organizer, two.response), (Some(Address::new("", "ada@example.org")), Some(true), Response::Tentative));
    assert_eq!(two.attendees, [Attendee { address: Address::new("", "bob@example.org"), optional: false, response: Response::None }]);
    // Addresses that are none: no person to invite. A role that is not set is none.
    let odd = with(json!({
        "a": {"email": "not an address", "roles": {"attendee": true}},
        "b": {"email": "a@", "roles": {"attendee": true}},
        "c": {"email": "@example.org", "roles": {"attendee": true}},
        "d": {"sendTo": {"imip": "https://example.org/bob@example.org"}, "roles": {"attendee": true}},
        "e": {"sendTo": {"imip": "mailto:bad%zz@example.org"}, "roles": {"attendee": true}},
        "f": {"email": "bob <bob@example.org>", "roles": {"attendee": true}},
        "g": {"email": format!("{}@example.org", "x".repeat(400)), "roles": {"attendee": true}},
        "h": {"email": "carol@example.org", "roles": {"attendee": false}},
        "i": {"email": "dan@example.org", "roles": ["attendee"]},
        "j": {"email": "eve@example.org", "roles": {"attendee": true, "optional": "yes"}, "participationStatus": 4}}));
    assert_eq!(odd.attendees, [Attendee { address: Address::new("", "eve@example.org"), optional: false, response: Response::None }]);
    // An event of an account with no address is nobody's own.
    let anonymous =
        match event("E1", &json!({"utcStart": "2026-11-03T09:00:00Z", "participants": {"a": {"roles": {"owner": true}, "name": "X"}}}), " ") {
            Mapped::Event(event) => *event,
            _ => panic!("an event"),
        };
    assert_eq!((anonymous.response, anonymous.is_organizer), (Response::None, None));
}

#[test]
fn what_is_no_event_is_passed_over_and_what_is_not_what_the_standard_says_is_as_good_as_absent() {
    for other in [json!(null), json!("E1"), json!(5), json!([plain("E1")]), json!({"@type": "Task"}), json!({"@type": "Group"}), json!({"@type": 5})]
    {
        assert!(matches!(event("E1", &other, ""), Mapped::Other), "{other}");
    }
    let wrong = json!({
        "id": "another", "utcStart": "2026-11-03T09:00:00Z", "utcEnd": false, "uid": 5, "title": ["x"], "description": {"text": "x"},
        "locations": ["Room"], "timeZone": "Europe/Berlin", "updated": "yesterday", "keywords": ["a", "b"], "privacy": true, "status": 1,
        "freeBusyStatus": null, "alerts": {"a": "soon", "b": {"trigger": "-PT5M"}, "c": {"trigger": {"offset": 5}}, "d": {"trigger": {"offset": "P1M"}}},
        "participants": ["ada@example.org"], "showWithoutTime": "yes", "duration": 60,
    });
    let read = mapped(&wrong).unwrap();
    let start = at("2026-11-03T09:00:00Z");
    let expected = RemoteEvent {
        id: "E1".into(),
        start,
        end: start,
        start_zone: "Europe/Berlin".into(),
        end_zone: "Europe/Berlin".into(),
        show_as: ShowAs::Busy,
        ..RemoteEvent::default()
    };
    assert_eq!(read, expected);
    // An alert that says nothing of what it counts from counts from the start; at the start is no minute before.
    let alert =
        |trigger: Value| mapped(&json!({"utcStart": "2026-11-03T09:00:00Z", "alerts": {"a": {"trigger": trigger}}})).unwrap().reminder_minutes;
    assert_eq!(alert(json!({"offset": "-PT10M"})), Some(10));
    assert_eq!(alert(json!({"@type": "OffsetTrigger", "offset": "PT0S"})), Some(0));
    assert_eq!(alert(json!({"@type": "OffsetTrigger", "offset": "-P1D"})), Some(1_440));
    assert_eq!(alert(json!({"@type": "OffsetTrigger", "offset": "-PT59S"})), Some(0));
    assert_eq!(alert(json!({"@type": "OffsetTrigger", "offset": "PT10M"})), None);
    assert_eq!(alert(json!({"@type": "UnknownTrigger", "offset": "-PT10M"})), None);
    assert_eq!(alert(json!({"@type": "OffsetTrigger", "offset": "-PT10M", "relativeTo": "end"})), None);
    // A zone whose name is too long to be one is not kept, and the server's instants still say when.
    let long = mapped(&json!({"utcStart": "2026-11-03T09:00:00Z", "timeZone": "Z".repeat(500)})).unwrap();
    assert_eq!((long.start, long.start_zone.as_str()), (start, ""));
}

#[test]
fn what_an_event_holds_is_cut_to_size() {
    let huge = "\u{e9}".repeat(600_000);
    let long = "\u{e9}".repeat(2_000);
    let mut participants = Map::new();
    let mut keywords = Map::new();
    let mut alerts = Map::new();
    let mut locations = Map::new();
    for n in 0..5_000 {
        participants
            .insert(format!("p{n:04}"), json!({"name": long, "email": format!("p{n}@example.org"), "roles": {"attendee": true, "owner": n == 0}}));
        keywords.insert(format!("k{n:04}{long}"), json!(true));
        alerts.insert(format!("a{n:04}"), json!({"trigger": {"offset": "PT5M"}}));
        locations.insert(format!("l{n:04}"), json!({"name": if n == 0 { huge.as_str() } else { "" }}));
    }
    let value = json!({
        "utcStart": "2026-11-03T09:00:00Z", "uid": huge, "title": huge, "description": huge, "timeZone": huge, "updated": huge, "duration": huge,
        "participants": participants, "keywords": keywords, "alerts": alerts, "locations": locations,
    });
    let read = mapped(&value).unwrap();
    assert!(read.uid.len() <= 1_024 && read.title.len() <= 1_024 && read.location.len() <= 1_024 && !read.title.is_empty());
    assert!(read.description.len() <= 32 << 10 && !read.description.is_empty());
    assert!(read.start_zone.is_empty() && read.modified == 0 && read.reminder_minutes.is_none());
    assert!(read.attendees.len() < 200 && !read.attendees.is_empty(), "{}", read.attendees.len());
    assert!(read.attendees.iter().all(|attendee| attendee.address.name.len() <= 1_024));
    assert!(read.organizer.is_some());
    assert_eq!(read.categories.len(), 50);
    assert!(read.categories.iter().all(|category| category.len() <= 255));
}

#[test]
fn a_hostile_list_of_calendars_and_hostile_events_are_read_within_bounds() {
    // Calendars that are none, one twice, and more than an account is taken to have.
    let mut calendars: Vec<Value> = vec![json!(null), json!(5), json!({"id": "a b"}), json!({"id": ["C0"]}), json!({"name": "No id"})];
    calendars
        .push(json!({"id": "C1", "name": "x".repeat(100_000), "color": "#".repeat(100_000), "sortOrder": -4, "isDefault": "yes", "myRights": 7}));
    calendars.push(json!({"id": "C1", "name": "Again", "isDefault": true}));
    calendars.extend((0..3_000).map(|n| json!({"id": format!("X{n}"), "sortOrder": u64::MAX, "isSubscribed": null})));
    let connection = connected(vec![answer("Calendar/get", json!({"list": calendars, "notFound": {"C1": true}}))]);
    let got = run(&connection);
    let list = listed(&got);
    assert_eq!(list.calendars.len(), MAX_REMOTE_CALENDARS);
    assert!(!list.complete, "the list named what is no calendar, and more than are kept");
    assert_eq!(list.calendars.iter().filter(|calendar| calendar.main).count(), 1);
    let first = &list.calendars[0];
    assert!(first.main && first.id == "C1" && first.name.len() <= 1_024 && first.color.is_none() && first.rights == Rights::Read);
    // (The script has run out: the server can not be had, and the run gives up on the events.)
    assert!(got.events.is_empty());
    assert_eq!(got.errors.len(), 1);
    assert!(got.errors[0].starts_with("Couldn't read the events of 100 calendars: "), "{}", got.errors[0]);
    assert_eq!(sent(&connection).len(), 1 + MAX_DOWN);

    // A list of what is no id, of an id twice, and of more than was asked for.
    let mut ids_listed: Vec<Value> = vec![json!(5), json!(null), json!("a b"), json!("../x"), json!(["E0"]), json!("E1"), json!("E1")];
    ids_listed.extend((0..2_000).map(|n| json!(format!("X{n}"))));
    let page = answer("CalendarEvent/query", json!({"ids": ids_listed, "total": "many", "position": -4}));
    // The events: one that is real, and the rest are not what was asked.
    let mut events: Vec<Value> = vec![json!(null), plain("E1"), plain("E1")];
    events.extend((0..3_000).map(|n| json!({"id": format!("Z{n}"), "utcStart": "2026-11-03T09:00:00Z"})));
    let replies = vec![
        listing(json!([{"id": "C1"}])),
        page,
        answer("CalendarEvent/query", json!({"ids": []})),
        answer("CalendarEvent/get", json!({"list": events, "notFound": {"E1": true}})),
    ];
    let connection = connected(replies);
    let got = run(&connection);
    assert!(listed(&got).complete);
    assert_eq!(ids(&got.events[0]), ["E1"]);
    assert!(!got.events[0].complete, "the list named what is no id, and events were left out");
    assert_eq!(got.errors.len(), 1, "{:?}", got.errors);
    // A page is no more than was asked for, and a call no more than the server takes.
    let asked = calls(&connection);
    assert_eq!(asked[2].1["position"], PAGE);
    assert_eq!(asked[3].1["ids"].as_array().unwrap().len(), MAX_CHUNK);
    // Whatever the answers, no request goes anywhere but to the API of the session.
    assert!(sent(&connection).iter().all(|request| request.url() == API));
    // A calendar whose id could carry something into a request is none; the list is then not whole.
    let connection = connected(vec![listing(json!([{"id": "C1\", \"x\": \""}, {"id": "../C2"}, {"id": ""}]))]);
    let got = run(&connection);
    assert!(listed(&got).calendars.is_empty() && !listed(&got).complete && got.events.is_empty());
    assert_eq!(got.errors, ["Couldn't read all the calendars from the server."]);
    assert_eq!(sent(&connection).len(), 1);
}

#[test]
fn no_line_names_a_calendar_an_event_or_the_account() {
    let secret = json!({"id": "CalendarOfAda", "name": "Ada's secret diary"});
    let hidden = json!({"id": "E1", "title": "Surprise party", "start": "2026-11-05T12:00:00", "timeZone": "Nowhere/At_All"});
    let connection =
        connected(vec![listing(json!([secret, calendar("C2")])), query(json!(["E1", "E2"])), get(json!([hidden])), refused("serverFail")]);
    let got = run(&connection);
    assert!(!got.errors.is_empty());
    for line in &got.errors {
        for word in ["CalendarOfAda", "diary", "Surprise", "ada@", "example.org", "E1", "tk-3f9a7c"] {
            assert!(!line.contains(word), "{line}");
        }
    }
    // Nor does the result, shown for a log.
    let shown = format!("{got:?}");
    assert!(!shown.contains("diary") && !shown.contains("ada@") && !shown.contains("Surprise"), "{shown}");
}

proptest! {
    #[test]
    fn any_value_as_an_event_is_read_or_passed_over_and_never_panics(value in any_json(), other in any_json(), text in ".{0,60}", number in any::<i64>()) {
        let _ = event("E1", &value, &text);
        let names = ["uid", "title", "description", "locations", "start", "timeZone", "duration", "showWithoutTime", "utcStart", "utcEnd", "updated",
            "keywords", "privacy", "status", "freeBusyStatus", "alerts", "participants"];
        // The value as every property, and as every property of an entry of every property.
        let mut flat = Map::new();
        let mut deep = Map::new();
        for name in names {
            flat.insert(name.to_string(), value.clone());
            let entry = json!({
                "name": value, "email": other, "sendTo": {"imip": value, "other": other}, "calendarAddress": text, "roles": value,
                "participationStatus": other, "trigger": {"@type": other, "offset": value, "relativeTo": text}, "@type": value,
            });
            deep.insert(name.to_string(), json!({"a": entry, text.clone(): value, "c": other, "d": {"trigger": value, "roles": {"owner": true, "attendee": other}}}));
        }
        for made in [Value::Object(flat), Value::Object(deep)] {
            if let Mapped::Event(read) = event("E1", &made, "ada@example.org") {
                prop_assert!(read.kind == RemoteEventKind::Single && read.id == "E1" && read.end >= read.start);
                prop_assert!(read.attendees.len() <= 200 && read.categories.len() <= 50);
            }
        }
        // Any text and any number where a time, a zone, a duration or an address is read.
        for all_day in [true, false] {
            let made = json!({
                "showWithoutTime": all_day, "start": text, "timeZone": text, "duration": text, "utcStart": text, "utcEnd": text, "updated": text,
                "alerts": {"a": {"trigger": {"offset": text}}},
                "participants": {"a": {"sendTo": {"imip": format!("mailto:{text}")}, "roles": {"owner": true}}, "b": {"email": text, "roles": {"attendee": true}}},
            });
            let _ = event("E1", &made, &text);
            let day = number.rem_euclid(28) + 1;
            let spans = [format!("P{number}D"), format!("-PT{number}S"), format!("P{}WT{}H", number.unsigned_abs(), number.unsigned_abs()), "P40000DT3456000000S".to_string()];
            for span in spans {
                for (start, zone) in [("9999-12-31T23:59:59".to_string(), "Pacific/Kiritimati"), ("0001-01-01T00:00:00".to_string(), "Etc/GMT+12"),
                    (format!("2026-03-{day:02}T02:30:00"), "Europe/Berlin")] {
                    let made = json!({"showWithoutTime": all_day, "start": start, "timeZone": zone, "duration": span, "alerts": {"a": {"trigger": {"offset": span}}}});
                    if let Mapped::Event(read) = event("E1", &made, "") {
                        prop_assert!(read.end >= read.start);
                    }
                }
            }
        }
    }

    #[test]
    fn any_text_as_a_duration_is_read_within_bounds_and_never_panics(text in ".{0,40}", made in "[+-]?P?([0-9]{0,20}[WDTHMS.]){0,6}", weeks in 0u64..6_000, days in 0u64..50_000, hours in 0u64..2_000_000) {
        for text in [text, made] {
            if let Some(span) = duration(&text) {
                prop_assert!((0..=40_000).contains(&span.days) && (0..=40_000 * 86_400).contains(&span.seconds));
                prop_assert!(span.total() >= 0);
            }
        }
        let written = format!("P{weeks}W{days}DT{hours}H");
        let all = weeks * 7 + days;
        let expected = (all <= 40_000 && hours * 3_600 <= 40_000 * 86_400).then(|| Span { negative: false, days: i64::try_from(all).unwrap(), seconds: i64::try_from(hours * 3_600).unwrap() });
        prop_assert_eq!(duration(&written), expected);
    }

    #[test]
    fn any_answers_are_read_or_refused_and_never_panic(first in any_json(), second in any_json(), third in any_json()) {
        let real_list = json!({"list": [{"id": "C1"}, second.clone(), {"id": "C2", "isDefault": third.clone()}]});
        let real_page = json!({"ids": ["E1", "E2"], "total": 2});
        let events = json!({"list": [plain("E1"), third.clone()], "notFound": first.clone()});
        for (calendars, page, read) in [(first.clone(), second.clone(), third.clone()), (real_list.clone(), second.clone(), third.clone()), (real_list.clone(), real_page.clone(), third.clone()), (real_list, real_page, events)] {
            let replies = vec![
                answer("Calendar/get", calendars),
                answer("CalendarEvent/query", page.clone()),
                answer("CalendarEvent/get", read.clone()),
                answer("CalendarEvent/query", page),
                answer("CalendarEvent/get", read),
            ];
            let connection = connected(replies);
            let got = run(&connection);
            prop_assert!(got.calendars.is_some() || got.events.is_empty());
            if let Some(list) = &got.calendars {
                prop_assert!(list.calendars.len() <= MAX_REMOTE_CALENDARS);
                prop_assert_eq!(list.calendars.iter().filter(|calendar| calendar.main).count(), usize::from(!list.calendars.is_empty()));
                prop_assert!(list.calendars.iter().all(|calendar| calendar.rights == Rights::Read));
                prop_assert!(got.events.len() <= list.calendars.len());
                prop_assert!(got.events.iter().all(|down| down.calendar.is_empty() || list.calendars.iter().any(|calendar| calendar.id == down.calendar)));
            }
            for down in &got.events {
                prop_assert!(down.events.iter().all(|event| ["E1", "E2"].contains(&event.id.as_str())) || !down.complete || down.events.len() <= 500);
            }
            prop_assert!(got.events.iter().any(|down| !down.complete) || got.events.len() == got.calendars.as_ref().map_or(0, |list| list.calendars.len()) || !got.errors.is_empty());
            prop_assert!(sent(&connection).iter().all(|request| request.url() == API));
        }
    }
}
