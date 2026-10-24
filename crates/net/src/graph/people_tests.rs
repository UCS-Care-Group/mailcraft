// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars opened by address (#70), held against the stand-in: what is asked at
//! `/users/{address}/calendar…`, what the owner may do with it, and that nothing of it is asked
//! at `/me`.

use mailcraft_model::remote::{EventChangeKind, EventFields, EventOp, EventResult, EventsUpload, OpenedWhy, opened_id};
use mailcraft_model::{Rights, ShowAs};
use mailcraft_time::Timestamp;

use super::people::open_calendar_at;
use super::standin::{Access, StandIn, TOKEN};
use super::*;

const NOW: Timestamp = 1_790_100_000;

fn opened(server: &StandIn, address: &str) -> Opened {
    open_calendar_at(&server.url(), TOKEN, address)
}

/// What of the requests the stand-in has seen reached the owner's own `/me`, apart from the
/// schedule question (which asks about any address and is the one thing that does).
fn on_me(server: &StandIn) -> Vec<String> {
    let seen = server.state().requests.clone();
    seen.iter()
        .filter(|asked| {
            asked.path.get(1).is_some_and(|piece| piece == "me") && asked.path.get(2..) != Some(&["calendar".to_string(), "getSchedule".to_string()])
        })
        .map(|asked| format!("{} /{}", asked.method, asked.path.join("/")))
        .collect()
}

fn query(address: &str) -> EventsQuery {
    EventsQuery { calendar: opened_id(address), from: NOW - 86_400, to: NOW + 7 * 86_400, series: Vec::new(), shared: String::new() }
}

#[test]
fn a_calendar_that_is_shared_for_reading_is_opened_with_its_owners_name_and_read_rights() {
    let server = StandIn::start().unwrap();
    server.state().add_person("cy@example.org", "Cy Colleague", Access::Read);
    let Opened::Read(calendar) = opened(&server, "cy@example.org") else { panic!("not opened") };
    assert_eq!(calendar.id, "user:cy@example.org");
    assert_eq!((calendar.name.as_str(), calendar.rights, calendar.main), ("Cy Colleague", Rights::Read, false));
    assert_eq!(calendar.owner.as_ref().map(|owner| owner.address.as_str()), Some("cy@example.org"));
    assert!(on_me(&server).is_empty(), "{:?}", on_me(&server));
    let seen = server.state().requests.clone();
    assert!(seen.iter().all(|asked| asked.path[1] == "users" && asked.method == "GET"));
}

#[test]
fn a_calendar_that_can_be_changed_is_opened_with_edit_rights() {
    let server = StandIn::start().unwrap();
    server.state().add_person("dee@example.org", "Dee Delegate", Access::Edit);
    let Opened::Read(calendar) = opened(&server, "dee@example.org") else { panic!("not opened") };
    assert_eq!(calendar.rights, Rights::Edit);
}

#[test]
fn a_person_who_shares_only_free_and_busy_is_opened_as_blocks() {
    let server = StandIn::start().unwrap();
    server.state().add_person("fay@example.org", "Fay", Access::FreeBusy);
    server.state().person("fay@example.org").unwrap().busy =
        vec![(NOW + 3_600, NOW + 7_200, "busy".into()), (NOW + 9_000, NOW + 10_800, "oof".into())];
    let Opened::Read(calendar) = opened(&server, "fay@example.org") else { panic!("not opened") };
    assert_eq!((calendar.rights, calendar.name.as_str()), (Rights::FreeBusy, "fay@example.org"));
    // Its events can't be listed: the blocks come from the schedule, named by how they show.
    let download = events_in_at(&server.url(), TOKEN, &query("fay@example.org")).unwrap();
    assert_eq!(download.calendar, "user:fay@example.org");
    assert!(download.free_busy);
    let shown: Vec<ShowAs> = download.events.iter().map(|event| event.show_as).collect();
    assert_eq!(shown, [ShowAs::Busy, ShowAs::OutOfOffice]);
    assert!(download.events.iter().all(|event| event.title.is_empty()));
    assert!(download.events.iter().all(|event| event.attendees.is_empty() && event.description.is_empty() && !event.id.is_empty()));
    // Nothing of the calendar itself came from `/me`.
    assert!(on_me(&server).is_empty(), "{:?}", on_me(&server));
}

#[test]
fn a_refusal_says_why() {
    let server = StandIn::start().unwrap();
    server.state().add_person("gus@example.org", "Gus", Access::None);
    server.state().add_person("hal@example.org", "Hal", Access::Missing);
    assert_eq!(opened(&server, "gus@example.org"), Opened::Refused(OpenedWhy::NoRights));
    assert_eq!(opened(&server, "hal@example.org"), Opened::Refused(OpenedWhy::NoSuchPerson));
    assert_eq!(opened(&server, "nobody@example.org"), Opened::Refused(OpenedWhy::NoSuchPerson));
    server.state().fail_next.push(("GET".into(), 503, "ServiceUnavailable".into()));
    server.state().add_person("ivy@example.org", "Ivy", Access::Read);
    assert_eq!(opened(&server, "ivy@example.org"), Opened::Refused(OpenedWhy::Unreachable));
    assert!(on_me(&server).is_empty());
    assert!(matches!(open_calendar_at(&server.url(), "not a token\r\n", "ivy@example.org"), Opened::SignInRefused));
}

#[test]
fn an_address_that_isnt_one_asks_nothing() {
    let server = StandIn::start().unwrap();
    let before = server.state().requests.len();
    for hostile in [
        "",
        "no-at-sign",
        "a/../b@example.org",
        "a@example.org/../../me",
        "a b@example.org",
        "a@b@example.org",
        "é@example.org",
        "a%2f@example.org",
        "a?x=1@example.org",
        "x@example.org#y",
    ] {
        assert_eq!(opened(&server, hostile), Opened::Refused(OpenedWhy::NoSuchPerson), "{hostile}");
        // Nor are events or changes asked for under an id that says "by address" and names none.
        let events = events_in_at(
            &server.url(),
            TOKEN,
            &EventsQuery { calendar: format!("user:{hostile}"), from: NOW, to: NOW + 3_600, series: Vec::new(), shared: String::new() },
        );
        assert!(events.is_err(), "{hostile}");
        let create = EventOp {
            kind: EventChangeKind::Create,
            token: "0f8fad5b-d9cb-469f-a165-70867728950e".into(),
            calendar: format!("user:{hostile}"),
            fields: EventFields { title: Some("x".into()), ..Default::default() },
            ..Default::default()
        };
        let upload = EventsUpload { account: 1, email: "owner@example.org".into(), ops: vec![create], ..Default::default() };
        let sent = push_events_at(&server.url(), TOKEN, &upload, &EventOptions::default());
        assert!(sent.outcomes.iter().all(|outcome| outcome.result == EventResult::Refused), "{hostile}");
    }
    assert_eq!(server.state().requests.len(), before, "something was asked: {:?}", server.state().requests);
}

#[test]
fn the_events_of_an_opened_calendar_come_from_the_persons_mailbox_and_never_from_me() {
    let server = StandIn::start().unwrap();
    server.state().add_person("jo@example.org", "Jo", Access::Read);
    server.state().add_person_event("jo@example.org", "Jo's dentist", NOW + 3_600);
    // The owner's own calendar has an event too, which must not appear in Jo's.
    let main = server.state().main_calendar();
    server.state().add_event(&main, "Owner's lunch", NOW + 7_200);
    let download = events_in_at(&server.url(), TOKEN, &query("jo@example.org")).unwrap();
    let titles: Vec<&str> = download.events.iter().map(|event| event.title.as_str()).collect();
    assert_eq!(titles, ["Jo's dentist"]);
    assert_eq!(download.calendar, "user:jo@example.org");
    assert!(on_me(&server).is_empty(), "{:?}", on_me(&server));
    let seen = server.state().requests.clone();
    assert!(seen.iter().all(|asked| asked.path[1] == "users" && asked.path[2] == "jo@example.org"), "{seen:?}");
}

#[test]
fn a_series_of_an_opened_calendar_is_asked_for_at_the_persons_mailbox() {
    let server = StandIn::start().unwrap();
    server.state().add_person("kim@example.org", "Kim", Access::Read);
    // A series that is expected and isn't brought by the stretch is asked about by name: Graph
    // doesn't have it, and the answer says so.
    let mut asked = query("kim@example.org");
    asked.series = vec!["AAMkAGVmMDEz-series-that-is-gone=".into()];
    let download = events_in_at(&server.url(), TOKEN, &asked).unwrap();
    assert_eq!(download.gone, ["AAMkAGVmMDEz-series-that-is-gone="]);
    assert!(on_me(&server).is_empty(), "{:?}", on_me(&server));
    let seen = server.state().requests.clone();
    let batch = seen.iter().find(|asked| asked.path.last().is_some_and(|last| last == "$batch")).expect("the series were asked for together");
    assert!(batch.body.contains("/users/kim@example.org/events/"), "{}", batch.body);
}

fn new_event(mark: &str, calendar: &str, title: &str) -> EventOp {
    let time = mailcraft_model::remote::EventTime { start: NOW + 3_600, end: NOW + 7_200, all_day: false };
    let fields = EventFields { title: Some(title.into()), time: Some(time), ..Default::default() };
    EventOp { kind: EventChangeKind::Create, event: 1, token: mark.into(), calendar: calendar.into(), fields, ..Default::default() }
}

#[test]
fn an_event_is_made_changed_and_deleted_in_a_calendar_that_can_be_changed_at_the_persons_address() {
    let server = StandIn::start().unwrap();
    server.state().add_person("lee@example.org", "Lee", Access::Edit);
    let calendar = opened_id("lee@example.org");
    let push = |ops: Vec<EventOp>| {
        let upload = EventsUpload { account: 1, email: "owner@example.org".into(), ops, ..Default::default() };
        push_events_at(&server.url(), TOKEN, &upload, &EventOptions::default()).outcomes
    };
    let made = push(vec![new_event("0f8fad5b-d9cb-469f-a165-70867728950e", &calendar, "Review")]);
    assert_eq!(made[0].result, EventResult::Done, "{:?}", made[0].error);
    let remote = made[0].remote.clone();
    assert!(!remote.is_empty());
    let change = EventOp {
        kind: EventChangeKind::Update,
        event: 1,
        remote: remote.clone(),
        calendar: calendar.clone(),
        fields: EventFields { title: Some("Review (moved)".into()), ..Default::default() },
        ..Default::default()
    };
    assert_eq!(push(vec![change])[0].result, EventResult::Done);
    let download = events_in_at(&server.url(), TOKEN, &query("lee@example.org")).unwrap();
    assert_eq!(download.events.iter().map(|event| event.title.as_str()).collect::<Vec<_>>(), ["Review (moved)"]);
    let gone = EventOp { kind: EventChangeKind::Delete, event: 1, remote, calendar: calendar.clone(), ..Default::default() };
    assert_eq!(push(vec![gone])[0].result, EventResult::Done);
    assert!(events_in_at(&server.url(), TOKEN, &query("lee@example.org")).unwrap().events.is_empty());
    assert!(on_me(&server).is_empty(), "{:?}", on_me(&server));
    // None of it is in the owner's own calendar.
    assert!(server.state().events.is_empty());
}

#[test]
fn a_change_to_a_calendar_that_can_only_be_read_is_denied() {
    let server = StandIn::start().unwrap();
    server.state().add_person("max@example.org", "Max", Access::Read);
    let upload = EventsUpload {
        account: 1,
        email: "owner@example.org".into(),
        ops: vec![new_event("0f8fad5b-d9cb-469f-a165-70867728950e", &opened_id("max@example.org"), "Nope")],
        ..Default::default()
    };
    let outcomes = push_events_at(&server.url(), TOKEN, &upload, &EventOptions::default()).outcomes;
    assert_eq!(outcomes[0].result, EventResult::Denied);
    assert!(on_me(&server).is_empty(), "{:?}", on_me(&server));
}

#[test]
fn an_id_a_servers_list_brings_that_says_it_was_opened_by_address_is_not_a_calendar() {
    let entry = serde_json::json!({"id": "user:victim@example.org", "name": "Trick", "canEdit": true});
    assert!(calendar_of(&entry, "").is_none());
}

#[test]
fn the_blocks_of_a_wide_stretch_are_asked_for_in_stretches() {
    let server = StandIn::start().unwrap();
    server.state().add_person("fay@example.org", "Fay", Access::FreeBusy);
    let now = mailcraft_time::now();
    server.state().person("fay@example.org").unwrap().busy = vec![(now + 3_600, now + 7_200, "busy".into())];
    // A month back to a year ahead, as a Send/Receive asks: more than a schedule is asked for at once.
    let wide = EventsQuery {
        calendar: opened_id("fay@example.org"),
        from: now - 31 * 86_400,
        to: now + 366 * 86_400,
        series: Vec::new(),
        shared: String::new(),
    };
    let download = events_in_at(&server.url(), TOKEN, &wide).unwrap();
    assert_eq!(download.events.len(), 1);
    // (Asked in stretches a schedule can be asked for: the whole window is covered.)
    assert!(download.complete && download.from == wide.from && download.to == wide.to);
}
