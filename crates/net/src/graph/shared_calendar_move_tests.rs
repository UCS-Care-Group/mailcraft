// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Moving an appointment between the owner's calendar and a shared mailbox's (or one opened by
//! address) through Graph (#70): the copy is made at the target's own address first, the
//! original is deleted at its own address only after that went through, and nothing of the shared
//! side goes to `/me`. The order is the engine's; here the two halves are sent the way a host
//! sends them, against the stand-in ([`standin`]). Every address is invented.

use mailcraft_model::remote::opened_id;
use mailcraft_time::Date;

use super::standin::{CalendarAccess, StandIn, TOKEN};
use super::*;

const SHARED: &str = "support@example.org";

fn at(day: u8, hour: u8) -> Timestamp {
    DateTime::new(Date::new(2026, 10, day).unwrap(), hour, 0, 0).unwrap().to_timestamp(0)
}

fn copy_of(title: &str, start: Timestamp, calendar: &str, token: &str) -> EventOp {
    EventOp {
        kind: EventChangeKind::Create,
        event: 1,
        token: token.into(),
        calendar: calendar.into(),
        fields: EventFields {
            title: Some(title.into()),
            location: Some(String::new()),
            description: Some(String::new()),
            time: Some(EventTime { start, end: start + 3_600, all_day: false }),
            show_as: Some(ShowAs::Busy),
            private: Some(false),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A stand-in with a shared mailbox (a calendar the owner may edit, one they may only read) holding
/// "Stand-up", and the owner's own calendar holding "Dentist". Their calendars' names.
fn served() -> (StandIn, String, String) {
    let server = StandIn::start().unwrap();
    let (main, team) = {
        let mut state = server.state();
        state.add_other(SHARED, true);
        let main = state.add_other_calendar(SHARED, "Calendar", true);
        let team = state.add_other_calendar(SHARED, "Team", false);
        state.add_other_event(SHARED, &main, "Stand-up", at(14, 9), "busy");
        state.share_calendars(SHARED, CalendarAccess::Open);
        let own = state.main_calendar();
        state.add_event(&own, "Dentist", at(14, 15));
        (main, team)
    };
    (server, main, team)
}

/// Send `ops` as one account's upload would be: the shared mailbox's (`SHARED`) or the owner's (`""`).
fn push(server: &StandIn, shared: &str, ops: Vec<EventOp>) -> Vec<EventOutcome> {
    let upload =
        EventsUpload { account: 5, email: if shared.is_empty() { "owner@example.org".into() } else { shared.into() }, ops, ..Default::default() };
    let options = EventOptions { shared: shared.into(), ..Default::default() };
    let uploaded = push_events_at(&server.url(), TOKEN, &upload, &options);
    assert!(!uploaded.token_refused);
    uploaded.outcomes
}

fn titles_of(server: &StandIn, of: &str) -> Vec<String> {
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

fn own_id(server: &StandIn, title: &str) -> String {
    server.state().events.iter().find(|e| e.subject == title).map(|e| e.id.clone()).unwrap()
}

fn shared_id(server: &StandIn, title: &str) -> String {
    let mut state = server.state();
    state.other(SHARED).and_then(|o| o.events.iter().find(|e| e.subject == title).map(|e| e.id.clone())).unwrap()
}

/// What was written, in order, as `METHOD path`.
fn writes(server: &StandIn) -> Vec<String> {
    server.state().requests.iter().filter(|a| a.method != "GET").map(|a| format!("{} {}", a.method, a.path.join("/"))).collect()
}

fn delete(remote: String) -> EventOp {
    EventOp { kind: EventChangeKind::Delete, event: 1, remote, ..Default::default() }
}

#[test]
fn from_the_owners_calendar_into_a_shared_mailbox_the_copy_goes_to_its_address_and_the_original_to_me_after() {
    let (server, main, _) = served();
    let copy = push(&server, SHARED, vec![copy_of("Dentist", at(14, 15), &main, "0f3c1d2e-aaaa-4bbb-8ccc-0000000000a1")]);
    assert_eq!(copy[0].result, EventResult::Done, "{}", copy[0].error);
    assert_eq!(titles_of(&server, SHARED), ["Dentist", "Stand-up"]);
    assert_eq!(owns(&server), ["Dentist"], "the original is untouched until the copy is made");
    let original = own_id(&server, "Dentist");
    assert_eq!(push(&server, "", vec![delete(original)])[0].result, EventResult::Done);
    assert!(owns(&server).is_empty());
    assert_eq!(titles_of(&server, SHARED), ["Dentist", "Stand-up"], "and the copy stays");
    let sent = writes(&server);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].starts_with("POST ") && sent[0].contains("users/support@example.org"), "{sent:?}");
    assert!(sent[1].starts_with("DELETE ") && sent[1].contains("/me/") && !sent[1].contains("support@example.org"), "{sent:?}");
}

#[test]
fn from_a_shared_mailbox_into_the_owners_calendar_the_copy_goes_to_me_and_the_original_to_its_address_after() {
    let (server, ..) = served();
    let copy = push(&server, "", vec![copy_of("Stand-up", at(14, 9), "", "0f3c1d2e-aaaa-4bbb-8ccc-0000000000a2")]);
    assert_eq!(copy[0].result, EventResult::Done, "{}", copy[0].error);
    assert_eq!(owns(&server), ["Dentist", "Stand-up"]);
    assert_eq!(titles_of(&server, SHARED), ["Stand-up"], "the original is untouched until the copy is made");
    let original = shared_id(&server, "Stand-up");
    assert_eq!(push(&server, SHARED, vec![delete(original)])[0].result, EventResult::Done);
    assert!(titles_of(&server, SHARED).is_empty());
    assert_eq!(owns(&server), ["Dentist", "Stand-up"]);
    let sent = writes(&server);
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(sent[0].starts_with("POST ") && sent[0].contains("/me/") && !sent[0].contains("support@example.org"), "{sent:?}");
    assert!(sent[1].starts_with("DELETE ") && sent[1].contains("users/support@example.org"), "{sent:?}");
}

#[test]
fn a_calendar_opened_by_address_gets_the_copy_at_that_address_and_nothing_of_it_goes_to_me() {
    let (server, ..) = served();
    let copy = push(&server, "", vec![copy_of("Dentist", at(14, 15), &opened_id(SHARED), "0f3c1d2e-aaaa-4bbb-8ccc-0000000000a3")]);
    assert_eq!(copy[0].result, EventResult::Done, "{}", copy[0].error);
    assert_eq!(titles_of(&server, SHARED), ["Dentist", "Stand-up"]);
    assert_eq!(owns(&server), ["Dentist"]);
    let original = shared_id(&server, "Stand-up");
    // Out of it again: deleted through the calendar's own address, not the owner's.
    let out = EventOp { calendar: opened_id(SHARED), ..delete(original) };
    assert_eq!(push(&server, "", vec![out])[0].result, EventResult::Done);
    assert_eq!(titles_of(&server, SHARED), ["Dentist"]);
    let sent = writes(&server);
    assert!(sent.len() == 2 && sent.iter().all(|line| line.contains("users/support@example.org") && !line.contains("/me/")), "{sent:?}");
}

#[test]
fn a_copy_the_target_refuses_leaves_the_original_where_it_is_and_a_failed_delete_leaves_both() {
    let (server, _, team) = served();
    // Into a calendar the owner may only read: refused, nothing is written, and the host (told
    // by the outcome) has no reason to delete anything.
    let refused = push(&server, SHARED, vec![copy_of("Dentist", at(14, 15), &team, "0f3c1d2e-aaaa-4bbb-8ccc-0000000000a4")]);
    assert_eq!(refused[0].result, EventResult::Denied);
    assert_eq!(titles_of(&server, SHARED), ["Stand-up"]);
    assert_eq!(owns(&server), ["Dentist"]);
    assert!(writes(&server).iter().all(|line| !line.starts_with("DELETE")), "{:?}", writes(&server));
    // The copy is made; then the original can't be deleted (an id the server doesn't have): the
    // outcome says so, and the copy and the original both stay.
    let main = server.state().other(SHARED).and_then(|o| o.calendars.iter().find(|c| c.name == "Calendar").map(|c| c.id.clone())).unwrap();
    assert_eq!(
        push(&server, SHARED, vec![copy_of("Dentist", at(14, 15), &main, "0f3c1d2e-aaaa-4bbb-8ccc-0000000000a5")])[0].result,
        EventResult::Done
    );
    let failed = push(&server, "", vec![delete("no-such-event".into())]);
    assert_ne!(failed[0].result, EventResult::Done);
    assert_eq!(owns(&server), ["Dentist"]);
    assert_eq!(titles_of(&server, SHARED), ["Dentist", "Stand-up"]);
}
