// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Writing to a shared mailbox's calendars through Graph (#70): every change goes to
//! `/users/{address}/…` with the owner's token and never to `/me`, against the stand-in
//! ([`standin`]). Nothing here leaves this machine; every address is invented.

use mailcraft_time::Date;

use super::standin::{CalendarAccess, StandIn, TOKEN};
use super::*;

const SHARED: &str = "support@example.org";

fn at(day: u8, hour: u8) -> Timestamp {
    DateTime::new(Date::new(2026, 10, day).unwrap(), hour, 0, 0).unwrap().to_timestamp(0)
}

fn hour(start: Timestamp) -> Option<EventTime> {
    Some(EventTime { start, end: start + 3_600, all_day: false })
}

fn fields(title: &str, start: Timestamp) -> EventFields {
    EventFields {
        title: Some(title.into()),
        location: Some(String::new()),
        description: Some(String::new()),
        time: hour(start),
        show_as: Some(ShowAs::Busy),
        private: Some(false),
        reminder: Some(mailcraft_model::remote::EventReminder { on: true, minutes: 15 }),
        ..Default::default()
    }
}

/// A stand-in with a shared mailbox whose main calendar the owner may edit and whose team calendar
/// they may only read, and the owner's own calendar with an event of its own.
fn served() -> (StandIn, String, String) {
    let server = StandIn::start().unwrap();
    let (main, team) = {
        let mut state = server.state();
        state.add_other(SHARED, true);
        let main = state.add_other_calendar(SHARED, "Calendar", true);
        let team = state.add_other_calendar(SHARED, "Team", false);
        state.add_other_event(SHARED, &main, "Stand-up", at(14, 9), "busy");
        state.add_other_event(SHARED, &team, "Rota", at(15, 9), "busy");
        state.share_calendars(SHARED, CalendarAccess::Open);
        let own = state.main_calendar();
        state.add_event(&own, "Dentist", at(14, 15));
        (main, team)
    };
    (server, main, team)
}

fn push(server: &StandIn, shared: &str, ops: Vec<EventOp>) -> Vec<EventOutcome> {
    let upload = EventsUpload { account: 5, email: SHARED.into(), ops, ..Default::default() };
    let options = EventOptions { shared: shared.into(), ..Default::default() };
    let uploaded = push_events_at(&server.url(), TOKEN, &upload, &options);
    assert!(!uploaded.token_refused);
    uploaded.outcomes
}

fn titles(server: &StandIn, of: &str) -> Vec<String> {
    let mut state = server.state();
    let mut titles: Vec<String> = match state.other(of) {
        Some(other) => other.events.iter().map(|e| e.subject.clone()).collect(),
        None => Vec::new(),
    };
    titles.sort();
    titles
}

fn owns(server: &StandIn) -> Vec<String> {
    let mut titles: Vec<String> = server.state().events.iter().map(|e| e.subject.clone()).collect();
    titles.sort();
    titles
}

fn asked(server: &StandIn) -> Vec<String> {
    server.state().requests.iter().filter(|a| a.method != "GET").map(|a| format!("{} {}", a.method, a.path.join("/"))).collect()
}

fn remote_of(server: &StandIn, title: &str) -> String {
    let mut state = server.state();
    state.other(SHARED).and_then(|o| o.events.iter().find(|e| e.subject == title).map(|e| e.id.clone())).unwrap()
}

#[test]
fn events_are_made_changed_and_deleted_in_a_shared_mailboxs_calendar_and_never_at_me() {
    let (server, main, _) = served();
    let created = EventOp {
        kind: EventChangeKind::Create,
        event: 1,
        token: "0f3c1d2e-aaaa-4bbb-8ccc-000000000001".into(),
        fields: fields("Review", at(16, 10)),
        ..Default::default()
    };
    let outcomes = push(&server, SHARED, vec![created]);
    assert_eq!(outcomes[0].result, EventResult::Done, "{}", outcomes[0].error);
    assert!(!outcomes[0].remote.is_empty());
    assert_eq!(titles(&server, SHARED), ["Review", "Rota", "Stand-up"]);
    assert_eq!(owns(&server), ["Dentist"], "the owner's own calendar was not written to");

    // Made in one calendar of its own, by that calendar's name.
    let into_main = EventOp {
        kind: EventChangeKind::Create,
        event: 2,
        token: "0f3c1d2e-aaaa-4bbb-8ccc-000000000002".into(),
        calendar: main,
        fields: fields("Rota change", at(17, 10)),
        ..Default::default()
    };
    assert_eq!(push(&server, SHARED, vec![into_main])[0].result, EventResult::Done);

    // Changed, and deleted.
    let id = remote_of(&server, "Stand-up");
    let change = EventOp {
        kind: EventChangeKind::Update,
        event: 3,
        remote: id.clone(),
        fields: EventFields { title: Some("Daily stand-up".into()), ..Default::default() },
        ..Default::default()
    };
    assert_eq!(push(&server, SHARED, vec![change])[0].result, EventResult::Done);
    assert!(titles(&server, SHARED).contains(&"Daily stand-up".to_string()));
    let gone = EventOp { kind: EventChangeKind::Delete, event: 3, remote: id, ..Default::default() };
    assert_eq!(push(&server, SHARED, vec![gone])[0].result, EventResult::Done);
    assert!(!titles(&server, SHARED).contains(&"Daily stand-up".to_string()));

    assert_eq!(owns(&server), ["Dentist"]);
    let writes = asked(&server);
    assert!(writes.len() == 4 && writes.iter().all(|line| line.contains("users/support@example.org")), "{writes:?}");
    let all: Vec<String> = server.state().requests.iter().map(|a| a.path.join("/")).collect();
    assert!(all.iter().all(|path| !path.contains("/me/") && !path.ends_with("/me") && !path.starts_with("v1.0/me")), "nothing at /me: {all:?}");
}

#[test]
fn a_calendar_the_owner_may_only_read_refuses_the_change_and_nothing_is_written() {
    let (server, _, team) = served();
    let made = EventOp {
        kind: EventChangeKind::Create,
        event: 1,
        token: "0f3c1d2e-aaaa-4bbb-8ccc-000000000003".into(),
        calendar: team,
        fields: fields("Sneaked in", at(16, 10)),
        ..Default::default()
    };
    let outcomes = push(&server, SHARED, vec![made]);
    assert_eq!(outcomes[0].result, EventResult::Denied);
    assert!(!titles(&server, SHARED).contains(&"Sneaked in".to_string()));
    let id = remote_of(&server, "Rota");
    let change = EventOp {
        kind: EventChangeKind::Update,
        event: 3,
        remote: id,
        fields: EventFields { title: Some("x".into()), ..Default::default() },
        ..Default::default()
    };
    // (Found through the mailbox's events: the change is refused for the calendar it is in.)
    let outcomes = push(&server, SHARED, vec![change]);
    assert_eq!(outcomes[0].result, EventResult::Denied);
    assert!(titles(&server, SHARED).contains(&"Rota".to_string()));
    assert_eq!(owns(&server), ["Dentist"]);
}

#[test]
fn a_meeting_in_a_shared_mailbox_is_answered_called_off_and_forwarded_from_its_address() {
    let (server, main, _) = served();
    let (answered, called_off) = {
        let mut state = server.state();
        let invited = state.add_other_event(SHARED, &main, "Planning", at(20, 9), "busy");
        let own = state.add_other_event(SHARED, &main, "Team lunch", at(21, 12), "busy");
        if let Some(other) = state.other(SHARED) {
            for event in &mut other.events {
                if event.id == invited {
                    event.is_organizer = false;
                    event.attendees = vec![json!({"emailAddress": {"address": SHARED, "name": "Support"}, "type": "required"})];
                }
                if event.id == own {
                    event.is_organizer = true;
                    event.attendees = vec![json!({"emailAddress": {"address": "kit@example.net", "name": "Kit"}, "type": "required"})];
                }
            }
        }
        (invited, own)
    };
    let respond = |response: Response, comment: &str| EventOp {
        kind: EventChangeKind::Respond,
        event: 9,
        remote: answered.clone(),
        response,
        send: true,
        comment: comment.into(),
        ..Default::default()
    };
    let outcomes = push(&server, SHARED, vec![respond(Response::Tentative, "Maybe")]);
    assert_eq!(outcomes[0].result, EventResult::Done, "{}", outcomes[0].error);
    let outcomes = push(&server, SHARED, vec![respond(Response::Accepted, "Yes")]);
    assert_eq!(outcomes[0].result, EventResult::Done, "{}", outcomes[0].error);
    let proposal = EventOp { proposed: hour(at(22, 9)), ..respond(Response::Declined, "Later?") };
    let outcomes = push(&server, SHARED, vec![proposal]);
    assert_eq!(outcomes[0].result, EventResult::Done, "{}", outcomes[0].error);
    // It lands in the shared mailbox's calendar (declined, it is gone from it), and the answers
    // went out: the stand-in kept the mail the server would have sent.
    assert!(!titles(&server, SHARED).contains(&"Planning".to_string()));
    let mail = server.state().mail.clone();
    assert!(mail.iter().any(|line| line.starts_with("response:Planning:tentativelyAccept")), "{mail:?}");
    assert!(mail.iter().any(|line| line.starts_with("response:Planning:accept")), "{mail:?}");
    assert!(mail.iter().any(|line| line.starts_with("proposal:Planning:")), "{mail:?}");

    // A meeting the shared mailbox called is called off, and can be forwarded.
    let forward = EventOp {
        kind: EventChangeKind::Forward,
        event: 10,
        remote: called_off.clone(),
        send: true,
        to: vec![Address { name: "Lee".into(), email: "lee@example.net".into() }],
        ..Default::default()
    };
    assert_eq!(push(&server, SHARED, vec![forward])[0].result, EventResult::Done);
    let cancel = EventOp { kind: EventChangeKind::Delete, event: 10, remote: called_off, cancel: true, comment: "Off".into(), ..Default::default() };
    assert_eq!(push(&server, SHARED, vec![cancel])[0].result, EventResult::Done);
    assert!(!titles(&server, SHARED).contains(&"Team lunch".to_string()));
    let mail = server.state().mail.clone();
    assert!(
        mail.iter().any(|line| line.starts_with("forward:Team lunch:1")) && mail.iter().any(|line| line.starts_with("cancellation:Team lunch:1")),
        "{mail:?}"
    );

    // Every one of them for the shared mailbox's address, none at /me.
    let writes = asked(&server);
    assert!(writes.iter().all(|line| line.contains("users/support@example.org")), "{writes:?}");
    for action in ["tentativelyAccept", "accept", "decline", "forward", "cancel"] {
        assert!(writes.iter().any(|line| line.ends_with(action)), "{action}: {writes:?}");
    }
    assert!(server.state().requests.iter().all(|a| !a.path.join("/").starts_with("v1.0/me")));
}

#[test]
fn an_address_that_names_no_shared_mailbox_writes_nothing_anywhere() {
    let (server, _, _) = served();
    for address in ["support@example.org/../me", "a b@example.org", "support"] {
        let made = EventOp {
            kind: EventChangeKind::Create,
            event: 1,
            token: "0f3c1d2e-aaaa-4bbb-8ccc-000000000004".into(),
            fields: fields("Lost", at(16, 10)),
            ..Default::default()
        };
        let outcomes = push(&server, address, vec![made]);
        assert_eq!(outcomes[0].result, EventResult::Refused, "{address}");
    }
    assert!(server.state().requests.is_empty(), "not one request, at /me or anywhere");
    assert_eq!(owns(&server), ["Dentist"]);
}
