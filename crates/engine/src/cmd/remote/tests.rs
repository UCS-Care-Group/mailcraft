// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Downloads merged into the sample mailbox. The server's side is written out by hand, as
//! `mailcraft-net` brings it from Microsoft Graph.

use mailcraft_model::remote::{
    ContactsDownload, EventChange, EventChangeKind, EventFields, EventOp, EventOutcome, EventReminder, EventRepeat, EventResult, EventTime,
    EventWall, EventsDownload, EventsUpload, EventsUploaded, RemoteContact, RemoteEvent, RemoteEventKind,
};
use mailcraft_model::{Address, Attendee, Contact, Event, Freq, Id, Labeled, Origin, Recurrence, Response, ShowAs};
use mailcraft_time::{Date, DateTime, Timestamp};
use proptest::prelude::*;
use serde_json::{Value, json};

use super::{
    EventsWaiting, Merged, apply_contacts, apply_event_outcomes, apply_events, event_from_remote, event_upload, events_waiting, forget, keeper,
    merge_contacts, merge_events, retry_event_changes, server_start, server_time,
};
use crate::cmd::calendar::current_calendar;
use crate::{CmdError, Session};

/// An instant, by the clock in UTC.
fn at(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> Timestamp {
    clock(year, month, day, hour, minute).to_timestamp(0)
}

/// A wall-clock time.
fn clock(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> DateTime {
    DateTime::new(Date::new(year, month, day).unwrap(), hour, minute, 0).unwrap()
}

/// New York's offset in 2026: four hours behind UTC from March 8th, 07:00 UTC, to November 1st,
/// 06:00 UTC, and five hours behind outside of that.
fn new_york(instant: Timestamp) -> i32 {
    if (at(2026, 3, 8, 7, 0)..at(2026, 11, 1, 6, 0)).contains(&instant) { -240 } else { -300 }
}

/// The sample mailbox on Wednesday 2026-10-14, 16:00 UTC, in a zone four hours behind, with an
/// account that signs in with Microsoft. Its id comes with it.
fn session() -> (Session, Id) {
    let now = at(2026, 10, 14, 16, 0);
    let mut s = Session::at(crate::sample::mailbox(now, -240), now, -240);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// Why a command was refused.
fn refused(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Err(e @ (CmdError::Failed(_) | CmdError::Said(_))) => e.to_string(),
        other => panic!("{id} {params}: expected a refusal, got {other:?}"),
    }
}

/// An event of `minutes` as a server has it.
fn remote(id: &str, title: &str, start: Timestamp, minutes: i64) -> RemoteEvent {
    RemoteEvent { id: id.into(), change_key: format!("ck-{id}-1"), title: title.into(), start, end: start + minutes * 60, ..Default::default() }
}

/// All of October 2026 for `account`, complete.
fn october(account: Id, events: Vec<RemoteEvent>) -> EventsDownload {
    EventsDownload {
        account,
        email: "ada@example.org".into(),
        from: at(2026, 10, 1, 0, 0),
        to: at(2026, 11, 1, 0, 0),
        complete: true,
        events,
        ..Default::default()
    }
}

fn person(id: &str, first: &str, email: &str) -> RemoteContact {
    RemoteContact {
        id: id.into(),
        change_key: format!("ck-{id}-1"),
        first: first.into(),
        emails: vec![Labeled::new("", email)],
        ..Default::default()
    }
}

fn everybody(account: Id, contacts: Vec<RemoteContact>) -> ContactsDownload {
    ContactsDownload { account, email: "ada@example.org".into(), complete: true, contacts }
}

/// The events that came from `account`'s server, by start.
fn from_server(s: &Session, account: Id) -> Vec<Event> {
    let mut events: Vec<Event> = s.mailbox.events.values().filter(|e| e.origin.account == account).map(|e| e.as_ref().clone()).collect();
    events.sort_by_key(|e| (e.start, e.id));
    events
}

fn titles(s: &Session, account: Id) -> Vec<String> {
    from_server(s, account).into_iter().map(|e| e.title).collect()
}

/// The event that the server calls `remote`.
fn event_called(s: &Session, account: Id, remote: &str) -> Event {
    from_server(s, account).into_iter().find(|e| e.origin.id == remote).unwrap_or_else(|| panic!("no event {remote}"))
}

/// Everything in the mailbox that no server keeps.
fn own(s: &Session) -> (Vec<Event>, Vec<Contact>) {
    (
        s.mailbox.events.values().filter(|e| e.origin.is_local()).map(|e| e.as_ref().clone()).collect(),
        s.mailbox.contacts.values().filter(|c| c.origin.is_local()).map(|c| c.as_ref().clone()).collect(),
    )
}

/// The names of the contacts that came from `account`'s server, in order.
fn names(s: &Session, account: Id) -> Vec<String> {
    let mut names: Vec<String> = s.mailbox.contacts.values().filter(|c| c.origin.account == account).map(|c| c.name()).collect();
    names.sort();
    names
}

fn contact_called(s: &Session, account: Id, remote: &str) -> Contact {
    let found = s.mailbox.contacts.values().find(|c| c.origin.account == account && c.origin.id == remote);
    found.map(|c| c.as_ref().clone()).unwrap_or_else(|| panic!("no contact {remote}"))
}

fn counts(merged: &Value) -> (u64, u64, u64, u64, u64) {
    let n = |name: &str| merged[name].as_u64().unwrap();
    (n("added"), n("updated"), n("removed"), n("unchanged"), n("skipped"))
}

#[test]
fn a_download_becomes_events_in_a_calendar_of_the_accounts_own() {
    let (mut s, account) = session();
    let (before, calendars, default) = (own(&s), s.mailbox.calendars.len(), s.mailbox.default_calendar());
    let review = RemoteEvent {
        uid: "040000008200E00074C5B7101A82E008".into(),
        location: "Room 4".into(),
        description: "Agenda:\n1. Numbers".into(),
        organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
        attendees: vec![
            Attendee { address: Address::new("Ada Lovelace", "ADA@example.org"), optional: false, response: Response::None },
            Attendee { address: Address::new("Cy", "cy@example.org"), optional: true, response: Response::Tentative },
        ],
        response: Response::Accepted,
        show_as: ShowAs::Tentative,
        private: true,
        kind: RemoteEventKind::Occurrence,
        series: "series-1".into(),
        reminder_minutes: Some(15),
        modified: at(2026, 10, 1, 8, 0),
        ..remote("e-review", "Budget review", at(2026, 10, 12, 13, 0), 90)
    };
    let holiday = RemoteEvent { all_day: true, cancelled: true, ..remote("e-holiday", "Company holiday", at(2026, 10, 16, 0, 0), 24 * 60) };
    let download = october(account, vec![review, holiday]);
    let (step, rev) = (s.undo_label(), s.rev());
    s.dirty = false;

    let merged = run(&mut s, "sync.applyEvents", json!({"download": download}));
    assert_eq!(counts(&merged), (2, 0, 0, 0, 0));
    assert_eq!(merged["complete"], true);
    // The server's calendar has one of its own here, called what the server calls it: whose it
    // is, the account it is listed under says.
    assert_eq!(s.mailbox.calendars.len(), calendars + 1);
    let calendar = s.mailbox.calendars.last().unwrap().clone();
    assert_eq!(merged["calendar"].as_u64(), Some(calendar.id));
    assert_eq!((calendar.name.as_str(), calendar.visible), ("Calendar", true));
    assert_eq!(calendar.origin, Origin { account, ..Default::default() });
    assert_eq!(keeper(&s.mailbox, &calendar.origin).map(|a| a.email.as_str()), Some("ada@example.org"));
    // The calendar the mailbox had is still its first, and now says where it lives.
    assert_eq!(s.mailbox.default_calendar(), default);
    assert_eq!(s.mailbox.calendar(default.unwrap()).unwrap().name, "On this computer");

    // 13:00 UTC is nine in the morning here; whole days are the days the server names.
    let events = from_server(&s, account);
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0],
        Event {
            id: events[0].id,
            calendar: calendar.id,
            uid: "040000008200E00074C5B7101A82E008".into(),
            title: "Budget review".into(),
            location: "Room 4".into(),
            description: "Agenda:\n1. Numbers".into(),
            start: clock(2026, 10, 12, 9, 0),
            end: clock(2026, 10, 12, 10, 30),
            organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
            // What the owner answered is said of the owner.
            attendees: vec![
                Attendee { address: Address::new("Ada Lovelace", "ADA@example.org"), optional: false, response: Response::Accepted },
                Attendee { address: Address::new("Cy", "cy@example.org"), optional: true, response: Response::Tentative },
            ],
            reminder_minutes: Some(15),
            show_as: ShowAs::Tentative,
            private: true,
            // Bo organizes it: the owner is a guest.
            origin: Origin {
                account,
                id: "e-review".into(),
                change_key: "ck-e-review-1".into(),
                series: "series-1".into(),
                guest: true,
                occurrence: None,
            },
            ..Default::default()
        }
    );
    let holiday = &events[1];
    assert_eq!((holiday.start, holiday.end, holiday.all_day, holiday.cancelled), (clock(2026, 10, 16, 0, 0), clock(2026, 10, 17, 0, 0), true, true));
    assert_eq!(holiday.origin, Origin { account, id: "e-holiday".into(), change_key: "ck-e-holiday-1".into(), ..Default::default() });
    // The calendar shows them like any other event.
    let listed = run(&mut s, "calendar.list", json!({"from": "2026-10-12", "to": "2026-10-16"}));
    let shown: Vec<&str> = listed["events"].as_array().unwrap().iter().filter_map(|e| e["title"].as_str()).collect();
    assert!(shown.contains(&"Budget review") && shown.contains(&"Company holiday"), "{shown:?}");

    // Nothing that was made here was touched, and what arrived is not an undo step.
    assert_eq!(own(&s), before);
    assert!(s.undo_label() == step && s.dirty && s.rev() > rev);
    assert!(s.mailbox.clone().repair().is_empty());

    // A second time changes nothing at all: not the mailbox, and not whether it needs saving.
    let (snapshot, rev) = (s.mailbox.clone(), s.rev());
    s.dirty = false;
    let again = apply_events(&mut s, &download).unwrap();
    assert_eq!(counts(&again), (0, 0, 0, 2, 0));
    assert!(s.mailbox == snapshot && s.rev() == rev && !s.dirty);
    let again = run(&mut s, "sync.applyEvents", json!({"download": download}));
    assert_eq!((counts(&again), s.mailbox.calendars.len()), ((0, 0, 0, 2, 0), calendars + 1));
    assert!(s.mailbox == snapshot && !s.dirty);
}

#[test]
fn what_changed_is_updated_and_what_is_gone_is_removed_within_the_window() {
    let (mut s, account) = session();
    // September and October, the first time.
    let first = EventsDownload {
        from: at(2026, 9, 1, 0, 0),
        events: vec![
            remote("e-standup", "Stand-up", at(2026, 10, 12, 13, 0), 30),
            remote("e-lunch", "Lunch with Bo", at(2026, 10, 20, 16, 0), 60),
            remote("e-kickoff", "Kick-off", at(2026, 9, 28, 14, 0), 60),
            RemoteEvent { all_day: true, ..remote("e-fair", "Trade fair", at(2026, 10, 1, 0, 0), 24 * 60) },
        ],
        ..october(account, Vec::new())
    };
    assert_eq!(counts(&apply_events(&mut s, &first).unwrap()), (4, 0, 0, 0, 0));
    let before = own(&s);
    let standup = event_called(&s, account, "e-standup").id;
    let lunch = event_called(&s, account, "e-lunch").id;
    // A category from when categories were kept here only (a download that doesn't say what
    // categories the events have leaves them as they are); and the one that will go is selected.
    s.mailbox.event_mut(standup).unwrap().categories = vec!["Blue category".into()];
    run(&mut s, "calendar.select", json!({"id": lunch}));
    let step = s.undo_label();

    // October alone, later: one event moved and renamed, one gone, one new.
    let second = october(
        account,
        vec![
            RemoteEvent {
                change_key: "ck-e-standup-2".into(),
                location: "Room 2".into(),
                ..remote("e-standup", "Daily stand-up", at(2026, 10, 12, 13, 30), 15)
            },
            remote("e-retro", "Retrospective", at(2026, 10, 30, 18, 0), 60),
            RemoteEvent { all_day: true, ..remote("e-fair", "Trade fair", at(2026, 10, 1, 0, 0), 24 * 60) },
        ],
    );
    let merged = apply_events(&mut s, &second).unwrap();
    assert_eq!(counts(&merged), (1, 1, 1, 1, 0));
    // The one in September is outside of what was asked about: the download says nothing of it.
    assert_eq!(titles(&s, account), ["Kick-off", "Trade fair", "Daily stand-up", "Retrospective"]);
    // The event that changed is the same event here, with what was kept here.
    let changed = event_called(&s, account, "e-standup");
    assert_eq!((changed.id, changed.title.as_str(), changed.location.as_str()), (standup, "Daily stand-up", "Room 2"));
    assert_eq!((changed.start, changed.end), (clock(2026, 10, 12, 9, 30), clock(2026, 10, 12, 9, 45)));
    assert_eq!((changed.categories, changed.origin.change_key.as_str()), (vec!["Blue category".to_string()], "ck-e-standup-2"));
    // The one that is gone is no longer selected, and nothing else changed.
    assert!(s.mailbox.event(lunch).is_none() && s.sel.event.is_none());
    assert_eq!(own(&s), before);
    assert_eq!(s.undo_label(), step);
    assert_eq!(counts(&apply_events(&mut s, &second).unwrap()), (0, 0, 0, 3, 0));

    // A download that isn't all there is adds and updates, and removes nothing.
    let partial = EventsDownload { complete: false, ..october(account, vec![remote("e-offsite", "Offsite", at(2026, 10, 22, 13, 0), 480)]) };
    let merged = apply_events(&mut s, &partial).unwrap();
    assert_eq!((counts(&merged), merged["complete"].as_bool()), ((1, 0, 0, 0, 0), Some(false)));
    assert_eq!(titles(&s, account).len(), 5);
    // Nor does one for a stretch of no length, or one that runs backwards.
    for (from, to) in [(at(2026, 10, 1, 0, 0), at(2026, 10, 1, 0, 0)), (at(2026, 11, 1, 0, 0), at(2026, 10, 1, 0, 0)), (i64::MAX, i64::MIN)] {
        let merged = apply_events(&mut s, &EventsDownload { from, to, ..october(account, Vec::new()) }).unwrap();
        assert_eq!(counts(&merged), (0, 0, 0, 0, 0));
    }
    // The edges of the stretch: an event is asked about when it starts in it.
    let edges = EventsDownload { from: at(2026, 10, 12, 13, 30), to: at(2026, 10, 22, 13, 0), ..october(account, Vec::new()) };
    assert_eq!(counts(&apply_events(&mut s, &edges).unwrap()), (0, 0, 1, 0, 0));
    assert_eq!(titles(&s, account), ["Kick-off", "Trade fair", "Offsite", "Retrospective"]);
    // A whole-day event is in the stretch by its days as the server names them.
    let day = EventsDownload { from: at(2026, 10, 1, 0, 0), to: at(2026, 10, 1, 0, 1), ..october(account, Vec::new()) };
    assert_eq!(counts(&apply_events(&mut s, &day).unwrap()), (0, 0, 1, 0, 0));
    // An empty calendar, all of it: everything in the stretch goes, and the calendar stays.
    let everything = EventsDownload { from: at(2026, 1, 1, 0, 0), to: at(2027, 1, 1, 0, 0), ..october(account, Vec::new()) };
    assert_eq!(counts(&apply_events(&mut s, &everything).unwrap()), (0, 0, 3, 0, 0));
    assert!(from_server(&s, account).is_empty() && s.mailbox.calendars.iter().any(|c| c.origin.account == account));
    assert_eq!(own(&s), before);
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn only_the_accounts_own_events_from_the_server_are_looked_at() {
    let (mut s, account) = session();
    let other = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"]
        .as_u64()
        .unwrap();
    // An event made here that looks just like one the server has.
    let twin =
        run(&mut s, "calendar.new", json!({"title": "Stand-up", "start": "2026-10-12T09:00", "end": "2026-10-12T09:30"}))["id"].as_u64().unwrap();
    let before = own(&s);

    let mine = october(account, vec![remote("e-1", "Stand-up", at(2026, 10, 12, 13, 0), 30), remote("e-2", "Lunch", at(2026, 10, 13, 16, 0), 60)]);
    assert_eq!(counts(&apply_events(&mut s, &mine).unwrap()), (2, 0, 0, 0, 0));
    // The other account's server calls its events the same: they are other events all the same.
    let theirs =
        EventsDownload { email: "bo@example.org".into(), ..october(other, vec![remote("e-1", "Their stand-up", at(2026, 10, 12, 14, 0), 30)]) };
    assert_eq!(counts(&apply_events(&mut s, &theirs).unwrap()), (1, 0, 0, 0, 0));
    assert_eq!((titles(&s, account), titles(&s, other)), (vec!["Stand-up".to_string(), "Lunch".to_string()], vec!["Their stand-up".to_string()]));
    let calendar = |account: Id| s.mailbox.calendars.iter().find(|c| c.origin.account == account).map(|c| (c.id, c.name.clone())).unwrap();
    assert_ne!(calendar(account).0, calendar(other).0);
    // Each is simply "Calendar", under its own account.
    assert_eq!((calendar(account).1, calendar(other).1), ("Calendar".to_string(), "Calendar".to_string()));
    assert_eq!(event_called(&s, other, "e-1").calendar, calendar(other).0);

    // An empty calendar for the one empties that one only; what was made here is nobody's to remove.
    assert_eq!(counts(&apply_events(&mut s, &october(account, Vec::new())).unwrap()), (0, 0, 2, 0, 0));
    assert_eq!((titles(&s, account).len(), titles(&s, other).len()), (0, 1));
    assert!(s.mailbox.event(twin).is_some());
    assert_eq!(own(&s), before);

    // A download for somebody else, or for nobody, changes nothing.
    let snapshot = s.mailbox.clone();
    let stranger = EventsDownload { email: "eve@example.org".into(), ..october(account, vec![remote("e-9", "Theirs", at(2026, 10, 12, 13, 0), 30)]) };
    assert_eq!(refused(&mut s, "sync.applyEvents", json!({"download": stranger})), "this download was for another account, not for ada@example.org");
    assert!(apply_events(&mut s, &stranger).is_err());
    assert_eq!(refused(&mut s, "sync.applyEvents", json!({"download": october(424_242, Vec::new())})), "no account with id 424242");
    assert!(merge_events(&mut s.mailbox, &october(0, Vec::new()), &|_| 0).is_err());
    let odd = EventsDownload { calendar: "two words".into(), ..october(account, Vec::new()) };
    assert!(matches!(s.run("sync.applyEvents", &json!({"download": odd})), Err(CmdError::Params(_))));
    assert!(s.mailbox == snapshot);
    // Without the address, the id alone says whose it is; letter case and spaces don't matter.
    for email in ["", " ADA@Example.org "] {
        let anonymous = EventsDownload { email: email.into(), ..october(account, vec![remote("e-3", "Tea", at(2026, 10, 15, 19, 0), 30)]) };
        assert!(apply_events(&mut s, &anonymous).is_ok(), "{email:?}");
    }
    assert_eq!(titles(&s, account), ["Tea"]);
    // Another calendar of the same account's server is another calendar here.
    let second = EventsDownload { calendar: "AAMk-team=".into(), ..october(account, vec![remote("e-1", "Team day", at(2026, 10, 19, 13, 0), 60)]) };
    assert_eq!(counts(&apply_events(&mut s, &second).unwrap()), (1, 0, 0, 0, 0));
    let of_account: Vec<(&str, &str)> =
        s.mailbox.calendars.iter().filter(|c| c.origin.account == account).map(|c| (c.name.as_str(), c.origin.id.as_str())).collect();
    assert_eq!(of_account, [("Calendar", ""), ("Calendar 2", "AAMk-team=")]);
    // What the one no longer has says nothing of the other.
    assert_eq!(counts(&apply_events(&mut s, &october(account, Vec::new())).unwrap()), (0, 0, 1, 0, 0));
    assert_eq!(titles(&s, account), ["Team day"]);
    // A server's event that strayed into another calendar here (in a file from elsewhere) is
    // put back where it belongs, not made a second time.
    let team_day = event_called(&s, account, "e-1");
    s.mailbox.event_mut(team_day.id).unwrap().calendar = s.mailbox.default_calendar().unwrap();
    assert_eq!(counts(&apply_events(&mut s, &second).unwrap()), (0, 1, 0, 0, 0));
    assert_eq!(from_server(&s, account), [team_day]);
}

#[test]
fn what_is_not_an_event_of_its_own_is_passed_over() {
    let (mut s, account) = session();
    let series = RemoteEvent { kind: RemoteEventKind::SeriesMaster, ..remote("e-series", "Stand-up (the series)", at(2026, 10, 5, 13, 0), 30) };
    let occurrence = |day: u8| RemoteEvent {
        kind: RemoteEventKind::Occurrence,
        series: "e-series".into(),
        ..remote(&format!("e-series-{day}"), "Stand-up", at(2026, 10, day, 13, 0), 30)
    };
    let download = october(
        account,
        vec![
            series,
            occurrence(5),
            occurrence(6),
            RemoteEvent { kind: RemoteEventKind::Exception, title: "Stand-up, later".into(), ..occurrence(7) },
            // Said twice, with no id, with an id that isn't one.
            RemoteEvent { title: "Stand-up again".into(), ..occurrence(5) },
            remote("", "Nameless", at(2026, 10, 8, 13, 0), 30),
            remote("two words", "Odd", at(2026, 10, 8, 13, 0), 30),
            remote(&"x".repeat(5_000), "Long", at(2026, 10, 8, 13, 0), 30),
        ],
    );
    assert_eq!(counts(&apply_events(&mut s, &download).unwrap()), (3, 0, 0, 0, 5));
    assert_eq!(titles(&s, account), ["Stand-up", "Stand-up", "Stand-up, later"]);
    // Every occurrence knows its series, and none of them repeats by itself.
    assert!(from_server(&s, account).iter().all(|e| e.origin.series == "e-series" && e.recurrence.is_none()));
    assert_eq!(counts(&apply_events(&mut s, &download).unwrap()), (0, 0, 0, 3, 5));

    // What comes from outside is cut to size on the way in.
    let wordy = RemoteEvent {
        title: format!("Budget\r\nreview{}", "x".repeat(5_000)),
        description: "é".repeat(300_000),
        organizer: Some(Address { name: "Bo\r\nBcc: eve@example.org".into(), email: "not an address".into() }),
        attendees: (0..2_000).map(|n| Attendee { address: Address::new("", format!("p{n}@example.org")), ..Default::default() }).collect(),
        reminder_minutes: Some(u32::MAX),
        start: i64::MAX,
        end: i64::MIN,
        ..remote("e-wordy", "", 0, 0)
    };
    assert_eq!(counts(&apply_events(&mut s, &EventsDownload { complete: false, ..october(account, vec![wordy]) }).unwrap()), (1, 0, 0, 0, 0));
    let wordy = event_called(&s, account, "e-wordy");
    assert_eq!(
        (wordy.title.chars().count(), wordy.description.chars().count(), wordy.attendees.len(), wordy.reminder_minutes),
        (500, 100_000, 500, Some(40_320))
    );
    assert!(wordy.title.starts_with("Budget  review") && wordy.end >= wordy.start && wordy.start.date.year == 9999);
    assert_eq!(wordy.organizer, Some(Address { name: "Bo  Bcc: eve@example.org".into(), email: String::new() }));
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn times_become_the_wall_clock_times_of_their_own_moment() {
    let local = |start: Timestamp, end: Timestamp| {
        let event = event_from_remote(&RemoteEvent { start, end, ..remote("e", "x", 0, 0) }, "", &new_york);
        (event.start, event.end)
    };
    // A weekly meeting at nine in New York is at nine before and after the clocks go back, though
    // the server has it an hour later by UTC.
    assert_eq!(local(at(2026, 10, 26, 13, 0), at(2026, 10, 26, 13, 30)), (clock(2026, 10, 26, 9, 0), clock(2026, 10, 26, 9, 30)));
    assert_eq!(local(at(2026, 11, 2, 14, 0), at(2026, 11, 2, 14, 30)), (clock(2026, 11, 2, 9, 0), clock(2026, 11, 2, 9, 30)));
    // And in spring, when they go forward.
    assert_eq!(local(at(2026, 3, 2, 14, 0), at(2026, 3, 2, 14, 30)), (clock(2026, 3, 2, 9, 0), clock(2026, 3, 2, 9, 30)));
    assert_eq!(local(at(2026, 3, 9, 13, 0), at(2026, 3, 9, 13, 30)), (clock(2026, 3, 9, 9, 0), clock(2026, 3, 9, 9, 30)));
    // An event across the change has each end at the time its own moment shows: five hours
    // from eleven at night to three in the morning.
    assert_eq!(local(at(2026, 11, 1, 3, 0), at(2026, 11, 1, 8, 0)), (clock(2026, 10, 31, 23, 0), clock(2026, 11, 1, 3, 0)));
    // The minute before the clocks change, and the minute they do.
    assert_eq!(local(at(2026, 11, 1, 5, 59), at(2026, 11, 1, 6, 0)), (clock(2026, 11, 1, 1, 59), clock(2026, 11, 1, 1, 59)));
    assert_eq!(local(at(2026, 3, 8, 6, 59), at(2026, 3, 8, 7, 0)), (clock(2026, 3, 8, 1, 59), clock(2026, 3, 8, 3, 0)));
    // Another day here than in UTC.
    assert_eq!(local(at(2026, 10, 13, 2, 0), at(2026, 10, 13, 3, 0)), (clock(2026, 10, 12, 22, 0), clock(2026, 10, 12, 23, 0)));
    // An end before the start is no length, not a negative one.
    assert_eq!(local(at(2026, 10, 12, 13, 0), at(2026, 10, 12, 12, 0)), (clock(2026, 10, 12, 9, 0), clock(2026, 10, 12, 9, 0)));

    // Whole days are the days the server names, whatever the clocks do in them.
    let days = |start: Timestamp, end: Timestamp, offset_at: &dyn Fn(Timestamp) -> i32| {
        let event = event_from_remote(&RemoteEvent { start, end, all_day: true, ..remote("e", "x", 0, 0) }, "", offset_at);
        assert!(event.all_day);
        (event.start, event.end)
    };
    let november_first = (clock(2026, 11, 1, 0, 0), clock(2026, 11, 2, 0, 0));
    let (utc, far_east, far_west) = (|_: Timestamp| 0, |_: Timestamp| 840, |_: Timestamp| -720);
    for offset_at in [&new_york as &dyn Fn(Timestamp) -> i32, &utc, &far_east, &far_west] {
        assert_eq!(days(at(2026, 11, 1, 0, 0), at(2026, 11, 2, 0, 0), offset_at), november_first);
        // Three days over the change.
        assert_eq!(days(at(2026, 10, 31, 0, 0), at(2026, 11, 3, 0, 0), offset_at), (clock(2026, 10, 31, 0, 0), clock(2026, 11, 3, 0, 0)));
        // A day has an end after its start, whatever the server says of it.
        assert_eq!(days(at(2026, 11, 1, 0, 0), at(2026, 11, 1, 0, 0), offset_at), november_first);
        assert_eq!(days(at(2026, 11, 1, 0, 0), at(2026, 10, 1, 0, 0), offset_at), november_first);
    }
    // A server that names the midnights of the zone instead of the days: the same days. The
    // day the clocks go back is twenty-five hours long there.
    assert_eq!(days(at(2026, 11, 1, 4, 0), at(2026, 11, 2, 5, 0), &new_york), november_first);
    assert_eq!(days(at(2026, 10, 31, 22, 0), at(2026, 11, 1, 22, 0), &|_| 120), november_first);
    // Nothing overflows at the ends of time.
    for instant in [i64::MIN, -1, 0, i64::MAX] {
        for offset in [i32::MIN, 0, i32::MAX] {
            let _ = days(instant, instant, &|_| offset);
            let _ = event_from_remote(&RemoteEvent { start: instant, end: instant, ..Default::default() }, "", &|_| offset);
        }
    }
}

#[test]
fn a_merge_over_a_daylight_saving_change_keeps_the_hour_and_the_window() {
    let (mut s, account) = session();
    // The weekly meeting at nine, New York time, as the server expands it over the change.
    let weekly: Vec<RemoteEvent> = [(10, 19, 13), (10, 26, 13), (11, 2, 14), (11, 9, 14)]
        .into_iter()
        .map(|(month, day, hour)| RemoteEvent {
            kind: RemoteEventKind::Occurrence,
            series: "e-weekly".into(),
            ..remote(&format!("e-weekly-{month}-{day}"), "Weekly", at(2026, month, day, hour, 0), 30)
        })
        .collect();
    let autumn = EventsDownload { from: at(2026, 10, 15, 0, 0), to: at(2026, 11, 15, 0, 0), events: weekly.clone(), ..october(account, Vec::new()) };
    let merged = merge_events(&mut s.mailbox, &autumn, &new_york).unwrap();
    assert_eq!((merged.added, merged.calendar != 0, merged.new_calendar, merged.changed()), (4, true, true, true));
    let starts: Vec<String> = from_server(&s, account).iter().map(|e| e.start.iso()).collect();
    assert_eq!(starts, ["2026-10-19T09:00:00", "2026-10-26T09:00:00", "2026-11-02T09:00:00", "2026-11-09T09:00:00"]);
    assert!(from_server(&s, account).iter().all(|e| e.duration_minutes() == 30));
    // Again: nothing to do.
    let again = merge_events(&mut s.mailbox, &autumn, &new_york).unwrap();
    assert_eq!(again, Merged { unchanged: 4, calendar: merged.calendar, ..Default::default() });
    assert!(!again.changed());

    // The window is counted in the server's instants on both sides of the change: the meeting
    // of November 2nd starts at 14:00 UTC, and is asked about by a window that begins then, not
    // by one that begins a minute later.
    let late =
        EventsDownload { from: at(2026, 11, 2, 14, 1), to: at(2026, 11, 15, 0, 0), events: vec![weekly[3].clone()], ..october(account, Vec::new()) };
    assert_eq!(merge_events(&mut s.mailbox, &late, &new_york).unwrap().removed, 0);
    let exact = EventsDownload { from: at(2026, 11, 2, 14, 0), ..late.clone() };
    assert_eq!(merge_events(&mut s.mailbox, &exact, &new_york).unwrap().removed, 1);
    // And the one of October 26th at 13:00 UTC, by a window that ends after it.
    let early =
        EventsDownload { from: at(2026, 10, 15, 0, 0), to: at(2026, 10, 26, 13, 0), events: vec![weekly[0].clone()], ..october(account, Vec::new()) };
    assert_eq!(merge_events(&mut s.mailbox, &early, &new_york).unwrap().removed, 0);
    let exact = EventsDownload { to: at(2026, 10, 26, 13, 1), ..early };
    assert_eq!(merge_events(&mut s.mailbox, &exact, &new_york).unwrap().removed, 1);
    assert_eq!(titles(&s, account).len(), 2);

    // Read again somewhere else, the same events show at that place's hours: updated, not added.
    let rest = EventsDownload { events: vec![weekly[0].clone(), weekly[3].clone()], ..autumn };
    let moved = merge_events(&mut s.mailbox, &rest, &|_| 60).unwrap();
    assert_eq!((moved.added, moved.updated, moved.removed, moved.unchanged), (0, 2, 0, 0));
    let starts: Vec<String> = from_server(&s, account).iter().map(|e| e.start.iso()).collect();
    assert_eq!(starts, ["2026-10-19T14:00:00", "2026-11-09T15:00:00"]);
}

#[test]
fn what_a_server_cannot_be_told_of_an_event_is_refused() {
    let (mut s, account) = session();
    let meeting = RemoteEvent {
        attendees: vec![Attendee { address: Address::new("Ada", "ada@example.org"), ..Default::default() }],
        organizer: Some(Address::new("Bo", "bo@example.org")),
        ..remote("e-1", "Budget review", at(2026, 10, 12, 13, 0), 60)
    };
    let download = october(account, vec![meeting]);
    apply_events(&mut s, &download).unwrap();
    let event = event_called(&s, account, "e-1");
    let (id, calendar) = (event.id, event.calendar);
    assert!(event.origin.guest, "Bo organizes it");
    let snapshot = s.mailbox.clone();

    // What is the organizer's to say is not changed in the owner's copy, and it stays in its
    // calendar; each refusal says why, and nothing changes.
    let theirs = "This meeting is kept by the server of ada@example.org, and somebody else organizes it.";
    for (command, params, why) in [
        ("calendar.edit", json!({"id": id, "attendees": "cy@example.org"}), format!("{theirs} Who is invited is for its organizer to say.")),
        ("calendar.edit", json!({"id": id, "recurrence": {"freq": "weekly"}}), format!("{theirs} How it repeats is for its organizer to say.")),
        (
            "calendar.edit",
            json!({"id": id, "calendar": s.mailbox.default_calendar()}),
            "This event is kept by the server of ada@example.org. It can be moved to another calendar of that account, not out of it: taking it out would delete it there."
                .to_string(),
        ),
    ] {
        assert_eq!(refused(&mut s, command, params.clone()), why, "{command} {params}");
        assert!(s.mailbox == snapshot, "{command} {params}");
    }
    assert_eq!(refused(&mut s, "calendar.delete", json!({"id": id, "occurrence": "2026-10-12"})), "that event doesn't repeat");
    // The calendar itself is the server's to keep.
    let why_calendar =
        "This calendar is kept by the server of ada@example.org. Changes made here are not sent back yet, so it can't be changed or deleted here.";
    assert_eq!(refused(&mut s, "calendar.removeCalendar", json!({"id": calendar})), why_calendar);
    assert!(s.mailbox == snapshot && s.mailbox.event_changes.is_empty());

    // Whether the calendar shows is kept here only, and needs no telling. A category goes to the
    // server (it is the owner's own business: nobody is written to).
    assert_eq!(run(&mut s, "calendar.showCalendar", json!({"id": calendar, "value": false}))["visible"], false);
    assert!(s.mailbox.event_changes.is_empty());
    assert_eq!(run(&mut s, "calendar.categorize", json!({"id": id, "category": "Green category"}))["categories"], json!(["Green category"]));
    let told = ops(&s, account);
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].fields, EventFields { categories: Some(vec!["Green category".into()]), ..Default::default() });
    // A download leaves both as they are here.
    assert_eq!(counts(&apply_events(&mut s, &download).unwrap()), (0, 0, 0, 1, 0));
    assert_eq!(event_called(&s, account, "e-1").categories, ["Green category"]);
    assert!(!s.mailbox.calendar(calendar).unwrap().visible);

    // An event whose account is gone (`account.remove` takes it with the account, #325; a file an
    // earlier version wrote may still have one) is the mailbox's own: nothing will ever update it
    // again.
    s.mailbox.remove_account(account).unwrap();
    assert!(keeper(&s.mailbox, &s.mailbox.event(id).unwrap().origin).is_none());
    run(&mut s, "calendar.edit", json!({"id": id, "title": "Mine now", "attendees": ""}));
    run(&mut s, "calendar.delete", json!({"id": id}));
    assert!(s.mailbox.event(id).is_none() && s.mailbox.event_changes.is_empty());
}

/// A session whose account's server has one event, "e-1" (an hour from nine in the morning
/// here on Monday the 12th), and one of the owner's own, "e-2", the day after. With the
/// account, the events' ids here and the server calendar's.
fn served() -> (Session, Id, Id, Id, Id) {
    let (mut s, account) = session();
    let download =
        october(account, vec![remote("e-1", "Budget review", at(2026, 10, 12, 13, 0), 60), remote("e-2", "Dentist", at(2026, 10, 13, 18, 0), 30)]);
    apply_events(&mut s, &download).unwrap();
    let (first, second) = (event_called(&s, account, "e-1"), event_called(&s, account, "e-2"));
    (s, account, first.id, second.id, first.calendar)
}

/// What the server is to be told now.
fn ops(s: &Session, account: Id) -> Vec<EventOp> {
    event_upload(s, account).unwrap().ops
}

/// The answer of a server that did everything it was asked: a new event is `made-n`, and every
/// change key is `ck-new`.
fn all_done(upload: &EventsUpload) -> EventsUploaded {
    let outcome = |(n, op): (usize, &EventOp)| EventOutcome {
        op: op.clone(),
        result: EventResult::Done,
        remote: if op.kind == EventChangeKind::Create { format!("made-{n}") } else { op.remote.clone() },
        change_key: "ck-new".into(),
        ..Default::default()
    };
    EventsUploaded {
        account: upload.account,
        email: upload.email.clone(),
        outcomes: upload.ops.iter().enumerate().map(outcome).collect(),
        ..Default::default()
    }
}

/// Send what waits to a server that does it all, and take its answer in.
fn push_done(s: &mut Session, account: Id) -> super::EventsApplied {
    let upload = event_upload(s, account).unwrap();
    apply_event_outcomes(s, &all_done(&upload)).unwrap()
}

/// Send what waits to a server that says `result` to everything, and take its answer in.
fn push_said(s: &mut Session, account: Id, result: EventResult, error: &str) -> super::EventsApplied {
    let upload = event_upload(s, account).unwrap();
    apply_event_outcomes(s, &all_said(&upload, result, error)).unwrap()
}

/// The answer of a server that said `result` to everything, with `error`.
fn all_said(upload: &EventsUpload, result: EventResult, error: &str) -> EventsUploaded {
    let outcome = |op: &EventOp| EventOutcome { op: op.clone(), result, error: error.into(), ..Default::default() };
    EventsUploaded { account: upload.account, email: upload.email.clone(), outcomes: upload.ops.iter().map(outcome).collect(), ..Default::default() }
}

#[test]
fn new_events_go_to_the_calendar_being_worked_in() {
    // No server: the mailbox's own calendar, as ever.
    let (mut s, account) = session();
    let local = s.mailbox.default_calendar().unwrap();
    assert_eq!(current_calendar(&s), local);
    let made = run(&mut s, "calendar.new", json!({"title": "Before", "start": "2026-10-12T15:00"}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.event(made).map(|e| (e.calendar, e.origin.is_local())), Some((local, true)));
    assert!(s.mailbox.event_changes.is_empty());

    // With a server calendar, that is where new events go: they are to reach the server.
    apply_events(&mut s, &october(account, Vec::new())).unwrap();
    let server = s.mailbox.calendars.iter().find(|c| c.origin.account == account).map(|c| c.id).unwrap();
    assert_eq!(current_calendar(&s), server);
    let made = run(&mut s, "calendar.new", json!({"title": "After", "start": "2026-10-12T16:00"}))["id"].as_u64().unwrap();
    let event = s.mailbox.event(made).unwrap().clone();
    assert_eq!((event.calendar, event.origin.account, event.origin.id.as_str()), (server, account, ""));
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.event)).collect::<Vec<_>>(), [(EventChangeKind::Create, made)]);

    // The owner picks the one on this computer to work in: new events stay here.
    assert_eq!(run(&mut s, "calendar.setCurrent", json!({"id": local}))["id"], local);
    let here = run(&mut s, "calendar.new", json!({"title": "Here", "start": "2026-10-12T17:00"}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.event(here).map(|e| (e.calendar, e.origin.is_local())), Some((local, true)));
    assert_eq!(s.mailbox.event_changes.len(), 1);
    // A calendar that is named wins over the one being worked in.
    let named = run(&mut s, "calendar.new", json!({"title": "Named", "start": "2026-10-12T18:00", "calendar": server}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.event(named).map(|e| e.calendar), Some(server));
    assert_eq!(s.mailbox.event_changes.len(), 2);
    // Back to the default; and a calendar that isn't one is no choice.
    assert_eq!(run(&mut s, "calendar.setCurrent", json!({"id": null}))["id"], server);
    assert_eq!(refused(&mut s, "calendar.setCurrent", json!({"id": 424_242})), "no calendar with id 424242");
    assert_eq!(current_calendar(&s), server);

    // A meeting in the server's calendar is one the server sends invitations for: the command
    // has to be told which is meant, and makes nothing when it isn't.
    let sync = json!({"title": "Sync", "start": "2026-10-12T19:00", "attendees": "bo@example.org; cy@example.org"});
    let events = s.mailbox.events.len();
    let why = refused(&mut s, "calendar.newMeeting", sync.clone());
    assert_eq!(
        why,
        "2 people are invited to this meeting, and putting it into a calendar that an account's server keeps makes that server send them the invitation by mail. Nothing was made. Say \"send\": true to invite them, or \"send\": false to keep the meeting on this computer and invite nobody."
    );
    assert_eq!((s.mailbox.events.len(), s.mailbox.event_changes.len()), (events, 2));
    // Not to be sent: kept on this computer, and the answer says so.
    let with = |send: bool| {
        let mut params = sync.clone();
        params["send"] = json!(send);
        params
    };
    let meeting = run(&mut s, "calendar.newMeeting", with(false));
    assert_eq!(meeting["keptHere"], true);
    assert_eq!(s.mailbox.event(meeting["id"].as_u64().unwrap()).map(|e| (e.calendar, e.origin.is_local())), Some((local, true)));
    assert_eq!(s.mailbox.event_changes.len(), 2);
    // To be sent: made on the server, with who is invited, and the owner's yes noted with it.
    let meeting = run(&mut s, "calendar.newMeeting", with(true));
    assert_eq!(meeting.get("keptHere"), None);
    let invited = s.mailbox.event(meeting["id"].as_u64().unwrap()).unwrap().clone();
    assert_eq!(
        (invited.calendar, invited.origin.account, invited.organizer.as_ref().map(|o| o.email.as_str())),
        (server, account, Some("ada@example.org"))
    );
    let noted = s.mailbox.event_changes.last().unwrap().clone();
    assert_eq!((noted.kind, noted.event, noted.send), (EventChangeKind::Create, invited.id, true));
    let told = ops(&s, account).pop().unwrap();
    let people: Vec<_> = told.fields.attendees.unwrap().into_iter().map(|a| (a.address.email, a.optional)).collect();
    assert_eq!(people, [("bo@example.org".to_string(), false), ("cy@example.org".to_string(), false)]);
    // A meeting in a calendar on this computer invites nobody, and needs nothing said.
    let here = json!({"title": "Dry run", "start": "2026-10-12T19:00", "attendees": "bo@example.org", "calendar": local});
    assert_eq!(run(&mut s, "calendar.newMeeting", here).get("keptHere"), None);
    // An event that repeats goes to the server as a series: its rule, and its times by the clock.
    let series = run(
        &mut s,
        "calendar.new",
        json!({"title": "Gym", "start": "2026-10-12T20:00", "recurrence": {"freq": "weekly", "count": 8}, "calendar": server}),
    );
    assert_eq!((series.get("keptHere"), s.mailbox.event_changes.len()), (None, 4));
    let told = ops(&s, account).pop().unwrap();
    assert_eq!(told.fields.wall, Some(EventWall { start: clock(2026, 10, 12, 20, 0), end: clock(2026, 10, 12, 20, 30) }));
    let rule = Recurrence { count: Some(8), ..Recurrence::every(Freq::Weekly) };
    assert_eq!(told.fields.repeat, Some(EventRepeat { rule: Some(rule), first: Date::new(2026, 10, 12).unwrap() }));
    assert_eq!(told.fields.time.map(|t| t.start), Some(at(2026, 10, 13, 0, 0)));
}

#[test]
fn the_mailboxs_own_calendar_goes_or_says_where_it_lives() {
    // A mailbox whose own calendar is empty: beside the server's it is gone, and there is one
    // "Calendar" where there would have been two.
    let now = at(2026, 10, 14, 16, 0);
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("Ada", "ada@example.org"), now, -240);
    let account = s.mailbox.accounts[0].id;
    s.mailbox.account_mut(account).unwrap().kind = mailcraft_model::AccountKind::Imap;
    assert_eq!(s.mailbox.calendars.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["Calendar"]);
    let download =
        EventsDownload { email: "ada@example.org".into(), ..october(account, vec![remote("e-1", "Stand-up", at(2026, 10, 12, 13, 0), 30)]) };
    let merged = apply_events(&mut s, &download).unwrap();
    assert_eq!(counts(&merged), (1, 0, 0, 0, 0));
    let calendars: Vec<(String, Id)> = s.mailbox.calendars.iter().map(|c| (c.name.clone(), c.origin.account)).collect();
    assert_eq!(calendars, [("Calendar".to_string(), account)]);
    // An event still has somewhere to go: the server's calendar; and a meeting that is not to
    // invite anybody yet gets a calendar on this computer made for it.
    let plain = run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-12T12:00"}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.event(plain).map(|e| e.calendar), Some(s.mailbox.calendars[0].id));
    let draft =
        run(&mut s, "calendar.newMeeting", json!({"title": "Gym", "start": "2026-10-12T18:00", "attendees": "bo@example.org", "send": false}));
    assert_eq!(draft["keptHere"], true);
    let names: Vec<&str> = s.mailbox.calendars.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Calendar", "On this computer"]);
    // It holds an event now: the next download leaves it, name and all.
    apply_events(&mut s, &download).unwrap();
    assert_eq!(s.mailbox.calendars.len(), 2);
    assert!(s.mailbox.clone().repair().is_empty());

    // A mailbox whose own calendar holds events (the sample's): it stays, and says where it lives.
    let (mut s, account) = session();
    let own_events = own(&s).0.len();
    apply_events(&mut s, &october(account, Vec::new())).unwrap();
    let names: Vec<&str> = s.mailbox.calendars.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["On this computer", "Calendar"]);
    assert_eq!(own(&s).0.len(), own_events);
    // A calendar the owner made and named is the owner's: empty or not, it is left alone.
    let family = run(&mut s, "calendar.addCalendar", json!({"name": "Family"}))["id"].as_u64().unwrap();
    apply_events(&mut s, &october(account, Vec::new())).unwrap();
    assert!(s.mailbox.calendar(family).is_some());
    // One that was named after its account, as it once was, is called what the server calls it.
    s.mailbox.calendars.iter_mut().find(|c| c.origin.account == account).unwrap().name = "Calendar (ada@example.org)".into();
    let merged = merge_events(&mut s.mailbox, &october(account, Vec::new()), &|_| -240).unwrap();
    assert!(merged.settled && merged.changed());
    assert_eq!(s.mailbox.calendars.iter().find(|c| c.origin.account == account).unwrap().name, "Calendar");
    assert!(!merge_events(&mut s.mailbox, &october(account, Vec::new()), &|_| -240).unwrap().changed());
}

#[test]
fn an_event_of_the_mailboxs_own_can_be_put_on_the_server() {
    let (mut s, account, _, _, server) = served();
    let local = s.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
    // Made on this computer, as the owner's first tries were.
    let stranded = run(&mut s, "calendar.new", json!({"title": "Stranded", "start": "2026-10-15T10:00", "calendar": local}))["id"].as_u64().unwrap();
    assert!(s.mailbox.event_changes.is_empty());
    // Moved to the server's calendar: the same event, the server's from now on, to be made there.
    run(&mut s, "calendar.edit", json!({"id": stranded, "calendar": server}));
    let event = s.mailbox.event(stranded).unwrap().clone();
    assert_eq!((event.calendar, event.origin.account, event.origin.id.as_str()), (server, account, ""));
    let upload = event_upload(&s, account).unwrap();
    assert_eq!(upload.ops.len(), 1);
    assert_eq!(
        (upload.ops[0].kind, upload.ops[0].event, upload.ops[0].fields.title.as_deref()),
        (EventChangeKind::Create, stranded, Some("Stranded"))
    );
    apply_event_outcomes(&mut s, &all_done(&upload)).unwrap();
    assert_eq!(s.mailbox.event(stranded).map(|e| e.origin.id.as_str()), Some("made-0"));
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Stranded").count(), 1, "one event, not a copy and an original");
    // The other way would delete it on the server: it isn't done from here.
    let why = refused(&mut s, "calendar.edit", json!({"id": stranded, "calendar": local}));
    assert_eq!(
        why,
        "This event is kept by the server of ada@example.org. It can be moved to another calendar of that account, not out of it: taking it out would delete it there."
    );
    // A series goes too, and the days it skips here are deleted there once it is there.
    let series =
        run(&mut s, "calendar.new", json!({"title": "Gym", "start": "2026-10-15T18:00", "recurrence": {"freq": "weekly"}, "calendar": local}))["id"]
            .clone();
    run(&mut s, "calendar.delete", json!({"id": series, "occurrence": "2026-10-22"}));
    run(&mut s, "calendar.edit", json!({"id": series, "calendar": server}));
    let upload = event_upload(&s, account).unwrap();
    assert_eq!(upload.ops.iter().map(|op| (op.kind, op.fields.repeat.is_some())).collect::<Vec<_>>(), [(EventChangeKind::Create, true)]);
    apply_event_outcomes(&mut s, &all_done(&upload)).unwrap();
    let skips = ops(&s, account);
    assert_eq!(
        skips.iter().map(|op| (op.kind, op.remote.as_str(), op.series.as_str(), op.occurrence)).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, "", "made-0", at(2026, 10, 22, 22, 0))]
    );
    // A meeting of the mailbox's own invites people once it is on a server: only when told to.
    let meeting = json!({"title": "Dry run", "start": "2026-10-16T10:00", "attendees": "bo@example.org", "calendar": local});
    let meeting = run(&mut s, "calendar.newMeeting", meeting)["id"].clone();
    let why = refused(&mut s, "calendar.edit", json!({"id": meeting, "calendar": server}));
    assert_eq!(
        why,
        "1 person is invited to this meeting, and putting it into a calendar that an account's server keeps makes that server send them the invitation by mail. Nothing was changed. Say that this is meant with \"send\": true."
    );
    assert!(s.mailbox.event(meeting.as_u64().unwrap()).unwrap().origin.is_local());
    run(&mut s, "calendar.edit", json!({"id": meeting, "calendar": server, "send": true}));
    assert!(s.mailbox.event_changes.iter().any(|change| change.kind == EventChangeKind::Create && change.send));
    // But not a meeting that repeats and skips days: each would be a cancellation in the mail.
    let odd = json!({"title": "Odd", "start": "2026-10-16T11:00", "attendees": "bo@example.org", "recurrence": {"freq": "daily"}, "calendar": local});
    let odd = run(&mut s, "calendar.newMeeting", odd)["id"].clone();
    run(&mut s, "calendar.delete", json!({"id": odd, "occurrence": "2026-10-17"}));
    let why = refused(&mut s, "calendar.edit", json!({"id": odd, "calendar": server, "send": true}));
    assert!(why.starts_with("A meeting that repeats and skips some days can't be put on the server's calendar from here"), "{why}");
}

#[test]
fn what_is_done_to_a_servers_event_is_noted_for_the_server() {
    let (mut s, account, first, second, _) = served();
    assert!(ops(&s, account).is_empty() && events_waiting(&s, account) == EventsWaiting::default());

    // Moved: only the times go, as instants. Nine in the morning on the 14th here is 13:00 UTC.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    let moved = ops(&s, account);
    assert_eq!(moved.len(), 1);
    assert_eq!((moved[0].kind, moved[0].event, moved[0].remote.as_str(), moved[0].token.as_str()), (EventChangeKind::Update, first, "e-1", ""));
    let time = EventTime { start: at(2026, 10, 14, 13, 0), end: at(2026, 10, 14, 14, 0), all_day: false };
    assert_eq!(moved[0].fields, EventFields { time: Some(time), ..Default::default() });
    // Changed again before the server heard: still one change, of everything that differs.
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Budget review (moved)", "location": "Room 4"}));
    run(&mut s, "calendar.showAs", json!({"id": first, "value": "tentative"}));
    run(&mut s, "calendar.reminder", json!({"id": first, "minutes": 5}));
    run(&mut s, "calendar.private", json!({"id": first}));
    let changed = ops(&s, account);
    assert_eq!(changed.len(), 1);
    assert_eq!(
        changed[0].fields,
        EventFields {
            title: Some("Budget review (moved)".into()),
            location: Some("Room 4".into()),
            description: None,
            time: Some(time),
            show_as: Some(ShowAs::Tentative),
            private: Some(true),
            reminder: Some(EventReminder { on: true, minutes: 5 }),
            ..Default::default()
        }
    );
    assert_eq!(events_waiting(&s, account), EventsWaiting { due: 1, total: 1, next: None });
    // Its categories go to the server with it, and taking one back leaves nothing more to tell.
    run(&mut s, "calendar.categorize", json!({"id": first, "category": "Green category"}));
    assert_eq!(ops(&s, account)[0].fields.categories, Some(vec!["Green category".to_string()]));
    run(&mut s, "calendar.categorize", json!({"id": first, "category": "Green category"}));
    assert_eq!(ops(&s, account), changed);

    // Deleted: the server is to delete it; the change that waited is overtaken.
    run(&mut s, "calendar.delete", json!({"id": first}));
    let deleted = ops(&s, account);
    assert_eq!(
        deleted.iter().map(|op| (op.kind, op.remote.as_str(), op.fields.is_empty())).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, "e-1", true)]
    );
    // What is put back, should the server refuse, is the event as the server has it.
    assert_eq!(
        s.mailbox.event_changes[0].base.as_ref().map(|base| (base.title.as_str(), base.start)),
        Some(("Budget review", clock(2026, 10, 12, 9, 0)))
    );

    // A whole day: the days as the server names them. And a new event goes whole, with a mark.
    run(&mut s, "calendar.edit", json!({"id": second, "allDay": true}));
    let made =
        run(&mut s, "calendar.new", json!({"title": "Offsite", "start": "2026-10-20", "end": "2026-10-21", "allDay": true}))["id"].as_u64().unwrap();
    let all = ops(&s, account);
    assert_eq!(all.iter().map(|op| op.kind).collect::<Vec<_>>(), [EventChangeKind::Delete, EventChangeKind::Update, EventChangeKind::Create]);
    assert_eq!(
        all[1].fields,
        EventFields { time: Some(EventTime { start: at(2026, 10, 13, 0, 0), end: at(2026, 10, 14, 0, 0), all_day: true }), ..Default::default() }
    );
    let new = &all[2];
    assert_eq!((new.event, new.remote.as_str(), new.token.len()), (made, "", 36));
    assert_eq!(
        new.fields,
        EventFields {
            title: Some("Offsite".into()),
            location: Some(String::new()),
            description: Some(String::new()),
            time: Some(EventTime { start: at(2026, 10, 20, 0, 0), end: at(2026, 10, 22, 0, 0), all_day: true }),
            // As Outlook has a new whole-day event: free, and a reminder the afternoon before.
            show_as: Some(ShowAs::Free),
            private: Some(false),
            reminder: Some(EventReminder { on: true, minutes: 18 * 60 }),
            ..Default::default()
        }
    );
    // The same through the command, as plain data.
    let listed = run(&mut s, "sync.eventChanges", json!({"account": account}));
    assert_eq!(
        (listed["account"].as_u64(), listed["email"].as_str(), listed["ops"].as_array().map(Vec::len)),
        (Some(account), Some("ada@example.org"), Some(3))
    );
    assert_eq!(serde_json::from_value::<EventsUpload>(listed).unwrap().ops, all);
    // A new event that is deleted again never reaches the server.
    run(&mut s, "calendar.delete", json!({"id": made}));
    assert_eq!(ops(&s, account).len(), 2);
    assert!(matches!(s.run("sync.eventChanges", &json!({"account": 424_242})), Err(CmdError::Failed(_) | CmdError::Said(_))));
}

#[test]
fn times_go_to_the_server_as_the_instants_of_their_own_moment() {
    // New York: nine in the morning is 13:00 UTC in October and 14:00 UTC in November.
    let event = |start: DateTime, end: DateTime, all_day: bool| Event { start, end, all_day, ..Default::default() };
    let time = |e: &Event| server_time(e, &new_york);
    let october = event(clock(2026, 10, 26, 9, 0), clock(2026, 10, 26, 10, 0), false);
    assert_eq!(time(&october), EventTime { start: at(2026, 10, 26, 13, 0), end: at(2026, 10, 26, 14, 0), all_day: false });
    let november = event(clock(2026, 11, 2, 9, 0), clock(2026, 11, 2, 10, 0), false);
    assert_eq!(time(&november), EventTime { start: at(2026, 11, 2, 14, 0), end: at(2026, 11, 2, 15, 0), all_day: false });
    // One that runs over the change: each end with the offset of its own moment. The clocks go
    // back at two on November 1st, so half past midnight to three is three and a half hours.
    let night = event(clock(2026, 11, 1, 0, 30), clock(2026, 11, 1, 3, 0), false);
    assert_eq!(time(&night), EventTime { start: at(2026, 11, 1, 4, 30), end: at(2026, 11, 1, 8, 0), all_day: false });
    // And in spring, when they go forward: an evening before, a morning after.
    let spring = event(clock(2026, 3, 7, 22, 0), clock(2026, 3, 8, 9, 0), false);
    assert_eq!(time(&spring), EventTime { start: at(2026, 3, 8, 3, 0), end: at(2026, 3, 8, 13, 0), all_day: false });
    // Whole days are the same days everywhere, a day at least.
    let days = event(clock(2026, 10, 31, 0, 0), clock(2026, 11, 2, 0, 0), true);
    assert_eq!(time(&days), EventTime { start: at(2026, 10, 31, 0, 0), end: at(2026, 11, 2, 0, 0), all_day: true });
    let no_length = event(clock(2026, 10, 31, 0, 0), clock(2026, 10, 31, 0, 0), true);
    assert_eq!(time(&no_length).end, at(2026, 11, 1, 0, 0));
    // What is sent comes back as what it was: the wall-clock times of a server's instants.
    for e in [&october, &november, &night, &spring, &days] {
        let there = time(e);
        let mut back = Event::default();
        super::put_fields(&mut back, &EventFields { time: Some(there), ..Default::default() }, &new_york);
        assert_eq!((back.start, back.end, back.all_day), (e.start, e.end, e.all_day));
    }
}

#[test]
fn the_servers_answers_settle_what_was_noted() {
    let (mut s, account, first, second, server) = served();
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    run(&mut s, "calendar.delete", json!({"id": second}));
    let made = run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-15T12:00"}))["id"].as_u64().unwrap();
    assert!(s.can_undo());
    let upload = event_upload(&s, account).unwrap();
    assert_eq!(upload.ops.len(), 3);

    let applied = apply_event_outcomes(&mut s, &all_done(&upload)).unwrap();
    assert_eq!((applied.done, applied.refused, applied.failed, applied.waiting, applied.denied), (3, 0, 0, 0, false));
    assert!(applied.messages.is_empty() && s.mailbox.event_changes.is_empty());
    // The new event has the server's name for it and its change key; the moved one a new key.
    let new = s.mailbox.event(made).unwrap();
    assert_eq!((new.origin.id.as_str(), new.origin.change_key.as_str(), new.origin.account, new.calendar), ("made-2", "ck-new", account, server));
    assert_eq!(s.mailbox.event(first).map(|e| (e.origin.change_key.as_str(), e.start)), Some(("ck-new", clock(2026, 10, 14, 9, 0))));
    assert!(s.mailbox.event(second).is_none());
    // The server was told: what was before is not what it has any more, so there is no undoing it.
    assert!(!s.can_undo() && !s.can_redo());
    assert!(s.mailbox.clone().repair().is_empty());

    // The next download brings the same events: nothing is there twice, nothing comes back.
    let now = october(
        account,
        vec![
            RemoteEvent { change_key: "ck-new".into(), ..remote("e-1", "Budget review", at(2026, 10, 14, 13, 0), 60) },
            RemoteEvent { change_key: "ck-new".into(), reminder_minutes: Some(15), ..remote("made-2", "Lunch", at(2026, 10, 15, 16, 0), 30) },
        ],
    );
    assert_eq!(counts(&apply_events(&mut s, &now).unwrap()), (0, 0, 0, 2, 0));
    assert_eq!(titles(&s, account), ["Budget review", "Lunch"]);

    // The same through the command; and what isn't an answer is refused as parameters.
    run(&mut s, "calendar.edit", json!({"id": made, "title": "Late lunch"}));
    let upload = event_upload(&s, account).unwrap();
    let answer = run(&mut s, "sync.applyEventChanges", json!({"result": all_done(&upload)}));
    assert_eq!((answer["done"].as_u64(), answer["waiting"].as_u64(), answer["denied"].as_bool()), (Some(1), Some(0), Some(false)));
    assert!(matches!(s.run("sync.applyEventChanges", &json!({})), Err(CmdError::Params(_))));
    assert!(matches!(s.run("sync.applyEventChanges", &json!({"result": {"outcomes": 7}})), Err(CmdError::Params(_))));
    let stranger = EventsUploaded { email: "eve@example.org".into(), ..all_done(&upload) };
    assert_eq!(
        refused(&mut s, "sync.applyEventChanges", json!({"result": stranger})),
        "this download was for another account, not for ada@example.org"
    );
}

#[test]
fn text_that_is_cut_on_its_way_settles_all_the_same() {
    let (mut s, account, first, _, _) = served();
    // A description with line ends and spaces that are not sent as they are.
    run(&mut s, "calendar.edit", json!({"id": first, "description": "  Agenda:\r\n1. Numbers\u{7}  \r\n"}));
    let upload = event_upload(&s, account).unwrap();
    assert_eq!(upload.ops[0].fields.description.as_deref(), Some("Agenda:\n1. Numbers"));
    push_done(&mut s, account);
    // The server has what was sent. What is here differs from that only in what was cut away:
    // there is nothing more to tell, now or ever.
    assert!(s.mailbox.event_changes.is_empty() && ops(&s, account).is_empty());
    // A change that is only such a difference is no change.
    run(&mut s, "calendar.edit", json!({"id": first, "description": "Agenda:\n1. Numbers\r\n"}));
    assert!(s.mailbox.event_changes.is_empty());
    // And a new event with such text is made once.
    let made =
        run(&mut s, "calendar.new", json!({"title": "Notes", "start": "2026-10-15T12:00", "description": "One\r\nTwo  "}))["id"].as_u64().unwrap();
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty());
    assert_eq!(s.mailbox.event(made).map(|e| e.description.as_str()), Some("One\r\nTwo  "));
}

#[test]
fn undo_before_the_server_hears_cancels_the_change() {
    let (mut s, account, first, second, _) = served();
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    run(&mut s, "calendar.delete", json!({"id": second}));
    let made = run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-15T12:00"}))["id"].as_u64().unwrap();
    assert_eq!(ops(&s, account).len(), 3);
    // Each undo takes its change back, and with it the note that the server was to hear of it.
    assert!(s.undo());
    assert!(s.mailbox.event(made).is_none());
    assert_eq!(ops(&s, account).iter().map(|op| op.kind).collect::<Vec<_>>(), [EventChangeKind::Update, EventChangeKind::Delete]);
    assert!(s.undo());
    assert!(s.mailbox.event(second).is_some());
    assert!(s.undo());
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 12, 9, 0)));
    assert!(ops(&s, account).is_empty() && s.mailbox.event_changes.is_empty());
    // And redo notes it again.
    assert!(s.redo());
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.remote.as_str())).collect::<Vec<_>>(), [(EventChangeKind::Update, "e-1")]);
    // Moved back by hand to where the server has it: nothing is left to tell.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-12T09:00"}));
    assert!(s.mailbox.event_changes.is_empty());
}

#[test]
fn an_answer_to_a_change_that_was_undone_meanwhile_sends_the_opposite() {
    // The change was on its way when the owner took it back: the server did what the mailbox
    // no longer says.
    let (mut s, account, first, second, _) = served();

    // A move: the server has the event at its new time, the mailbox at the old. It is moved back there.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    let upload = event_upload(&s, account).unwrap();
    assert!(s.undo());
    apply_event_outcomes(&mut s, &all_done(&upload)).unwrap();
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 12, 9, 0)));
    let back = ops(&s, account);
    assert_eq!(back.len(), 1);
    let time = EventTime { start: at(2026, 10, 12, 13, 0), end: at(2026, 10, 12, 14, 0), all_day: false };
    assert_eq!(
        (back[0].kind, back[0].remote.as_str(), back[0].fields.clone()),
        (EventChangeKind::Update, "e-1", EventFields { time: Some(time), ..Default::default() })
    );
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty());

    // A new event: the server made it, and the mailbox no longer has it. It is deleted there.
    run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-15T12:00"}));
    let upload = event_upload(&s, account).unwrap();
    assert!(s.undo());
    apply_event_outcomes(&mut s, &all_done(&upload)).unwrap();
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.remote.as_str())).collect::<Vec<_>>(), [(EventChangeKind::Delete, "made-0")]);
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty() && !titles(&s, account).contains(&"Lunch".to_string()));

    // A deletion: the server deleted it, and the mailbox has it again. It is made anew there.
    run(&mut s, "calendar.delete", json!({"id": second}));
    let upload = event_upload(&s, account).unwrap();
    assert!(s.undo());
    apply_event_outcomes(&mut s, &all_done(&upload)).unwrap();
    let again = ops(&s, account);
    assert_eq!(
        again.iter().map(|op| (op.kind, op.event, op.fields.title.as_deref())).collect::<Vec<_>>(),
        [(EventChangeKind::Create, second, Some("Dentist"))]
    );
    assert_eq!(s.mailbox.event(second).map(|e| e.origin.id.as_str()), Some(""));

    // And one that was changed again while the first change was on its way: the rest goes after.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    let upload = EventsUpload {
        ops: ops(&s, account).into_iter().filter(|op| op.kind == EventChangeKind::Update).collect(),
        ..event_upload(&s, account).unwrap()
    };
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Budget review, again"}));
    apply_event_outcomes(&mut s, &all_done(&upload)).unwrap();
    let rest: Vec<EventOp> = ops(&s, account).into_iter().filter(|op| op.kind == EventChangeKind::Update).collect();
    assert_eq!(rest.len(), 1);
    assert_eq!(rest[0].fields, EventFields { title: Some("Budget review, again".into()), ..Default::default() });
}

#[test]
fn a_change_that_could_not_be_sent_waits_and_is_tried_later_each_time() {
    let (mut s, account, first, _, _) = served();
    let now = s.now();
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    let upload = event_upload(&s, account).unwrap();
    // No connection: the change stays as it is here, and waits half a minute.
    let applied = apply_event_outcomes(&mut s, &all_said(&upload, EventResult::Failed, "the server couldn't be reached")).unwrap();
    assert_eq!((applied.done, applied.failed, applied.refused, applied.waiting), (0, 1, 0, 1));
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 14, 9, 0)));
    let change = s.mailbox.event_changes[0].clone();
    assert_eq!((change.failures, change.retry_at, change.error.as_str()), (1, now + 30, "the server couldn't be reached"));
    // Nothing was told: undo still has the change.
    assert!(s.can_undo());
    // Until then nothing is asked for; when the time has come it is.
    assert!(ops(&s, account).is_empty());
    assert_eq!(events_waiting(&s, account), EventsWaiting { due: 0, total: 1, next: Some(now + 30) });
    // Twice as long after every failure, a quarter of an hour at most; a server that says how
    // long to stay away is believed when that is longer.
    for (failures, wait) in [(2, 60), (3, 120), (4, 240), (5, 480), (6, 900), (7, 900)] {
        s.clock.now = Some(now + 100_000 * i64::from(failures));
        let upload = event_upload(&s, account).unwrap();
        assert_eq!(upload.ops.len(), 1, "{failures}");
        apply_event_outcomes(&mut s, &all_said(&upload, EventResult::Failed, "busy")).unwrap();
        let change = &s.mailbox.event_changes[0];
        assert_eq!((change.failures, change.retry_at - s.now()), (failures, wait));
    }
    s.clock.now = Some(now + 10_000_000);
    let mut busy = all_said(&event_upload(&s, account).unwrap(), EventResult::Failed, "busy");
    busy.outcomes[0].retry_after = Some(3_600);
    apply_event_outcomes(&mut s, &busy).unwrap();
    assert_eq!(s.mailbox.event_changes[0].retry_at - s.now(), 3_600);
    // A download meanwhile doesn't put the event back to where the server still has it.
    let stale =
        october(account, vec![remote("e-1", "Budget review", at(2026, 10, 12, 13, 0), 60), remote("e-2", "Dentist", at(2026, 10, 13, 18, 0), 30)]);
    assert_eq!(counts(&apply_events(&mut s, &stale).unwrap()), (0, 0, 0, 2, 0));
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 14, 9, 0)));
    // The owner changes it again: that is something new to say, and it is said at once.
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Budget review, later"}));
    assert_eq!(ops(&s, account).len(), 1);
    // And once it goes through, the waiting is over.
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty());
    // A sign-in that changed lets what was held back go at once.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-15T09:00"}));
    push_said(&mut s, account, EventResult::Failed, "");
    assert!(ops(&s, account).is_empty());
    retry_event_changes(&mut s, account);
    assert_eq!(ops(&s, account).len(), 1);
}

#[test]
fn what_the_server_refuses_for_good_is_put_back_and_says_so() {
    let (mut s, account, first, second, server) = served();
    let local = s.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| c.id).unwrap();
    run(&mut s, "calendar.categorize", json!({"id": first, "category": "Green category"}));
    run(&mut s, "calendar.edit", json!({"id": first, "title": "Renamed", "start": "2026-10-14T09:00"}));
    run(&mut s, "calendar.delete", json!({"id": second}));
    let made = run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-15T12:00"}))["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.event(made).map(|e| e.calendar), Some(server));
    let upload = event_upload(&s, account).unwrap();

    let applied =
        apply_event_outcomes(&mut s, &all_said(&upload, EventResult::Refused, "Microsoft Graph answered with status 400 (ErrorInvalidRequest)"))
            .unwrap();
    assert_eq!((applied.done, applied.refused, applied.failed, applied.waiting), (0, 3, 0, 0));
    assert!(s.mailbox.event_changes.is_empty());
    // The change is taken back to what the server has, its categories too.
    let event = s.mailbox.event(first).unwrap();
    assert_eq!((event.title.as_str(), event.start, event.categories.len()), ("Budget review", clock(2026, 10, 12, 9, 0), 0));
    // The deleted event is back, under the id it had.
    assert_eq!(s.mailbox.event(second).map(|e| (e.title.as_str(), e.origin.id.as_str())), Some(("Dentist", "e-2")));
    // The new one is not thrown away: it is kept on this computer.
    assert_eq!(s.mailbox.event(made).map(|e| (e.calendar, e.origin.is_local())), Some((local, true)));
    assert_eq!(
        applied.messages,
        [
            "The server wouldn't take the change to \"Renamed\" (Microsoft Graph answered with status 400 (ErrorInvalidRequest)). It was put back as it was.",
            "The server wouldn't delete \"Dentist\" (Microsoft Graph answered with status 400 (ErrorInvalidRequest)). It is back in the calendar.",
            "The server wouldn't take \"Lunch\" (Microsoft Graph answered with status 400 (ErrorInvalidRequest)). It is kept on this device only.",
        ]
    );
    assert!(s.mailbox.clone().repair().is_empty());

    // The server no longer has an event that was changed here: it goes here too. One that was
    // deleted here is simply done.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    run(&mut s, "calendar.delete", json!({"id": second}));
    let applied = push_said(&mut s, account, EventResult::Gone, "");
    assert!(s.mailbox.event(first).is_none() && s.mailbox.event(second).is_none() && s.mailbox.event_changes.is_empty());
    assert_eq!(applied.messages, ["\"Budget review\" is no longer on the server, so it couldn't be changed there. It was removed here too."]);
}

#[test]
fn a_change_the_account_may_not_make_waits_for_permission_once() {
    let (mut s, account, first, _, _) = served();
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    // 403: the owner agreed to reading the calendar only. The change is kept, and the owner is
    // to be asked.
    let denied = |s: &mut Session| push_said(s, account, EventResult::Denied, "this account may not change its calendar through Microsoft Graph");
    let applied = denied(&mut s);
    assert_eq!((applied.denied, applied.refused, applied.failed, applied.waiting), (true, 0, 1, 1));
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 14, 9, 0)));
    assert!(s.mailbox.event_changes[0].denied && applied.messages.is_empty());
    // The permission was given (the sign-in changed): it goes again, and through.
    retry_event_changes(&mut s, account);
    let mut again = s.mailbox.clone();
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty());
    // Or it is refused a second time, permission or not: then it is for good, and put back.
    std::mem::swap(&mut s.mailbox, &mut again);
    let applied = denied(&mut s);
    assert_eq!((applied.denied, applied.refused, applied.waiting), (false, 1, 0));
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 12, 9, 0)));
    assert_eq!(applied.messages.len(), 1);
}

#[test]
fn a_download_does_not_undo_what_waits_or_make_an_event_twice() {
    let (mut s, account, first, second, _) = served();
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    run(&mut s, "calendar.delete", json!({"id": second}));
    let made = run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-15T12:00"}))["id"].as_u64().unwrap();
    let token = s.mailbox.event_changes.iter().find(|c| c.kind == EventChangeKind::Create).map(|c| c.token.clone()).unwrap();
    let snapshot = s.mailbox.clone();

    // The server's copies are the older ones: the moved event stays moved, the deleted one stays
    // deleted, and the new one, which the server doesn't have, is not taken for one it lost.
    let stale = vec![remote("e-1", "Budget review", at(2026, 10, 12, 13, 0), 60), remote("e-2", "Dentist", at(2026, 10, 13, 18, 0), 30)];
    let merged = apply_events(&mut s, &october(account, stale.clone())).unwrap();
    assert_eq!(counts(&merged), (0, 0, 0, 1, 1));
    assert!(s.mailbox == snapshot);

    // The new event reaches the server, but the answer is lost; the next download has it, with
    // the mark it was sent with. It is that event, not a second one.
    let upload = event_upload(&s, account).unwrap();
    apply_event_outcomes(&mut s, &all_said(&upload, EventResult::Failed, "the server closed the connection")).unwrap();
    let mut with_new = stale.clone();
    with_new.push(RemoteEvent { token: token.clone(), reminder_minutes: Some(15), ..remote("made-9", "Lunch", at(2026, 10, 15, 16, 0), 30) });
    let merged = apply_events(&mut s, &october(account, with_new.clone())).unwrap();
    assert_eq!(counts(&merged), (0, 1, 0, 1, 1));
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Lunch").count(), 1);
    assert_eq!(s.mailbox.event(made).map(|e| (e.origin.id.as_str(), e.origin.change_key.as_str())), Some(("made-9", "ck-made-9-1")));
    // The server has it as it is here: nothing more to make, and the rest still waits.
    assert_eq!(s.mailbox.event_changes.iter().map(|c| c.kind).collect::<Vec<_>>(), [EventChangeKind::Update, EventChangeKind::Delete]);

    // An event that was changed here and is not in what the server has in the stretch: a
    // download never takes it away while the change waits (it may be elsewhere there than it is
    // here, outside what was asked for). The server says so itself when it is told the change:
    // then it goes, the change with it, and the owner hears of it.
    s.clock.now = Some(s.now() + 10_000);
    let merged = apply_events(&mut s, &october(account, vec![with_new[2].clone()])).unwrap();
    assert_eq!((merged["removed"].as_u64(), merged["dropped"]["count"].as_u64()), (Some(0), Some(0)));
    assert!(s.mailbox.event(first).is_some());
    assert_eq!(s.mailbox.event_changes.iter().map(|c| c.kind).collect::<Vec<_>>(), [EventChangeKind::Update, EventChangeKind::Delete]);
    let applied = push_said(&mut s, account, EventResult::Gone, "");
    assert!(s.mailbox.event(first).is_none() && s.mailbox.event_changes.is_empty());
    assert_eq!(applied.messages, ["\"Budget review\" is no longer on the server, so it couldn't be changed there. It was removed here too."]);
    assert_eq!(
        applied.dropped.iter().map(|d| (d.kind, d.reason)).collect::<Vec<_>>(),
        [(EventChangeKind::Update, "the server no longer has the event")]
    );
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn hostile_answers_and_notes_change_nothing_they_should_not() {
    let (mut s, account, first, second, _) = served();
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    let snapshot = s.mailbox.clone();
    let op = |kind: EventChangeKind, remote: &str, token: &str| EventOp {
        kind,
        event: 0,
        remote: remote.into(),
        token: token.into(),
        ..Default::default()
    };
    let done =
        |op: EventOp| EventOutcome { op, result: EventResult::Done, remote: "x\r\ny".into(), change_key: "ck two".into(), ..Default::default() };
    // Answers to what was never asked: about no event, about one of another account's, with
    // names that can't be names. Nothing is made, changed or deleted for them.
    let junk = EventsUploaded {
        account,
        email: "ada@example.org".into(),
        outcomes: vec![
            done(op(EventChangeKind::Update, "", "")),
            done(op(EventChangeKind::Update, "no-such-event", "")),
            done(op(EventChangeKind::Delete, "two words", "")),
            done(op(EventChangeKind::Create, "", "no such mark!")),
            EventOutcome {
                op: op(EventChangeKind::Delete, "e-2", ""),
                result: EventResult::Refused,
                error: "x".repeat(10_000),
                ..Default::default()
            },
            EventOutcome { op: op(EventChangeKind::Create, "", ""), result: EventResult::Refused, ..Default::default() },
        ],
        ..Default::default()
    };
    let applied = apply_event_outcomes(&mut s, &junk).unwrap();
    assert!(applied.messages.iter().all(|line| line.len() < 600));
    assert_eq!(s.mailbox.events, snapshot.events);
    assert_eq!(s.mailbox.event_changes.len(), 1);
    assert!(s.mailbox.event(second).is_some());

    // A file from elsewhere with notes that don't hang together with it: none of them is sent.
    let base = Box::new(s.mailbox.event(second).unwrap().clone());
    let note = |kind: EventChangeKind, event: Id, remote: &str, token: &str| EventChange {
        account,
        kind,
        event,
        remote: remote.into(),
        token: token.into(),
        base: Some(base.clone()),
        ..Default::default()
    };
    s.mailbox.event_changes = vec![
        // Deleting an event that is still here; and one the mailbox never had.
        note(EventChangeKind::Delete, second, "e-2", ""),
        note(EventChangeKind::Delete, 0, "somebody-elses", ""),
        note(EventChangeKind::Delete, 0, "line\r\nbreak", ""),
        // Changing an event under another name than its own, or one that isn't there.
        note(EventChangeKind::Update, first, "e-2", ""),
        note(EventChangeKind::Update, 424_242, "e-1", ""),
        // Making an event the server already has, or with no mark, or for another account.
        note(EventChangeKind::Create, first, "", "mark"),
        note(EventChangeKind::Create, 424_242, "", "mark"),
        EventChange { account: 424_242, ..note(EventChangeKind::Delete, 0, "gone", "") },
    ];
    assert!(ops(&s, account).is_empty() && events_waiting(&s, account).total == 0);
    assert!(event_upload(&s, 424_242).is_err());
    // And the next answer or download clears them out.
    apply_events(
        &mut s,
        &october(account, vec![remote("e-1", "Budget review", at(2026, 10, 12, 13, 0), 60), remote("e-2", "Dentist", at(2026, 10, 13, 18, 0), 30)]),
    )
    .unwrap();
    assert!(s.mailbox.event_changes.is_empty(), "{:?}", s.mailbox.event_changes);
    // Notes in a file read and write with it, and a mailbox without any says nothing of them.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-16T09:00"}));
    let json = serde_json::to_string(&s.mailbox).unwrap();
    assert!(json.contains(r#""eventChanges":[{"account":"#), "{:.300}", json);
    let read: mailcraft_model::Mailbox = serde_json::from_str(&json).unwrap();
    assert_eq!(read.event_changes, s.mailbox.event_changes);
    assert!(!serde_json::to_string(&snapshot_without_changes(&s)).unwrap().contains("eventChanges"));
    let odd: mailcraft_model::Mailbox =
        serde_json::from_str(r#"{"eventChanges": [7, {"kind": "explode"}, {"account": 3, "kind": "delete", "remote": "x"}]}"#).unwrap();
    assert_eq!(odd.event_changes.len(), 1);
}

/// The mailbox as it is, with nothing waiting for a server.
fn snapshot_without_changes(s: &Session) -> mailcraft_model::Mailbox {
    let mut mailbox = s.mailbox.clone();
    mailbox.event_changes.clear();
    mailbox
}

#[test]
fn contacts_are_merged_and_those_from_a_server_are_not_changed_here() {
    let (mut s, account) = session();
    let before = own(&s);
    let ada = RemoteContact {
        display_name: "Ada Lovelace".into(),
        last: "Lovelace".into(),
        company: "Analytical Engines Ltd".into(),
        title: "Analyst".into(),
        phones: vec![Labeled::new("mobile", "+44 7700 900000")],
        birthday: Date::new(1815, 12, 10),
        notes: "Met at the Royal Society.".into(),
        ..person("c-ada", "Ada", "ada@example.org")
    };
    let download = everybody(account, vec![ada.clone(), person("c-bo", "Bo", "bo@example.org"), person("c-cy", "Cy", "cy@example.org")]);
    let (step, rev) = (s.undo_label(), s.rev());
    let merged = run(&mut s, "sync.applyContacts", json!({"download": download}));
    assert_eq!((counts(&merged), merged["complete"].as_bool(), merged["calendar"].is_null()), ((3, 0, 0, 0, 0), Some(true), true));
    assert_eq!(names(&s, account), ["Ada Lovelace", "Bo", "Cy"]);
    let kept = contact_called(&s, account, "c-ada");
    assert_eq!(
        kept,
        Contact {
            id: kept.id,
            display_name: "Ada Lovelace".into(),
            first: "Ada".into(),
            last: "Lovelace".into(),
            company: "Analytical Engines Ltd".into(),
            title: "Analyst".into(),
            emails: vec![Labeled::new("", "ada@example.org")],
            phones: vec![Labeled::new("mobile", "+44 7700 900000")],
            birthday: Date::new(1815, 12, 10),
            notes: "Met at the Royal Society.".into(),
            origin: Origin { account, id: "c-ada".into(), change_key: "ck-c-ada-1".into(), ..Default::default() },
            ..Default::default()
        }
    );
    assert!(own(&s) == before && s.undo_label() == step && s.rev() > rev);
    // The list of people shows them.
    let found = run(&mut s, "people.list", json!({"query": "Lovelace"}));
    assert_eq!(found["contacts"][0]["email"], "ada@example.org");

    // A contact from the server isn't changed or deleted here…
    let why =
        "This contact is kept by the server of ada@example.org. Changes made here are not sent back yet, so it can't be changed or deleted here.";
    let snapshot = s.mailbox.clone();
    assert_eq!(refused(&mut s, "people.edit", json!({"id": kept.id, "first": "Augusta"})), why);
    assert_eq!(refused(&mut s, "people.delete", json!({"id": kept.id})), why);
    run(&mut s, "people.select", json!({"id": kept.id}));
    assert_eq!(refused(&mut s, "people.delete", json!({})), why);
    assert!(s.mailbox == snapshot);
    // …but what is only kept here is: its categories, and whether it is a favourite.
    run(&mut s, "people.categorize", json!({"id": kept.id, "category": "Red category"}));
    run(&mut s, "people.favorite", json!({"id": kept.id, "value": true}));
    assert_eq!(run(&mut s, "people.get", json!({"id": kept.id}))["company"], "Analytical Engines Ltd");
    // And the mailbox's own contacts are changed as ever.
    let mine = run(&mut s, "people.new", json!({"first": "Dee", "email": "dee@example.org"}))["id"].as_u64().unwrap();
    run(&mut s, "people.edit", json!({"id": mine, "last": "Park"}));
    let before = own(&s);

    // Later: one changed, one gone, one new, one said twice, some that can't be kept.
    let later = everybody(
        account,
        vec![
            RemoteContact { change_key: "ck-c-ada-2".into(), company: "Difference Engines Ltd".into(), ..ada.clone() },
            person("c-cy", "Cy", "cy@example.org"),
            person("c-eve", "Eve", "eve@example.org"),
            person("c-eve", "Eve again", "eve@example.org"),
            person("", "Nameless", "x@example.org"),
            RemoteContact { id: "c-blank".into(), ..Default::default() },
        ],
    );
    let merged = apply_contacts(&mut s, &later).unwrap();
    assert_eq!(counts(&merged), (1, 1, 1, 1, 3));
    assert_eq!(names(&s, account), ["Ada Lovelace", "Cy", "Eve"]);
    // The same contact here, with what the server changed and what was kept here.
    let changed = contact_called(&s, account, "c-ada");
    assert_eq!((changed.id, changed.company.as_str(), changed.origin.change_key.as_str()), (kept.id, "Difference Engines Ltd", "ck-c-ada-2"));
    assert_eq!((changed.categories, changed.favorite), (vec!["Red category".to_string()], true));
    assert_eq!(own(&s), before);
    // Again: nothing to do, and nothing to save.
    let (snapshot, rev) = (s.mailbox.clone(), s.rev());
    s.dirty = false;
    assert_eq!(counts(&apply_contacts(&mut s, &later).unwrap()), (0, 0, 0, 3, 3));
    assert!(s.mailbox == snapshot && s.rev() == rev && !s.dirty);

    // A list that isn't all there is removes nobody.
    let partial = ContactsDownload { complete: false, ..everybody(account, vec![person("c-fay", "Fay", "fay@example.org")]) };
    assert_eq!(counts(&apply_contacts(&mut s, &partial).unwrap()), (1, 0, 0, 0, 0));
    assert_eq!(names(&s, account).len(), 4);
    // Somebody else's list, or nobody's, changes nothing.
    let snapshot = s.mailbox.clone();
    let stranger = ContactsDownload { email: "eve@example.org".into(), ..everybody(account, Vec::new()) };
    assert_eq!(
        refused(&mut s, "sync.applyContacts", json!({"download": stranger})),
        "this download was for another account, not for ada@example.org"
    );
    assert!(merge_contacts(&mut s.mailbox, &everybody(424_242, Vec::new())).is_err());
    assert!(s.mailbox == snapshot);
    // An empty list that is all there is: everybody from the server goes, the one that is
    // selected too, and nobody else.
    run(&mut s, "people.select", json!({"id": kept.id}));
    assert_eq!(counts(&apply_contacts(&mut s, &everybody(account, Vec::new())).unwrap()), (0, 0, 4, 0, 0));
    assert!(names(&s, account).is_empty() && s.sel.contact.is_none());
    assert_eq!(own(&s), before);
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn an_account_can_be_forgotten() {
    let (mut s, account) = session();
    let other = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"]
        .as_u64()
        .unwrap();
    let before = own(&s);
    apply_events(
        &mut s,
        &october(account, vec![remote("e-1", "Mine", at(2026, 10, 12, 13, 0), 30), remote("e-2", "Mine too", at(2026, 10, 13, 13, 0), 30)]),
    )
    .unwrap();
    apply_contacts(&mut s, &everybody(account, vec![person("c-1", "Cy", "cy@example.org")])).unwrap();
    let theirs = EventsDownload { email: "bo@example.org".into(), ..october(other, vec![remote("e-1", "Theirs", at(2026, 10, 12, 13, 0), 30)]) };
    apply_events(&mut s, &theirs).unwrap();
    apply_contacts(&mut s, &ContactsDownload { email: "bo@example.org".into(), ..everybody(other, vec![person("c-1", "Dee", "dee@example.org")]) })
        .unwrap();
    let calendars = s.mailbox.calendars.len();

    let step = s.undo_label();
    let forgotten = run(&mut s, "sync.forgetRemote", json!({"account": account}));
    assert_eq!(forgotten, json!({"events": 2, "contacts": 1, "calendars": 1}));
    assert!(from_server(&s, account).is_empty() && names(&s, account).is_empty());
    assert_eq!((s.mailbox.calendars.len(), s.mailbox.calendars.iter().any(|c| c.origin.account == account)), (calendars - 1, false));
    // The other account's, and the mailbox's own, are as they were; and it is no undo step.
    assert_eq!((titles(&s, other), names(&s, other)), (vec!["Theirs".to_string()], vec!["Dee".to_string()]));
    assert!(own(&s) == before && s.undo_label() == step);
    // Nothing left to forget, and nothing to save for it.
    let rev = s.rev();
    assert_eq!(forget(&mut s, account).unwrap(), json!({"events": 0, "contacts": 0, "calendars": 0}));
    assert_eq!(s.rev(), rev);
    assert!(matches!(s.run("sync.forgetRemote", &json!({})), Err(CmdError::Params(_))));
    assert!(matches!(forget(&mut s, 0), Err(CmdError::Params(_))));
    // A download brings it all back.
    apply_events(&mut s, &october(account, vec![remote("e-1", "Mine", at(2026, 10, 12, 13, 0), 30)])).unwrap();
    assert_eq!(titles(&s, account), ["Mine"]);

    // A mailbox whose only calendar is a server's keeps one, as its own.
    let (mut s, account) = session();
    apply_events(&mut s, &october(account, vec![remote("e-1", "Mine", at(2026, 10, 12, 13, 0), 30)])).unwrap();
    let kept = s.mailbox.calendars.iter().find(|c| c.origin.account == account).unwrap().id;
    s.mailbox.calendars.retain(|c| c.id == kept);
    for event in s.mailbox.events.values_mut() {
        std::sync::Arc::make_mut(event).calendar = kept;
    }
    let own_events = own(&s).0.len();
    assert_eq!(forget(&mut s, account).unwrap(), json!({"events": 1, "contacts": 0, "calendars": 0}));
    assert_eq!(s.mailbox.calendars.len(), 1);
    assert!(s.mailbox.calendars[0].origin.is_local() && s.mailbox.calendars[0].id == kept);
    assert_eq!(own(&s).0.len(), own_events);
    assert!(s.mailbox.clone().repair().is_empty());
    run(&mut s, "calendar.new", json!({"title": "Goes somewhere", "start": "2026-10-12T15:00"}));
}

/// #325: an event made in the account's calendar that its server wasn't given yet is the
/// owner's: forgetting the account keeps it, as the mailbox's own, in a calendar of the mailbox's
/// own though another account's calendar comes first in the list, and nothing of it waits to go.
#[test]
fn forgetting_an_account_keeps_what_was_made_here() {
    let (mut s, account) = session();
    let other = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"]
        .as_u64()
        .unwrap();
    apply_events(&mut s, &october(account, vec![remote("e-1", "Mine", at(2026, 10, 12, 13, 0), 30)])).unwrap();
    let theirs = EventsDownload { email: "bo@example.org".into(), ..october(other, vec![remote("e-1", "Theirs", at(2026, 10, 12, 13, 0), 30)]) };
    apply_events(&mut s, &theirs).unwrap();
    s.mailbox.calendars.sort_by_key(|c| c.origin.account != other);
    let server = s.mailbox.calendars.iter().find(|c| c.origin.account == account).map(|c| c.id).unwrap();
    let made = run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-16T12:00", "calendar": server}))["id"].as_u64().unwrap();
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.event)).collect::<Vec<_>>(), [(EventChangeKind::Create, made)]);

    assert_eq!(run(&mut s, "sync.forgetRemote", json!({"account": account})), json!({"events": 1, "contacts": 0, "calendars": 1}));
    let lunch = s.mailbox.event(made).unwrap();
    assert!(lunch.origin.is_local(), "{:?}", lunch.origin);
    assert!(s.mailbox.calendar(lunch.calendar).is_some_and(|c| c.origin.is_local()), "{:?}", s.mailbox.calendars);
    assert!(from_server(&s, account).is_empty() && !s.mailbox.event_changes.iter().any(|c| c.account == account));
    assert_eq!(titles(&s, other), ["Theirs"]);
    assert!(s.mailbox.clone().repair().is_empty());
}

/// With no calendar of the mailbox's own left (`calendar.moveToAccount` takes one it emptied),
/// what was made here in the account's calendar doesn't go into another account's, whose server
/// would never be sent it: the account's main calendar becomes the mailbox's own, and the other
/// account's calendars stay as they are, its main one too, though it comes first in the list.
#[test]
fn forgetting_an_account_keeps_a_calendar_of_the_mailboxs_own_for_what_was_made_here() {
    let (mut s, account) = session();
    let other = run(&mut s, "account.add", json!({"email": "bo@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"]
        .as_u64()
        .unwrap();
    apply_events(&mut s, &october(account, vec![remote("e-1", "Mine", at(2026, 10, 12, 13, 0), 30)])).unwrap();
    let theirs = EventsDownload { email: "bo@example.org".into(), ..october(other, vec![remote("e-1", "Theirs", at(2026, 10, 12, 13, 0), 30)]) };
    apply_events(&mut s, &theirs).unwrap();
    s.mailbox.calendars.retain(|c| !c.origin.is_local());
    s.mailbox.calendars.sort_by_key(|c| c.origin.account != other);
    let theirs: Vec<mailcraft_model::Calendar> = s.mailbox.calendars.iter().filter(|c| c.origin.account == other).cloned().collect();
    let server = s.mailbox.calendars.iter().find(|c| c.origin.account == account).map(|c| c.id).unwrap();
    let made = run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-16T12:00", "calendar": server}))["id"].as_u64().unwrap();

    run(&mut s, "sync.forgetRemote", json!({"account": account}));
    let lunch = s.mailbox.event(made).unwrap();
    assert!(lunch.origin.is_local(), "{:?}", lunch.origin);
    assert!(s.mailbox.calendar(lunch.calendar).is_some_and(|c| c.origin.is_local()), "{:?}", s.mailbox.calendars);
    assert_eq!(s.mailbox.calendars.iter().filter(|c| c.origin.account == other).cloned().collect::<Vec<_>>(), theirs);
    assert_eq!(s.mailbox.calendars.len(), theirs.len() + 1);
    assert!(from_server(&s, account).is_empty());
    assert_eq!(titles(&s, other), ["Theirs"]);
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn hostile_downloads_are_refused_or_cut_to_size() {
    let (mut s, account) = session();
    for junk in [
        json!(null),
        json!({}),
        json!({"download": null}),
        json!({"download": []}),
        json!({"download": "x"}),
        json!({"download": {"account": "one"}}),
        json!({"download": {"account": account, "events": {"id": "e"}, "contacts": 5}}),
        json!({"download": {"account": account, "from": 1e300, "to": "later", "events": [], "contacts": "x"}}),
        json!({"download": {"account": account, "events": [{"id": 5}], "contacts": [{"id": 5}]}}),
        json!({"download": {"account": account, "events": [{"id": "e", "start": "now"}], "contacts": [{"id": "c", "birthday": "soon"}]}}),
        json!({"download": {"account": account, "events": [{"id": "e", "kind": "weekly"}], "contacts": [{"id": "c", "emails": "x"}]}}),
        json!({"download": {"account": -1}}),
        json!({"download": {"account": 18446744073709551615u64, "complete": true}}),
        json!({"download": {"account": account, "email": "\u{0}"}}),
        json!({"download": {"account": account, "calendar": "x".repeat(5_000), "contacts": 7}}),
    ] {
        for command in ["sync.applyEvents", "sync.applyContacts", "sync.forgetRemote"] {
            let result = s.run(command, &junk);
            assert!(result.is_err(), "{command} {junk}: {result:?}");
            // What is said of it is short, whatever was sent.
            assert!(result.unwrap_err().to_string().chars().count() < 400, "{command} {junk}");
        }
    }
    let long = json!({"download": {"account": "x".repeat(100_000)}});
    assert!(s.run("sync.applyEvents", &long).unwrap_err().to_string().chars().count() < 400);
    assert!(from_server(&s, account).is_empty() && names(&s, account).is_empty());

    // What can be read is taken, cut to size, with nothing in it that isn't what it says.
    let odd = json!({"download": {
        "account": account, "complete": true, "from": i64::MIN, "to": i64::MAX,
        "events": [
            {"id": "e-1", "title": "\u{0}Tea\r\ntime", "start": i64::MAX, "end": i64::MIN, "allDay": true, "attendees": [{"address": {"email": "\r\n"}}, {"address": {"name": "Bo", "email": "bo@example.org\r\nBcc: eve@example.org"}}]},
            {"id": "e-2", "start": i64::MIN, "end": i64::MAX, "reminderMinutes": 4294967295u32, "organizer": {"name": "", "email": ""}},
            {"id": "e\u{0}3"}, {"id": ""}, {},
        ],
        "contacts": [
            {"id": "c-1", "displayName": "\u{0}Ada\r\nLovelace", "emails": [{"value": "ada@example.org\r\nBcc: eve@example.org"}, {"label": "\u{0}", "value": "ada@example.org"}], "birthday": {"year": 0, "month": 13, "day": 32}},
            {"id": "c-2", "first": "\u{202e}"}, {"id": "c 3", "first": "Cy"}, {},
        ],
    }});
    assert_eq!(counts(&run(&mut s, "sync.applyEvents", odd.clone())), (2, 0, 0, 0, 3));
    let tea = event_called(&s, account, "e-1");
    assert_eq!((tea.title.as_str(), tea.all_day, tea.start.date.year, tea.attendees.len()), ("Tea  time", true, 9999, 1));
    assert_eq!(tea.attendees[0].address, Address { name: "Bo".into(), email: String::new() });
    let early = event_called(&s, account, "e-2");
    assert_eq!((early.start.date.year, early.end.date.year, early.reminder_minutes, early.organizer.clone()), (1, 9999, Some(40_320), None));
    assert_eq!(counts(&run(&mut s, "sync.applyContacts", odd.clone())), (2, 0, 0, 0, 2));
    let ada = contact_called(&s, account, "c-1");
    assert_eq!((ada.display_name.as_str(), ada.emails.clone(), ada.birthday), ("Ada  Lovelace", vec![Labeled::new("", "ada@example.org")], None));
    assert!(s.mailbox.clone().repair().is_empty());
    // The same again is the same again.
    assert_eq!(counts(&run(&mut s, "sync.applyEvents", odd.clone())), (0, 0, 0, 2, 3));
    assert_eq!(counts(&run(&mut s, "sync.applyContacts", odd)), (0, 0, 0, 2, 2));

    // A file from elsewhere with the same server event twice: a download leaves one.
    let double = Event { id: 0, ..event_called(&s, account, "e-1") };
    let copy = s.mailbox.add_event(double).unwrap();
    let everything = EventsDownload {
        from: i64::MIN,
        to: i64::MAX,
        events: vec![RemoteEvent { all_day: true, ..remote("e-1", "Tea  time", i64::MAX, 0) }],
        ..october(account, Vec::new())
    };
    let merged = apply_events(&mut s, &everything).unwrap();
    assert_eq!((merged["removed"].as_u64(), from_server(&s, account).iter().filter(|e| e.origin.id == "e-1").count()), (Some(2), 1));
    assert!(s.mailbox.event(copy).is_some() != s.mailbox.event(tea.id).is_some());
}

/// An event with anything at all in it.
fn any_remote_event() -> impl Strategy<Value = RemoteEvent> {
    let who = (any::<String>(), "\\PC{0,12}(@example\\.org)?").prop_map(|(name, email)| Address { name, email });
    let people = proptest::collection::vec(
        (who.clone(), any::<bool>()).prop_map(|(address, optional)| Attendee { address, optional, ..Default::default() }),
        0..4,
    );
    let kind = prop_oneof![
        Just(RemoteEventKind::Single),
        Just(RemoteEventKind::Occurrence),
        Just(RemoteEventKind::Exception),
        Just(RemoteEventKind::SeriesMaster)
    ];
    let instant = prop_oneof![any::<i64>(), 1_700_000_000i64..1_900_000_000];
    (
        ("[a-e]{0,2}|\\PC{0,8}", any::<String>(), any::<String>(), instant.clone(), instant),
        (any::<bool>(), proptest::option::of(who), people, kind, proptest::option::of(any::<u32>())),
    )
        .prop_map(|((id, title, description, start, end), (all_day, organizer, attendees, kind, reminder_minutes))| RemoteEvent {
            change_key: title.clone(),
            series: id.clone(),
            id,
            title,
            description,
            start,
            end,
            all_day,
            organizer,
            attendees,
            kind,
            reminder_minutes,
            ..Default::default()
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_download_merges_without_panic_and_settles(
        events in proptest::collection::vec(any_remote_event(), 0..12),
        contacts in proptest::collection::vec(("[a-e]{0,2}|\\PC{0,8}", any::<String>(), "\\PC{0,12}(@example\\.org)?"), 0..12),
        complete in any::<bool>(),
        window in (any::<i64>(), any::<i64>()),
        offset in prop_oneof![Just(0i32), Just(-240), Just(840), any::<i32>()],
    ) {
        let (mut s, account) = session();
        let before = own(&s);
        let download = EventsDownload { from: window.0, to: window.1, complete, events, ..october(account, Vec::new()) };
        let merged = merge_events(&mut s.mailbox, &download, &|_| offset).unwrap();
        prop_assert_eq!(merged.added + merged.skipped, download.events.len());
        // What is there is consistent, and the events are where the server's events go.
        prop_assert!(s.mailbox.clone().repair().is_empty());
        for event in from_server(&s, account) {
            prop_assert!(event.end >= event.start && event.calendar == merged.calendar && !event.origin.id.is_empty());
        }
        // A second time adds nothing and changes nothing; it may remove only what a complete download leaves out.
        let again = merge_events(&mut s.mailbox, &download, &|_| offset).unwrap();
        prop_assert_eq!((again.added, again.updated, again.new_calendar), (0, 0, false));
        prop_assert_eq!(again.removed, 0);
        prop_assert_eq!(again.unchanged, merged.added);

        let people = ContactsDownload {
            complete,
            contacts: contacts.into_iter().map(|(id, name, email)| RemoteContact { id, display_name: name.clone(), notes: name, emails: vec![Labeled::new("", email)], ..Default::default() }).collect(),
            ..everybody(account, Vec::new())
        };
        let merged = merge_contacts(&mut s.mailbox, &people).unwrap();
        prop_assert_eq!(merged.added + merged.skipped, people.contacts.len());
        let again = merge_contacts(&mut s.mailbox, &people).unwrap();
        prop_assert_eq!((again.added, again.updated, again.removed, again.unchanged), (0, 0, 0, merged.added));
        // Nothing of the mailbox's own was touched by any of it.
        prop_assert_eq!(own(&s), before);
        prop_assert!(s.mailbox.clone().repair().is_empty());
    }

    #[test]
    fn an_instant_and_its_wall_clock_time_name_each_other(instant in 1_750_000_000i64..1_800_000_000, minutes in 0i64..10_000) {
        // But for the hour that the clock shows twice when it goes back.
        prop_assume!(!(at(2026, 11, 1, 5, 0)..at(2026, 11, 1, 7, 0)).contains(&instant));
        let event = event_from_remote(&RemoteEvent { start: instant, end: instant + minutes * 60, ..Default::default() }, "", &new_york);
        prop_assert_eq!(server_start(&event, &new_york), instant);
        prop_assert_eq!(event.duration_minutes().abs_diff(minutes) <= 60, true);
        // A whole day is counted by its date alone.
        let day = (instant / 86_400) * 86_400;
        let whole = event_from_remote(&RemoteEvent { start: day, end: day + 86_400, all_day: true, ..Default::default() }, "", &new_york);
        prop_assert_eq!(server_start(&whole, &new_york), day);
        prop_assert_eq!(whole.duration_minutes(), 24 * 60);
    }
}

fn day(month: u8, day: u8) -> Date {
    Date::new(2026, month, day).unwrap()
}

/// A series as a server has it: it starts at `start` and repeats by `rule`.
fn master(id: &str, title: &str, start: Timestamp, minutes: i64, rule: Recurrence) -> RemoteEvent {
    let series_day = DateTime::from_timestamp(start, -240).date;
    RemoteEvent { kind: RemoteEventKind::SeriesMaster, recurrence: Some(rule), series_day: Some(series_day), ..remote(id, title, start, minutes) }
}

/// One occurrence of the series `of`, as the series has it.
fn instance(of: &str, title: &str, start: Timestamp, minutes: i64) -> RemoteEvent {
    let id = format!("{of}-{}", DateTime::from_timestamp(start, 0).date.iso());
    RemoteEvent { kind: RemoteEventKind::Occurrence, series: of.into(), original: Some(start), ..remote(&id, title, start, minutes) }
}

/// The stand-up: Monday, Wednesday and Friday at nine in the morning here (13:00 UTC), half an
/// hour, from Monday the 5th of October. With it: its occurrences on `days` of October.
fn standup(days: &[u8]) -> Vec<RemoteEvent> {
    let rule = Recurrence {
        by_day: vec![mailcraft_time::Weekday::Mon, mailcraft_time::Weekday::Wed, mailcraft_time::Weekday::Fri],
        ..Recurrence::every(Freq::Weekly)
    };
    let mut events: Vec<RemoteEvent> = days.iter().map(|d| instance("s-1", "Stand-up", at(2026, 10, *d, 13, 0), 30)).collect();
    events.push(master("s-1", "Stand-up", at(2026, 10, 5, 13, 0), 30, rule));
    events
}

const STANDUP_DAYS: [u8; 12] = [5, 7, 9, 12, 14, 16, 19, 21, 23, 26, 28, 30];

/// The days of October on which the calendar shows an event with this title, with its hour.
fn shown(s: &Session, title: &str) -> Vec<(u8, u8)> {
    let mut on: Vec<(u8, u8)> = s
        .mailbox
        .events
        .values()
        .filter(|e| e.title == title)
        .flat_map(|e| e.occurrences(day(10, 1), day(10, 31)))
        .map(|o| (o.start.date.day, o.start.hour))
        .collect();
    on.sort();
    on
}

#[test]
fn a_series_is_one_event_that_repeats_and_skips_what_the_server_does_not_have() {
    let (mut s, account) = session();
    // The 16th was deleted on the server; the 21st was moved to eleven and renamed there.
    let mut events = standup(&[5, 7, 9, 12, 14, 19, 23, 26, 28, 30]);
    let late = RemoteEvent {
        id: "x-21".into(),
        kind: RemoteEventKind::Exception,
        original: Some(at(2026, 10, 21, 13, 0)),
        ..instance("s-1", "Stand-up (late)", at(2026, 10, 21, 15, 0), 30)
    };
    events.push(late.clone());
    let download = october(account, events);
    assert_eq!(counts(&apply_events(&mut s, &download).unwrap()), (2, 0, 0, 0, 0));
    // One event for the series, with its rule; and one for the day that was changed.
    let series = event_called(&s, account, "s-1");
    assert_eq!(series.recurrence.as_ref().map(|r| r.describe(series.start.date)), Some("Every week on Monday, Wednesday, Friday".to_string()));
    assert_eq!((series.start, series.end), (clock(2026, 10, 5, 9, 0), clock(2026, 10, 5, 9, 30)));
    assert_eq!(series.exceptions, [day(10, 16), day(10, 21)]);
    let changed = event_called(&s, account, "x-21");
    assert_eq!((changed.origin.series.as_str(), changed.origin.occurrence, changed.recurrence.is_some()), ("s-1", Some(day(10, 21)), false));
    assert_eq!(from_server(&s, account).len(), 2);
    // What the calendar shows is what the server has: every day but the 16th, the 21st at eleven.
    let nine: Vec<(u8, u8)> = [5, 7, 9, 12, 14, 19, 23, 26, 28, 30].into_iter().map(|d| (d, 9)).collect();
    assert_eq!(shown(&s, "Stand-up"), nine);
    assert_eq!(shown(&s, "Stand-up (late)"), [(21, 11)]);
    // Doing it again changes nothing.
    assert_eq!(counts(&apply_events(&mut s, &download).unwrap()), (0, 0, 0, 2, 0));

    // The 16th is back on the server, the 21st is as the series has it again, the 30th is gone,
    // and the series was renamed: the same two events here become one, changed.
    let mut events = standup(&[5, 7, 9, 12, 14, 16, 19, 21, 23, 26, 28]);
    events.last_mut().unwrap().title = "Daily".into();
    events.last_mut().unwrap().change_key = "ck-2".into();
    let merged = apply_events(&mut s, &october(account, events)).unwrap();
    assert_eq!(counts(&merged), (0, 1, 1, 0, 0));
    let renamed = event_called(&s, account, "s-1");
    assert_eq!((renamed.id, renamed.title.as_str(), renamed.exceptions.as_slice()), (series.id, "Daily", &[day(10, 30)][..]));
    assert_eq!(from_server(&s, account).len(), 1);

    // November says nothing about October: what October skips stays, and what November skips is added.
    let rule = renamed.recurrence.clone().unwrap();
    let mut november: Vec<RemoteEvent> = [2u8, 4, 6, 9, 13].iter().map(|d| instance("s-1", "Daily", at(2026, 11, *d, 14, 0), 30)).collect();
    november.push(RemoteEvent { change_key: "ck-2".into(), ..master("s-1", "Daily", at(2026, 10, 5, 13, 0), 30, rule.clone()) });
    let stretch = EventsDownload { from: at(2026, 11, 1, 0, 0), to: at(2026, 11, 15, 0, 0), ..october(account, november.clone()) };
    apply_events(&mut s, &stretch).unwrap();
    // (Nine o'clock is 14:00 UTC once the clocks have gone back, but this session's zone is fixed:
    // those occurrences are an hour off the series here, and are kept as what they are.)
    let kept = event_called(&s, account, "s-1");
    assert!(kept.exceptions.contains(&day(10, 30)) && kept.exceptions.contains(&day(11, 11)), "{:?}", kept.exceptions);
    let off: Vec<_> =
        from_server(&s, account).into_iter().filter(|e| e.origin.occurrence.is_some()).map(|e| (e.start.date.day, e.start.hour)).collect();
    assert_eq!(off, [(2, 10), (4, 10), (6, 10), (9, 10), (13, 10)]);
    assert!(off.iter().all(|(d, _)| kept.exceptions.contains(&day(11, *d))));

    // A download that isn't complete takes nothing away: neither events nor days.
    let partial = EventsDownload { complete: false, ..october(account, standup(&[5])) };
    let before = event_called(&s, account, "s-1").exceptions.clone();
    apply_events(&mut s, &partial).unwrap();
    assert_eq!((event_called(&s, account, "s-1").exceptions, event_called(&s, account, "s-1").title.as_str()), (before, "Stand-up"));

    // The series can't be asked for just now, though its occurrences come: it stays as it is,
    // and they are not taken for events of their own.
    let events = from_server(&s, account).len();
    let without: Vec<RemoteEvent> = standup(&[5, 7, 9]).into_iter().filter(|e| e.kind != RemoteEventKind::SeriesMaster).collect();
    apply_events(&mut s, &EventsDownload { complete: false, ..october(account, without) }).unwrap();
    assert_eq!(from_server(&s, account).len(), events);

    // Its rule is changed to one that can't be said here: the series goes, and its occurrences
    // are events of their own, as they were before series came as series.
    let mut ruleless = standup(&[5, 7]);
    ruleless.last_mut().unwrap().recurrence = None;
    apply_events(&mut s, &october(account, ruleless)).unwrap();
    assert!(from_server(&s, account).iter().all(|e| e.recurrence.is_none()));
    assert_eq!(shown(&s, "Stand-up"), [(5, 9), (7, 9)]);
    // And back: the loose ones are the series' own again.
    apply_events(&mut s, &october(account, standup(&[5, 7]))).unwrap();
    assert_eq!(shown(&s, "Stand-up"), [(5, 9), (7, 9)]);
    assert_eq!(from_server(&s, account).iter().filter(|e| e.title == "Stand-up").count(), 1);

    // The server says the series is gone: it goes, with what was kept of its occurrences.
    apply_events(&mut s, &EventsDownload { gone: vec!["s-1".into(), "two words".into()], ..october(account, Vec::new()) }).unwrap();
    assert!(from_server(&s, account).iter().all(|e| e.origin.id != "s-1" && e.origin.series != "s-1"));
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn a_series_made_elsewhere_keeps_its_days_of_the_week_here() {
    // Made in Tokyo for Tuesday mornings at nine (midnight UTC), which is Monday evening at
    // eight in a zone four hours behind UTC.
    let (mut s, account) = session();
    let rule = Recurrence { by_day: vec![mailcraft_time::Weekday::Tue], until: Some(day(10, 27)), ..Recurrence::every(Freq::Weekly) };
    let tokyo = RemoteEvent { series_day: Some(day(10, 6)), ..master("s-t", "Tokyo call", at(2026, 10, 6, 0, 0), 60, rule) };
    let mut events: Vec<RemoteEvent> = [6u8, 13, 20, 27].iter().map(|d| instance("s-t", "Tokyo call", at(2026, 10, *d, 0, 0), 60)).collect();
    events.push(tokyo);
    apply_events(&mut s, &october(account, events)).unwrap();
    let series = event_called(&s, account, "s-t");
    let here = series.recurrence.clone().unwrap();
    assert_eq!((here.by_day.clone(), here.until, series.start), (vec![mailcraft_time::Weekday::Mon], Some(day(10, 26)), clock(2026, 10, 5, 20, 0)));
    assert_eq!((series.exceptions.len(), from_server(&s, account).len()), (0, 1));
    assert_eq!(shown(&s, "Tokyo call"), [(5, 20), (12, 20), (19, 20), (26, 20)]);
}

#[test]
fn one_day_of_a_series_or_all_of_it_is_changed_on_the_server() {
    let (mut s, account) = session();
    apply_events(&mut s, &october(account, standup(&STANDUP_DAYS))).unwrap();
    let series = event_called(&s, account, "s-1");
    assert!(series.exceptions.is_empty() && s.mailbox.event_changes.is_empty());

    // Just the 14th, an hour later: the series skips that day, and what happens on it is an
    // event of its own. The server is told what changed of the occurrence it has at that time.
    let moved =
        run(&mut s, "calendar.move", json!({"id": series.id, "start": "2026-10-05T10:00", "scope": "occurrence", "occurrence": "2026-10-14"}));
    let one = moved["id"].as_u64().unwrap();
    assert_eq!((moved["series"].as_u64(), moved["occurrence"].as_str()), (Some(series.id), Some("2026-10-14")));
    let event = s.mailbox.event(one).unwrap().clone();
    assert_eq!((event.start, event.end, event.recurrence.is_some()), (clock(2026, 10, 14, 10, 0), clock(2026, 10, 14, 10, 30), false));
    assert_eq!((event.origin.series.as_str(), event.origin.occurrence, event.origin.id.as_str()), ("s-1", Some(day(10, 14)), ""));
    assert_eq!(s.mailbox.event(series.id).unwrap().exceptions, [day(10, 14)]);
    assert_eq!((s.sel.event, s.sel.event_day), (Some(one), Some(day(10, 14))));
    let told = ops(&s, account);
    assert_eq!(told.len(), 1);
    assert_eq!(
        (told[0].kind, told[0].remote.as_str(), told[0].series.as_str(), told[0].occurrence),
        (EventChangeKind::Update, "", "s-1", at(2026, 10, 14, 13, 0))
    );
    let time = EventTime { start: at(2026, 10, 14, 14, 0), end: at(2026, 10, 14, 14, 30), all_day: false };
    assert_eq!(told[0].fields, EventFields { time: Some(time), ..Default::default() });
    // A download before the server has heard doesn't put the day back, or make a second event.
    apply_events(&mut s, &october(account, standup(&STANDUP_DAYS))).unwrap();
    assert_eq!(s.mailbox.event(series.id).unwrap().exceptions, [day(10, 14)]);
    assert_eq!(from_server(&s, account).len(), 2);
    // The server made the occurrence an event of its own, and says what it calls it.
    let upload = event_upload(&s, account).unwrap();
    let mut done = all_done(&upload);
    done.outcomes[0].remote = "x-14".into();
    apply_event_outcomes(&mut s, &done).unwrap();
    assert_eq!(s.mailbox.event(one).map(|e| (e.origin.id.as_str(), e.origin.change_key.as_str())), Some(("x-14", "ck-new")));
    assert!(s.mailbox.event_changes.is_empty());
    // From now on it is changed, and deleted, by its own name.
    run(&mut s, "calendar.edit", json!({"id": one, "location": "Room 2"}));
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.remote.as_str())).collect::<Vec<_>>(), [(EventChangeKind::Update, "x-14")]);
    push_done(&mut s, account);

    // Just the 19th is deleted: the server is to delete the occurrence it has then.
    run(&mut s, "calendar.select", json!({"id": series.id, "day": "2026-10-19"}));
    let gone = run(&mut s, "calendar.delete", json!({"scope": "occurrence"}));
    assert_eq!(gone["occurrence"], "2026-10-19");
    assert_eq!(s.mailbox.event(series.id).unwrap().exceptions, [day(10, 14), day(10, 19)]);
    let told = ops(&s, account);
    assert_eq!(
        (told[0].kind, told[0].series.as_str(), told[0].occurrence, told[0].cancel),
        (EventChangeKind::Delete, "s-1", at(2026, 10, 19, 13, 0), false)
    );
    // The server won't have it: the day is back, and the owner is told.
    let applied = push_said(&mut s, account, EventResult::Refused, "no");
    assert_eq!(applied.messages, ["The server wouldn't take deleting one occurrence of \"Stand-up\" (no). It was put back as it was."]);
    assert_eq!(s.mailbox.event(series.id).unwrap().exceptions, [day(10, 14)]);
    // Deleted for good the second time; a day that is skipped already, or isn't one of the
    // series', is nothing to delete.
    run(&mut s, "calendar.delete", json!({"id": series.id, "occurrence": "2026-10-19"}));
    push_done(&mut s, account);
    run(&mut s, "calendar.delete", json!({"id": series.id, "occurrence": "2026-10-19"}));
    assert!(s.mailbox.event_changes.is_empty());
    assert_eq!(refused(&mut s, "calendar.delete", json!({"id": series.id, "occurrence": "2026-10-20"})), "the series doesn't happen on 2026-10-20");
    assert_eq!(
        refused(&mut s, "calendar.move", json!({"id": series.id, "start": "2026-10-05T11:00", "occurrence": "2026-10-14"})),
        "the series doesn't happen on 2026-10-14"
    );

    // The entire series: its name, then its hour (by the clock, so that nine stays nine), then
    // its rule. Each is told to the series itself.
    run(&mut s, "calendar.edit", json!({"id": series.id, "title": "Daily sync", "scope": "series"}));
    let told = ops(&s, account);
    assert_eq!((told[0].kind, told[0].remote.as_str()), (EventChangeKind::Update, "s-1"));
    assert_eq!(told[0].fields, EventFields { title: Some("Daily sync".into()), ..Default::default() });
    push_done(&mut s, account);
    run(&mut s, "calendar.move", json!({"id": series.id, "start": "2026-10-05T09:30"}));
    let fields = ops(&s, account)[0].fields.clone();
    assert_eq!(fields.wall, Some(EventWall { start: clock(2026, 10, 5, 9, 30), end: clock(2026, 10, 5, 10, 0) }));
    assert_eq!((fields.time.map(|t| t.start), fields.repeat.is_none()), (Some(at(2026, 10, 5, 13, 30)), true));
    push_done(&mut s, account);
    run(
        &mut s,
        "calendar.edit",
        json!({"id": series.id, "recurrence": {"freq": "weekly", "byDay": ["tue"], "count": 4}, "start": "2026-10-06T09:30"}),
    );
    let fields = ops(&s, account)[0].fields.clone();
    let rule = Recurrence { by_day: vec![mailcraft_time::Weekday::Tue], count: Some(4), ..Recurrence::every(Freq::Weekly) };
    assert_eq!(fields.repeat, Some(EventRepeat { rule: Some(rule), first: day(10, 6) }));
    assert!(fields.time.is_some() && fields.wall.is_some());
    push_done(&mut s, account);
    // No longer a series at all.
    run(&mut s, "calendar.edit", json!({"id": series.id, "recurrence": null}));
    assert_eq!(ops(&s, account)[0].fields, EventFields { repeat: Some(EventRepeat { rule: None, first: day(10, 6) }), ..Default::default() });
    push_done(&mut s, account);

    // Which occurrence is meant has to be said, or selected.
    let again = run(&mut s, "calendar.new", json!({"title": "Gym", "start": "2026-10-12T18:00", "recurrence": {"freq": "daily"}}))["id"].clone();
    push_done(&mut s, account);
    run(&mut s, "calendar.select", json!({"id": null}));
    let unsaid = s.run("calendar.move", &json!({"id": again, "start": "2026-10-12T19:00", "scope": "occurrence"}));
    assert_eq!(unsaid, Err(CmdError::Params("`occurrence` (a day, YYYY-MM-DD) says which occurrence is meant".into())));
    assert_eq!(
        s.run("calendar.delete", &json!({"id": again, "scope": "some"})),
        Err(CmdError::Params("`scope` is \"occurrence\" or \"series\"".into()))
    );
    // Deleting the series deletes it on the server, with what was kept of its changed days.
    run(&mut s, "calendar.move", json!({"id": again, "start": "2026-10-12T19:00", "occurrence": "2026-10-13"}));
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Gym").count(), 2);
    run(&mut s, "calendar.delete", json!({"id": again, "scope": "series"}));
    assert_eq!(s.mailbox.events.values().filter(|e| e.title == "Gym").count(), 0);
    assert_eq!(ops(&s, account).iter().map(|op| (op.kind, op.remote.as_str())).collect::<Vec<_>>(), [(EventChangeKind::Delete, "made-0")]);
}

#[test]
fn one_day_of_a_series_on_this_computer_becomes_an_event_of_its_own() {
    let (mut s, account) = session();
    let series =
        run(&mut s, "calendar.new", json!({"title": "Gym", "start": "2026-10-12T18:00", "recurrence": {"freq": "daily"}}))["id"].as_u64().unwrap();
    // The form says everything of the series; one occurrence is changed by as much as differs.
    let whole = json!({"id": series, "title": "Gym (pool)", "start": "2026-10-12T19:00", "end": "2026-10-12T20:00", "recurrence": {"freq": "daily"}, "scope": "occurrence", "occurrence": "2026-10-15"});
    let one = run(&mut s, "calendar.edit", whole)["id"].as_u64().unwrap();
    let event = s.mailbox.event(one).unwrap().clone();
    assert_eq!((event.title.as_str(), event.start, event.end), ("Gym (pool)", clock(2026, 10, 15, 19, 0), clock(2026, 10, 15, 20, 0)));
    assert!(event.recurrence.is_none() && event.origin.is_local());
    let rest = s.mailbox.event(series).unwrap().clone();
    assert_eq!((rest.title.as_str(), rest.start, rest.exceptions.as_slice()), ("Gym", clock(2026, 10, 12, 18, 0), &[day(10, 15)][..]));
    // Nothing differs: nothing is taken out of the series.
    let same = run(&mut s, "calendar.edit", json!({"id": series, "title": "Gym", "scope": "occurrence", "occurrence": "2026-10-16"}));
    assert_eq!((same["id"].as_u64(), s.mailbox.event(series).unwrap().exceptions.len()), (Some(series), 1));
    // Without a word of which, it is the series, as it always was.
    run(&mut s, "calendar.move", json!({"id": series, "start": "2026-10-12T17:00"}));
    assert_eq!(s.mailbox.event(series).map(|e| e.start), Some(clock(2026, 10, 12, 17, 0)));
    assert!(s.mailbox.event_changes.is_empty() && ops(&s, account).is_empty());
}

/// A session whose account's server has a meeting the owner called, "m-1" (Bo and Cy are
/// invited), and one Bo called, "g-1" (the owner and Cy are invited). With the account and the
/// two events' ids here.
fn meetings() -> (Session, Id, Id, Id) {
    let (mut s, account) = session();
    let invite = |name: &str, email: &str| Attendee { address: Address::new(name, email), ..Default::default() };
    let own = RemoteEvent {
        organizer: Some(Address::new("Ada", "ada@example.org")),
        attendees: vec![invite("Bo", "bo@example.org"), invite("Cy", "cy@example.org")],
        is_organizer: Some(true),
        ..remote("m-1", "Planning", at(2026, 10, 12, 13, 0), 60)
    };
    let theirs = RemoteEvent {
        organizer: Some(Address::new("Bo", "bo@example.org")),
        attendees: vec![invite("Ada", "ada@example.org"), invite("Cy", "cy@example.org")],
        is_organizer: Some(false),
        ..remote("g-1", "Board", at(2026, 10, 13, 13, 0), 60)
    };
    apply_events(&mut s, &october(account, vec![own, theirs])).unwrap();
    let (own, theirs) = (event_called(&s, account, "m-1").id, event_called(&s, account, "g-1").id);
    (s, account, own, theirs)
}

#[test]
fn nobody_is_written_to_unless_the_owner_says_so() {
    let (mut s, account, own, _) = meetings();
    let snapshot = s.mailbox.clone();
    let update = "2 people are invited to this meeting, and changing it makes the account's server send them an update by mail. Nothing was changed. Say that this is meant with \"send\": true.";
    let cancel = "2 people are invited to this meeting, and deleting it makes the account's server send them a cancellation by mail. Nothing was changed. Say that this is meant with \"send\": true.";
    // Everything the people invited would hear of is refused without the word, and with a no.
    for (command, params, why) in [
        ("calendar.move", json!({"id": own, "start": "2026-10-12T10:00"}), update),
        ("calendar.move", json!({"id": own, "start": "2026-10-12T10:00", "send": false}), update),
        ("calendar.edit", json!({"id": own, "title": "Planning, again"}), update),
        ("calendar.edit", json!({"id": own, "attendees": "bo@example.org"}), update),
        ("calendar.edit", json!({"id": own, "recurrence": {"freq": "weekly"}, "send": "yes"}), update),
        ("calendar.private", json!({"id": own}), update),
        ("calendar.delete", json!({"id": own}), cancel),
        ("calendar.delete", json!({"id": own, "confirmed": true}), cancel),
    ] {
        assert_eq!(refused(&mut s, command, params.clone()), why, "{command} {params}");
        assert!(s.mailbox == snapshot, "{command} {params}");
    }
    // How it shows in the owner's own calendar is nobody else's business: no word is needed,
    // and none of it is anything the people invited are told.
    run(&mut s, "calendar.showAs", json!({"id": own, "value": "tentative"}));
    run(&mut s, "calendar.reminder", json!({"id": own, "minutes": 5}));
    run(&mut s, "calendar.categorize", json!({"id": own, "category": "Blue category"}));
    assert!(!s.mailbox.event_changes[0].send);
    push_done(&mut s, account);

    // With the word: the change goes, and carries the owner's yes.
    run(&mut s, "calendar.move", json!({"id": own, "start": "2026-10-12T10:00", "send": true}));
    assert!(s.mailbox.event_changes[0].send);
    run(&mut s, "calendar.edit", json!({"id": own, "attendees": "bo@example.org; cy@example.org", "optional": "dee@example.org", "send": true}));
    let told = ops(&s, account);
    assert_eq!(told.len(), 1);
    let people: Vec<_> = told[0].fields.attendees.clone().unwrap().into_iter().map(|a| (a.address.email, a.optional, a.response)).collect();
    assert_eq!(
        people,
        [
            ("bo@example.org".to_string(), false, Response::None),
            ("cy@example.org".to_string(), false, Response::None),
            ("dee@example.org".to_string(), true, Response::None)
        ]
    );
    assert!(told[0].fields.time.is_some() && !told[0].cancel);
    // A mailbox file that says a change is to go, and not that the owner said yes: it doesn't go.
    let noted = s.mailbox.event_changes.clone();
    s.mailbox.event_changes[0].send = false;
    assert!(ops(&s, account).is_empty());
    assert_eq!(events_waiting(&s, account).total, 0);
    s.mailbox.event_changes = noted;
    push_done(&mut s, account);

    // Who answered what comes with every download, and is nothing to send back.
    let answered = |response: Response| Attendee { address: Address::new("Bo", "bo@example.org"), optional: false, response };
    let mut now = RemoteEvent {
        organizer: Some(Address::new("Ada", "ada@example.org")),
        attendees: vec![answered(Response::Declined)],
        is_organizer: Some(true),
        change_key: "ck-3".into(),
        // (As it was sent: an hour later, and tentative.)
        show_as: ShowAs::Tentative,
        ..remote("m-1", "Planning", at(2026, 10, 12, 14, 0), 60)
    };
    apply_events(&mut s, &october(account, vec![now.clone()])).unwrap();
    assert_eq!(s.mailbox.event(own).unwrap().attendees, [answered(Response::Declined)]);
    now.attendees = vec![answered(Response::Accepted)];
    now.change_key = "ck-4".into();
    apply_events(&mut s, &october(account, vec![now])).unwrap();
    assert_eq!(s.mailbox.event(own).unwrap().attendees, [answered(Response::Accepted)]);
    assert!(s.mailbox.event_changes.is_empty());

    // Called off, with a note for the people invited.
    run(&mut s, "calendar.delete", json!({"id": own, "send": true, "comment": " Sorry,\r\nsomething came up. "}));
    let told = ops(&s, account);
    assert_eq!(told.len(), 1);
    assert_eq!(
        (told[0].kind, told[0].remote.as_str(), told[0].cancel, told[0].comment.as_str()),
        (EventChangeKind::Delete, "m-1", true, "Sorry,\nsomething came up.")
    );
    // The server won't: it is back, and says so.
    let applied = push_said(&mut s, account, EventResult::Refused, "no");
    assert_eq!(applied.messages, ["The server wouldn't delete \"Planning\" (no). It is back in the calendar."]);
    assert!(s.mailbox.event(own).is_some());

    // A meeting made here that was taken back while it was on its way is not deleted on the
    // server behind the owner's back (that would be a cancellation in everybody's mail).
    let made = run(&mut s, "calendar.newMeeting", json!({"title": "Sync", "start": "2026-10-15T10:00", "attendees": "bo@example.org", "send": true}));
    let upload = event_upload(&s, account).unwrap();
    assert!(s.undo());
    assert!(s.mailbox.event(made["id"].as_u64().unwrap()).is_none());
    let applied = apply_event_outcomes(&mut s, &all_done(&upload)).unwrap();
    assert!(s.mailbox.event_changes.is_empty() && ops(&s, account).is_empty());
    assert_eq!(applied.messages.len(), 1);
    assert!(applied.messages[0].starts_with("The invitations to \"Sync\" were already sent"), "{}", applied.messages[0]);
}

#[test]
fn an_invitation_is_answered_on_the_server() {
    let (mut s, account, own, theirs) = meetings();
    // The owner's own meeting has nobody to answer.
    assert_eq!(
        refused(&mut s, "calendar.respond", json!({"id": own, "response": "accept"})),
        "You organize this meeting, so there is nobody to send an answer to."
    );
    // Accepted, and the organizer told, with a note.
    let answer = run(&mut s, "calendar.respond", json!({"id": theirs, "response": "accept", "send": true, "comment": "See you there"}));
    assert_eq!((answer["response"].as_str(), answer["sent"].as_bool()), (Some("accepted"), Some(true)));
    let event = s.mailbox.event(theirs).unwrap().clone();
    assert_eq!((event.attendees[0].response, event.attendees[1].response, event.show_as), (Response::Accepted, Response::None, ShowAs::Busy));
    let told = ops(&s, account);
    assert_eq!(told.len(), 1);
    assert_eq!(
        (told[0].kind, told[0].remote.as_str(), told[0].response, told[0].send, told[0].comment.as_str()),
        (EventChangeKind::Respond, "g-1", Response::Accepted, true, "See you there")
    );
    // Until the server has heard, a download leaves the answer as it is here.
    apply_events(&mut s, &october(account, vec![remote("g-1", "Board", at(2026, 10, 13, 13, 0), 60)])).unwrap();
    assert_eq!(s.mailbox.event(theirs).unwrap().attendees.len(), 2);
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty());

    // Tentatively, and nobody told: that is what is meant when nothing is said about telling.
    let answer = run(&mut s, "calendar.respond", json!({"id": theirs, "response": "tentative", "comment": "not sent"}));
    assert_eq!(answer["sent"], false);
    assert_eq!(s.mailbox.event(theirs).unwrap().show_as, ShowAs::Tentative);
    let told = ops(&s, account);
    assert_eq!((told[0].response, told[0].send), (Response::Tentative, false));
    push_done(&mut s, account);

    // Declined: it is taken out of the calendar, and a download doesn't bring it back meanwhile.
    run(&mut s, "calendar.respond", json!({"id": theirs, "response": "decline", "send": true}));
    assert!(s.mailbox.event(theirs).is_none());
    let board = RemoteEvent {
        organizer: Some(Address::new("Bo", "bo@example.org")),
        is_organizer: Some(false),
        ..remote("g-1", "Board", at(2026, 10, 13, 13, 0), 60)
    };
    apply_events(&mut s, &october(account, vec![board.clone()])).unwrap();
    assert!(from_server(&s, account).iter().all(|e| e.origin.id != "g-1"));
    // The server won't take the answer: the meeting is back as it was.
    let applied = push_said(&mut s, account, EventResult::Refused, "no");
    assert_eq!(applied.messages, ["The server wouldn't take your answer to \"Board\" (no). It was put back as it was."]);
    assert_eq!(s.mailbox.event(theirs).map(|e| e.show_as), Some(ShowAs::Tentative));

    // Deleting an invitation: only the owner's copy (nobody is told anything), or declining it
    // (and the organizer is told, or not).
    run(&mut s, "calendar.delete", json!({"id": theirs}));
    let told = ops(&s, account);
    assert_eq!((told[0].kind, told[0].cancel, told[0].response), (EventChangeKind::Delete, false, Response::None));
    assert!(s.undo());
    run(&mut s, "calendar.delete", json!({"id": theirs, "decline": true, "send": true, "comment": "Can't make it"}));
    let told = ops(&s, account);
    assert_eq!(
        (told[0].kind, told[0].response, told[0].send, told[0].comment.as_str()),
        (EventChangeKind::Respond, Response::Declined, true, "Can't make it")
    );
    assert!(s.undo());
    run(&mut s, "calendar.delete", json!({"id": theirs, "decline": true}));
    assert_eq!(ops(&s, account).iter().map(|op| (op.response, op.send)).collect::<Vec<_>>(), [(Response::Declined, false)]);
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty() && s.mailbox.event(theirs).is_none());
    // An answer that isn't one is no answer.
    apply_events(&mut s, &october(account, vec![board])).unwrap();
    let back = event_called(&s, account, "g-1").id;
    assert!(matches!(s.run("calendar.respond", &json!({"id": back, "response": "maybe"})), Err(CmdError::Params(_))));
}

#[test]
fn a_mailbox_from_before_series_and_meetings_went_to_the_server_is_taken_as_it_is() {
    // As the first wave left a mailbox: a series that came as loose occurrences, a meeting and
    // a series that were kept "On this computer", and changes that were waiting to go.
    let (mut s, account, own, _) = meetings();
    let loose: Vec<RemoteEvent> = standup(&[5, 7, 9]).into_iter().filter(|e| e.kind != RemoteEventKind::SeriesMaster).collect();
    apply_events(&mut s, &EventsDownload { complete: false, ..october(account, loose) }).unwrap();
    assert_eq!(from_server(&s, account).iter().filter(|e| e.origin.series == "s-1" && e.recurrence.is_none()).count(), 3);
    let local = s.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| (c.id, c.name.clone())).unwrap();
    assert_eq!(local.1, "On this computer");
    let kept = json!({"title": "Team lunch", "start": "2026-10-20T12:00", "attendees": "bo@example.org; cy@example.org", "calendar": local.0});
    let lunch = run(&mut s, "calendar.newMeeting", kept)["id"].as_u64().unwrap();
    let gym =
        run(&mut s, "calendar.new", json!({"title": "Gym", "start": "2026-10-12T18:00", "recurrence": {"freq": "weekly"}, "calendar": local.0}));
    let gym = gym["id"].as_u64().unwrap();
    // Waiting, as the first wave noted them: an appointment that was moved, and a meeting of
    // the owner's that was deleted (asked about then, but with no word kept of the answer).
    let dentist = run(&mut s, "calendar.new", json!({"title": "Dentist", "start": "2026-10-21T15:00"}))["id"].as_u64().unwrap();
    push_done(&mut s, account);
    run(&mut s, "calendar.move", json!({"id": dentist, "start": "2026-10-22T15:00"}));
    run(&mut s, "calendar.delete", json!({"id": own, "send": true}));
    for change in &mut s.mailbox.event_changes {
        change.send = false;
    }
    let saved = serde_json::to_string(&s.mailbox).unwrap();
    for new in ["\"send\"", "\"occurrence\"", "\"readOnly\"", "\"comment\"", "\"response\":\"declined\"", "\"group\""] {
        assert!(!saved.contains(new), "{new} is not something the first wave wrote");
    }

    // It loads, and is the mailbox it was.
    let loaded: mailcraft_model::Mailbox = serde_json::from_str(&saved).unwrap();
    assert!(loaded == s.mailbox);
    let mut s = Session::at(loaded, at(2026, 10, 14, 16, 0), -240);
    assert!(s.mailbox.clone().repair().is_empty());
    // What waited and writes to nobody still goes. The deletion of the meeting doesn't: it
    // would be a cancellation in two people's mail, and nothing says the owner agreed to that.
    let told = ops(&s, account);
    assert_eq!(
        told.iter().map(|op| (op.kind, op.fields.attendees.is_some(), op.cancel)).collect::<Vec<_>>(),
        [(EventChangeKind::Update, false, false)]
    );
    assert_eq!(told[0].event, dentist);
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty());

    // The next download: the series is one event now, and the meeting that was "deleted" is back
    // (the server still has it), for the owner to call off or keep.
    let mut events = standup(&[5, 7, 9]);
    let invite = |name: &str, email: &str| Attendee { address: Address::new(name, email), ..Default::default() };
    events.push(RemoteEvent {
        attendees: vec![invite("Bo", "bo@example.org"), invite("Cy", "cy@example.org")],
        is_organizer: Some(true),
        ..remote("m-1", "Planning", at(2026, 10, 12, 13, 0), 60)
    });
    events.push(remote("made-0", "Dentist", at(2026, 10, 22, 19, 0), 30));
    apply_events(&mut s, &EventsDownload { from: at(2026, 10, 1, 0, 0), to: at(2026, 10, 11, 0, 0), ..october(account, events) }).unwrap();
    let series: Vec<Event> = from_server(&s, account).into_iter().filter(|e| e.title == "Stand-up").collect();
    assert_eq!((series.len(), series[0].recurrence.is_some(), series[0].exceptions.len()), (1, true, 0));
    assert!(from_server(&s, account).iter().any(|e| e.origin.id == "m-1"));

    // What was kept on this computer is still there, as it was, and nothing about it waits to
    // go anywhere: nobody was invited, and nobody is.
    let (lunch, gym) = (s.mailbox.event(lunch).unwrap().clone(), s.mailbox.event(gym).unwrap().clone());
    assert!(lunch.origin.is_local() && gym.origin.is_local() && lunch.calendar == local.0 && gym.calendar == local.0);
    assert_eq!((lunch.attendees.len(), gym.recurrence.is_some()), (2, true));
    assert!(s.mailbox.event_changes.is_empty() && ops(&s, account).is_empty());
    // They go to the server when the owner moves them there; the meeting only with a yes.
    let server = s.mailbox.calendars.iter().find(|c| c.origin.account == account).map(|c| c.id).unwrap();
    assert!(refused(&mut s, "calendar.edit", json!({"id": lunch.id, "calendar": server})).contains("Say that this is meant with \"send\": true."));
    assert!(s.mailbox.event_changes.is_empty());
    run(&mut s, "calendar.edit", json!({"id": gym.id, "calendar": server}));
    run(&mut s, "calendar.edit", json!({"id": lunch.id, "calendar": server, "send": true}));
    let told = ops(&s, account);
    assert_eq!(
        told.iter().map(|op| (op.kind, op.fields.repeat.is_some(), op.fields.attendees.is_some())).collect::<Vec<_>>(),
        [(EventChangeKind::Create, true, false), (EventChangeKind::Create, false, true)]
    );
}

/// A message from Bo in the account's Inbox that carries an iCalendar part: `method` for the
/// meeting `uid`, an hour from ten in the morning here on Thursday the 15th.
fn mail_with_calendar(s: &mut Session, account: Id, method: &str, uid: &str) -> Id {
    let ics = format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nMETHOD:{method}\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:Design review\r\nLOCATION:Room 4\r\n\
         DTSTART:20261015T140000Z\r\nDTEND:20261015T150000Z\r\nORGANIZER;CN=Bo Marsh:mailto:bo@example.org\r\n\
         ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:ada@example.org\r\n\
         ATTENDEE;PARTSTAT=ACCEPTED:mailto:cy@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"
    );
    let inbox = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
    let message = mailcraft_model::Message {
        account,
        folder: inbox,
        message_id: format!("{uid}.{method}@example.org"),
        from: Address::new("Bo Marsh", "bo@example.org"),
        to: vec![Address::new("Ada", "ada@example.org")],
        subject: "Design review".into(),
        attachments: vec![mailcraft_model::Attachment::new("invite.ics", "text/calendar", ics.into_bytes())],
        ..Default::default()
    };
    s.mailbox.add_message(message).unwrap()
}

/// What waits in the account's Outbox: who each message is to, its subject, and its calendar part.
fn outbox(s: &Session, account: Id) -> Vec<(String, String, String)> {
    let folder = s.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Outbox).unwrap();
    let part = |m: &mailcraft_model::Message| m.attachments.first().map(|a| String::from_utf8_lossy(&a.data).into_owned()).unwrap_or_default();
    s.mailbox.messages_in(folder).map(|m| (m.to.iter().map(|a| a.email.clone()).collect::<Vec<_>>().join(","), m.subject.clone(), part(m))).collect()
}

#[test]
fn an_invitation_in_the_mail_is_read_and_answered_on_this_computer() {
    // An account whose calendar no server keeps (nothing has come down from one).
    let (mut s, account) = session();
    let dentist = run(&mut s, "calendar.new", json!({"title": "Dentist", "start": "2026-10-15T10:30"}))["id"].as_u64().unwrap();
    let request = mail_with_calendar(&mut s, account, "REQUEST", "uid-1");
    let said = run(&mut s, "calendar.invitation", json!({"message": request}));
    assert_eq!((said["method"].as_str(), said["invitation"].as_bool(), said["cancellation"].as_bool()), (Some("REQUEST"), Some(true), Some(false)));
    assert_eq!((said["title"].as_str(), said["location"].as_str()), (Some("Design review"), Some("Room 4")));
    assert_eq!((said["start"].as_str(), said["end"].as_str()), (Some("2026-10-15T10:00:00"), Some("2026-10-15T11:00:00")));
    assert_eq!(
        (said["organizer"].as_str(), said["attendees"].as_u64(), said["uid"].as_str()),
        (Some("Bo Marsh <bo@example.org>"), Some(2), Some("uid-1"))
    );
    assert_eq!((said["event"].clone(), said["onServer"].as_bool(), said["response"].as_str()), (Value::Null, Some(false), Some("none")));
    // What else is on at that time is said with it.
    let clash: Vec<_> = said["conflicts"].as_array().unwrap().iter().filter(|c| c["id"].as_u64() == Some(dentist)).collect();
    assert_eq!(clash.len(), 1);
    assert_eq!(clash[0]["start"], "2026-10-15T10:30:00");
    assert!(s.mailbox.events.values().all(|e| e.uid != "uid-1"), "reading it changes nothing");

    // Accepted, and nobody told: it is on a calendar here, and nothing waits to be sent.
    let answer = run(&mut s, "calendar.respond", json!({"message": request, "response": "accept"}));
    let event = answer["event"].as_u64().unwrap();
    assert_eq!((answer["sent"].as_bool(), answer["onServer"].as_bool()), (Some(false), Some(false)));
    let here = s.mailbox.event(event).unwrap().clone();
    assert_eq!(
        (here.title.as_str(), here.start, here.origin.is_local(), here.show_as),
        ("Design review", clock(2026, 10, 15, 10, 0), true, ShowAs::Busy)
    );
    assert_eq!(
        here.attendees.iter().map(|a| (a.address.email.as_str(), a.response)).collect::<Vec<_>>(),
        [("ada@example.org", Response::Accepted), ("cy@example.org", Response::Accepted)]
    );
    assert!(outbox(&s, account).is_empty() && s.mailbox.message(request).unwrap().answered);
    assert_eq!(run(&mut s, "calendar.invitation", json!({"message": request}))["event"].as_u64(), Some(event));

    // Tentatively, and the organizer told: the same event, and an iCalendar REPLY in the Outbox.
    let answer = run(&mut s, "calendar.respond", json!({"message": request, "response": "tentative", "send": true, "comment": "Might be late"}));
    assert_eq!((answer["event"].as_u64(), answer["sent"].as_bool()), (Some(event), Some(true)));
    assert_eq!(s.mailbox.event(event).map(|e| e.show_as), Some(ShowAs::Tentative));
    assert_eq!(s.mailbox.events.values().filter(|e| e.uid == "uid-1").count(), 1);
    let waiting = outbox(&s, account);
    assert_eq!(waiting.len(), 1);
    assert_eq!((waiting[0].0.as_str(), waiting[0].1.as_str()), ("bo@example.org", "Tentative: Design review"));
    let reply = &waiting[0].2;
    assert!(reply.contains("METHOD:REPLY\r\n") && reply.contains("UID:uid-1\r\n"), "{reply}");
    assert!(reply.contains("PARTSTAT=TENTATIVE") && reply.contains("mailto:ada@example.org") && !reply.contains("cy@example.org"), "{reply}");
    // Declined: out of the calendar, and the organizer told that too.
    run(&mut s, "calendar.respond", json!({"message": request, "response": "decline", "send": true}));
    assert!(s.mailbox.event(event).is_none());
    assert_eq!(outbox(&s, account).iter().map(|m| m.1.as_str()).collect::<Vec<_>>(), ["Tentative: Design review", "Declined: Design review"]);

    // The organizer calls it off: that is no invitation to answer, and takes the meeting out
    // of the calendar when the owner says so.
    let event = run(&mut s, "calendar.respond", json!({"message": request, "response": "accept"}))["event"].as_u64().unwrap();
    let cancel = mail_with_calendar(&mut s, account, "CANCEL", "UID-1");
    assert_eq!(run(&mut s, "calendar.invitation", json!({"message": cancel}))["cancellation"], true);
    assert_eq!(
        refused(&mut s, "calendar.respond", json!({"message": cancel, "response": "accept"})),
        "This message isn't an invitation that can be answered."
    );
    assert_eq!(refused(&mut s, "calendar.removeCancelled", json!({"message": request})), "This message doesn't call a meeting off.");
    assert_eq!(run(&mut s, "calendar.removeCancelled", json!({"message": cancel}))["removed"].as_u64(), Some(event));
    assert!(s.mailbox.event(event).is_none());
    assert_eq!(run(&mut s, "calendar.removeCancelled", json!({"message": cancel}))["removed"], Value::Null);
    // A message can't call off what isn't somebody else's meeting.
    let own = run(&mut s, "calendar.new", json!({"title": "Mine", "start": "2026-10-16T10:00"}))["id"].as_u64().unwrap();
    s.mailbox.event_mut(own).unwrap().uid = "uid-mine".into();
    let forged = mail_with_calendar(&mut s, account, "CANCEL", "uid-mine");
    assert_eq!(
        refused(&mut s, "calendar.removeCancelled", json!({"message": forged})),
        "This is not somebody else's meeting: a message can't call it off."
    );

    // A message with no calendar part, or with one that says nothing, carries no invitation.
    let plain = s.mailbox.messages.values().find(|m| m.attachments.is_empty()).map(|m| m.id).unwrap();
    assert_eq!(refused(&mut s, "calendar.invitation", json!({"message": plain})), "This message carries no invitation.");
    let noise = mail_with_calendar(&mut s, account, "REQUEST", "x");
    s.mailbox.message_mut(noise).unwrap().attachments[0] =
        mailcraft_model::Attachment::new("invite.ics", "text/calendar", vec![0xFF, 0x00, b'B', b'E', b'G']);
    assert_eq!(
        refused(&mut s, "calendar.respond", json!({"message": noise, "response": "accept", "send": true})),
        "This message carries no invitation."
    );
}

#[test]
fn an_invitation_in_the_mail_is_answered_through_the_servers_copy() {
    let (mut s, account, _, theirs) = meetings();
    s.mailbox.event_mut(theirs).unwrap().uid = "040000008200E00074C5B7101A82E008".into();
    let request = mail_with_calendar(&mut s, account, "REQUEST", "040000008200e00074c5b7101a82e008");
    let said = run(&mut s, "calendar.invitation", json!({"message": request}));
    assert_eq!((said["event"].as_u64(), said["onServer"].as_bool()), (Some(theirs), Some(true)));
    // Answered on the server, which tells the organizer: no mail of our own is written.
    let answer = run(&mut s, "calendar.respond", json!({"message": request, "response": "accept", "send": true, "comment": "Yes"}));
    assert_eq!((answer["event"].as_u64(), answer["onServer"].as_bool(), answer["sent"].as_bool()), (Some(theirs), Some(true), Some(true)));
    let told = ops(&s, account);
    assert_eq!(
        (told[0].kind, told[0].remote.as_str(), told[0].response, told[0].send, told[0].comment.as_str()),
        (EventChangeKind::Respond, "g-1", Response::Accepted, true, "Yes")
    );
    assert!(outbox(&s, account).is_empty());
    // One that the server hasn't put on the calendar yet is not made up here beside it.
    let early = mail_with_calendar(&mut s, account, "REQUEST", "uid-not-here");
    let events = s.mailbox.events.len();
    let why = refused(&mut s, "calendar.respond", json!({"message": early, "response": "accept", "send": true}));
    assert!(why.starts_with("This meeting isn't on the account's calendar here yet"), "{why}");
    assert_eq!((s.mailbox.events.len(), outbox(&s, account).len()), (events, 0));
}

#[test]
fn every_calendar_of_the_account_is_a_calendar_here() {
    use mailcraft_model::Rgb;
    use mailcraft_model::remote::{CalendarsDownload, CategoriesDownload, RemoteCalendar};
    let (mut s, account, first, _, main) = served();
    let list = |calendars: Vec<RemoteCalendar>, complete: bool| CalendarsDownload {
        account,
        email: "ada@example.org".into(),
        complete,
        calendars,
        ..Default::default()
    };
    let entry = |id: &str, name: &str, group: &str| RemoteCalendar { id: id.into(), name: name.into(), group: group.into(), ..Default::default() };
    let all = vec![
        RemoteCalendar { main: true, color: Some(Rgb(1, 2, 3)), ..entry("cal-main", "Calendar", "My Calendars") },
        RemoteCalendar { color: Some(Rgb(4, 5, 6)), ..entry("cal-team", "Team", "My Calendars") },
        RemoteCalendar { rights: mailcraft_model::Rights::Read, ..entry("cal-holidays", "Holidays\r\nin Wonderland", "Other Calendars") },
        // Said twice, or with no name the server could call it by: passed over.
        entry("cal-team", "Team again", ""),
        entry("two words", "Odd", ""),
    ];
    let merged = super::apply_calendars(&mut s, &list(all.clone(), true)).unwrap();
    assert_eq!((merged["added"].as_u64(), merged["updated"].as_u64(), merged["removed"].as_u64()), (Some(2), Some(1), Some(0)));
    let of_account: Vec<_> = s.mailbox.calendars.iter().filter(|c| c.origin.account == account).cloned().collect();
    let shape: Vec<_> = of_account.iter().map(|c| (c.name.as_str(), c.origin.id.as_str(), c.group.as_str(), !c.can_edit(), c.visible)).collect();
    assert_eq!(
        shape,
        // Only the account's own calendar is ticked to begin with: the rest are listed, and
        // wait for the owner to tick them.
        [
            ("Calendar", "", "My Calendars", false, true),
            ("Team", "cal-team", "My Calendars", false, false),
            ("Holidays  in Wonderland", "cal-holidays", "Other Calendars", true, false)
        ]
    );
    // The main one is the calendar events came into before there was a list; colours are the server's.
    assert_eq!((of_account[0].id, of_account[0].color, of_account[1].color), (main, Rgb(1, 2, 3), Rgb(4, 5, 6)));
    assert_eq!(super::apply_calendars(&mut s, &list(all.clone(), true)).unwrap()["updated"], 0);
    let (team, holidays) = (of_account[1].id, of_account[2].id);
    // What the owner ticks and unticks stays as it is, whatever list comes later; and a
    // calendar that appears in a later list is unticked like the others were.
    let shown = |s: &Session| {
        s.mailbox.calendars.iter().filter(|c| c.origin.account == account).map(|c| (c.origin.id.clone(), c.visible)).collect::<Vec<_>>()
    };
    run(&mut s, "calendar.showCalendar", json!({"id": team, "value": true}));
    run(&mut s, "calendar.showCalendar", json!({"id": main, "value": false}));
    let mut later = all.clone();
    later.push(RemoteCalendar { color: Some(Rgb(7, 8, 9)), ..entry("cal-birthdays", "Birthdays", "Other Calendars") });
    assert_eq!(super::apply_calendars(&mut s, &list(later, false)).unwrap()["added"], 1);
    let ticks = [(String::new(), false), ("cal-team".to_string(), true), ("cal-holidays".to_string(), false), ("cal-birthdays".to_string(), false)];
    assert_eq!(shown(&s), ticks);
    // (A list with other names and colours for them changes those, and still not the ticks.)
    let renamed: Vec<RemoteCalendar> = all.iter().map(|c| RemoteCalendar { color: Some(Rgb(9, 9, 9)), ..c.clone() }).collect();
    assert!(super::apply_calendars(&mut s, &list(renamed, false)).unwrap()["updated"].as_u64() > Some(0));
    assert_eq!(shown(&s), ticks);
    super::apply_calendars(&mut s, &list(all.clone(), true)).unwrap();
    assert_eq!(shown(&s), ticks[..3]);
    run(&mut s, "calendar.showCalendar", json!({"id": main, "value": true}));
    // An account with one calendar has that one, ticked.
    let mut alone = Session::at(mailcraft_model::Mailbox::with_local_account("Bo", "bo@example.org"), at(2026, 10, 14, 16, 0), 0);
    let only = alone.mailbox.accounts.first().map(|a| a.id).unwrap();
    let one = CalendarsDownload {
        account: only,
        email: "bo@example.org".into(),
        complete: true,
        calendars: vec![RemoteCalendar { main: true, ..entry("cal-only", "Calendar", "") }],
        ..Default::default()
    };
    assert_eq!(super::apply_calendars(&mut alone, &one).unwrap()["added"], 1);
    assert_eq!(alone.mailbox.calendars.iter().filter(|c| c.origin.account == only).map(|c| c.visible).collect::<Vec<_>>(), [true]);

    // Each calendar's events come into it, and a new event goes to the calendar it is made in.
    let offsite = EventsDownload { calendar: "cal-team".into(), ..october(account, vec![remote("t-1", "Offsite", at(2026, 10, 20, 13, 0), 60)]) };
    apply_events(&mut s, &offsite).unwrap();
    let closed =
        EventsDownload { calendar: "cal-holidays".into(), ..october(account, vec![remote("h-1", "Bank holiday", at(2026, 10, 26, 0, 0), 60)]) };
    apply_events(&mut s, &closed).unwrap();
    assert_eq!((event_called(&s, account, "t-1").calendar, event_called(&s, account, "h-1").calendar), (team, holidays));
    assert_eq!(s.mailbox.event(first).map(|e| e.calendar), Some(main), "the main calendar's events stay where they are");
    let made = run(&mut s, "calendar.new", json!({"title": "Retro", "start": "2026-10-21T10:00", "calendar": team}))["id"].as_u64().unwrap();
    let told = ops(&s, account);
    assert_eq!((told[0].kind, told[0].event, told[0].calendar.as_str()), (EventChangeKind::Create, made, "cal-team"));
    run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-21T12:00"}));
    assert_eq!(ops(&s, account)[1].calendar, "");
    push_done(&mut s, account);

    // A calendar the account may only read refuses every change, in plain words.
    let read_only = "The calendar \"Holidays  in Wonderland\" can only be read: its owner hasn't let this account change it.";
    let holiday = event_called(&s, account, "h-1").id;
    for (command, params) in [
        ("calendar.new", json!({"title": "Mine", "start": "2026-10-26T10:00", "calendar": holidays})),
        ("calendar.move", json!({"id": holiday, "start": "2026-10-27T00:00"})),
        ("calendar.edit", json!({"id": holiday, "title": "Mine"})),
        ("calendar.delete", json!({"id": holiday})),
        ("calendar.categorize", json!({"id": holiday, "category": "Red category"})),
        ("calendar.showAs", json!({"id": holiday, "value": "free"})),
    ] {
        assert_eq!(refused(&mut s, command, params.clone()), read_only, "{command} {params}");
    }
    assert!(s.mailbox.event_changes.is_empty());

    // The series a calendar should have in a stretch are asked about by name, calendar by calendar.
    let gym = run(
        &mut s,
        "calendar.new",
        json!({"title": "Gym", "start": "2026-10-12T18:00", "recurrence": {"freq": "weekly", "count": 3}, "calendar": team}),
    );
    push_done(&mut s, account);
    assert_eq!(s.mailbox.event(gym["id"].as_u64().unwrap()).map(|e| e.origin.id.clone()), Some("made-0".to_string()));
    let expected = |s: &Session, calendar: &str, from: Timestamp, to: Timestamp| super::expected_series(s, account, calendar, from, to);
    assert_eq!(expected(&s, "cal-team", at(2026, 10, 1, 0, 0), at(2026, 11, 1, 0, 0)), ["made-0"]);
    assert!(expected(&s, "", at(2026, 10, 1, 0, 0), at(2026, 11, 1, 0, 0)).is_empty());
    assert!(expected(&s, "cal-team", at(2026, 11, 1, 0, 0), at(2026, 12, 1, 0, 0)).is_empty(), "three times, and over by November");

    // A list that isn't all there is takes nothing away; one that is takes the calendar that is
    // no longer listed, with its events. Never the main one, and never one of the mailbox's own.
    let fewer: Vec<RemoteCalendar> = all.iter().filter(|c| c.id == "cal-holidays" || c.id == "cal-main").cloned().collect();
    assert_eq!(super::apply_calendars(&mut s, &list(fewer.clone(), false)).unwrap()["removed"], 0);
    // Nor does a list that doesn't say which calendar is the main one (it couldn't be told from
    // the calendar that is here already).
    let headless: Vec<RemoteCalendar> = all.iter().filter(|c| c.id == "cal-holidays").cloned().collect();
    assert_eq!(super::apply_calendars(&mut s, &list(headless, true)).unwrap(), json!({"added": 0, "updated": 0, "removed": 0, "complete": false}));
    assert!(s.mailbox.calendar(team).is_some());
    let own_calendars = s.mailbox.calendars.iter().filter(|c| c.origin.is_local()).count();
    assert_eq!(super::apply_calendars(&mut s, &list(fewer, true)).unwrap()["removed"], 1);
    assert!(s.mailbox.calendar(team).is_none() && s.mailbox.calendar(main).is_some() && s.mailbox.calendar(holidays).is_some());
    assert!(s.mailbox.events.values().all(|e| e.calendar != team));
    assert_eq!(s.mailbox.calendars.iter().filter(|c| c.origin.is_local()).count(), own_calendars);
    assert!(s.mailbox.clone().repair().is_empty());
    // For another account, or from nobody: refused.
    assert!(super::apply_calendars(&mut s, &CalendarsDownload { email: "eve@example.org".into(), ..list(Vec::new(), true) }).is_err());

    // The mailbox's categories as the server has them: its colours, its spelling, and its own
    // names beside those that were here.
    let categories = CategoriesDownload {
        account,
        email: "ada@example.org".into(),
        categories: vec![
            mailcraft_model::Category { name: "RED Category".into(), color: Rgb(9, 9, 9) },
            mailcraft_model::Category { name: "Project X\r\n".into(), color: Rgb(7, 7, 7) },
            mailcraft_model::Category { name: " ".into(), color: Rgb(1, 1, 1) },
        ],
    };
    assert_eq!(super::apply_categories(&mut s, &categories).unwrap()["changed"], 2);
    assert_eq!(s.mailbox.category("red category").map(|c| (c.name.as_str(), c.color)), Some(("RED Category", Rgb(9, 9, 9))));
    assert_eq!(s.mailbox.category("Project X").map(|c| c.color), Some(Rgb(7, 7, 7)));
    assert!(s.mailbox.category("Blue category").is_some());
    assert_eq!(super::apply_categories(&mut s, &categories).unwrap()["changed"], 0);
}

#[test]
fn categories_travel_with_an_event_both_ways() {
    let (mut s, account, first, _, _) = served();
    // From the server, when the download says what categories the events have.
    let with = |categories: &[&str], key: &str| EventsDownload {
        has_categories: true,
        ..october(
            account,
            vec![
                RemoteEvent {
                    categories: categories.iter().map(|c| c.to_string()).collect(),
                    change_key: key.into(),
                    ..remote("e-1", "Budget review", at(2026, 10, 12, 13, 0), 60)
                },
                remote("e-2", "Dentist", at(2026, 10, 13, 18, 0), 30),
            ],
        )
    };
    apply_events(&mut s, &with(&["Team"], "ck-2")).unwrap();
    assert_eq!(s.mailbox.event(first).unwrap().categories, ["Team"]);
    // To the server, when one is given or taken here.
    run(&mut s, "calendar.categorize", json!({"id": first, "category": "Red category"}));
    assert_eq!(ops(&s, account)[0].fields, EventFields { categories: Some(vec!["Team".into(), "Red category".into()]), ..Default::default() });
    push_done(&mut s, account);
    // Taken away on the server (its copy has changed since): taken away here.
    apply_events(&mut s, &with(&[], "ck-3")).unwrap();
    assert!(s.mailbox.event(first).unwrap().categories.is_empty() && s.mailbox.event_changes.is_empty());
    // A category from when they were kept here only (the server's copy hasn't changed since,
    // and has none): it stays, and goes up.
    s.mailbox.event_mut(first).unwrap().categories = vec!["Blue category".into()];
    apply_events(&mut s, &with(&[], "ck-3")).unwrap();
    assert_eq!(s.mailbox.event(first).unwrap().categories, ["Blue category"]);
    let told = ops(&s, account);
    assert_eq!((told.len(), told[0].fields.categories.clone()), (1, Some(vec!["Blue category".to_string()])));
    push_done(&mut s, account);
    assert!(s.mailbox.event_changes.is_empty());
}

#[test]
fn free_and_busy_times_are_asked_for_and_merged() {
    use mailcraft_model::remote::BusyTime;
    let (mut s, _) = session();
    let asked = json!({"people": "Bo <bo@example.org>; cy@example.org; BO@example.org", "from": "2026-10-15T08:00", "to": "2026-10-15T18:00"});
    // What a host is to ask: who, and the stretch as instants (eight in the morning here is noon UTC).
    let ask = run(&mut s, "calendar.freeBusy", asked.clone());
    assert_eq!(ask, json!({"ask": {"people": ["bo@example.org", "cy@example.org"], "from": at(2026, 10, 15, 12, 0), "to": at(2026, 10, 15, 22, 0)}}));
    // What it brought back, merged: the strongest shows where things overlap, what touches and
    // shows alike is one stretch, free time is left out, and nothing reaches beyond the stretch.
    let time = |start: (u8, u8), end: (u8, u8), show_as: &str| json!({"start": at(2026, 10, 15, start.0, start.1), "end": at(2026, 10, 15, end.0, end.1), "showAs": show_as});
    let result = json!({
        "from": 0, "to": 0,
        "people": [
            {"email": "BO@example.org", "busy": [
                time((13, 0), (14, 0), "busy"), time((13, 30), (15, 0), "tentative"), time((14, 0), (14, 30), "busy"),
                time((16, 0), (17, 0), "free"), time((10, 0), (12, 30), "outOfOffice"), time((21, 0), (23, 0), "busy"), time((18, 0), (18, 0), "busy"),
            ]},
            {"email": "eve@example.org", "busy": [time((13, 0), (14, 0), "busy")]},
        ],
    });
    let mut with = asked.clone();
    with["result"] = result;
    let answer = run(&mut s, "calendar.freeBusy", with);
    assert_eq!((answer["from"].as_str(), answer["to"].as_str()), (Some("2026-10-15T08:00:00"), Some("2026-10-15T18:00:00")));
    assert_eq!(
        answer["people"],
        json!([
            {"email": "bo@example.org", "busy": [
                {"start": "2026-10-15T08:00:00", "end": "2026-10-15T08:30:00", "showAs": "outOfOffice"},
                {"start": "2026-10-15T09:00:00", "end": "2026-10-15T10:30:00", "showAs": "busy"},
                {"start": "2026-10-15T10:30:00", "end": "2026-10-15T11:00:00", "showAs": "tentative"},
                {"start": "2026-10-15T17:00:00", "end": "2026-10-15T18:00:00", "showAs": "busy"},
            ]},
            // Nothing came back about Cy; and Eve, whom nobody asked about, is not in the answer.
            {"email": "cy@example.org", "busy": [], "error": "the server said nothing about them"},
        ])
    );
    // Stretches as such.
    let t = |start: i64, end: i64, show_as: ShowAs| BusyTime { start, end, show_as };
    let merged = crate::cmd::calendar::merge_busy(
        &[t(10, 20, ShowAs::Busy), t(20, 30, ShowAs::Busy), t(25, 40, ShowAs::OutOfOffice), t(50, 40, ShowAs::Busy)],
        0,
        35,
    );
    assert_eq!(merged, [t(10, 25, ShowAs::Busy), t(25, 35, ShowAs::OutOfOffice)]);
    // What can't be asked.
    for (params, why) in [
        (json!({"people": [], "from": "2026-10-15T08:00", "to": "2026-10-15T18:00"}), "`people` names nobody with an e-mail address"),
        (
            json!({"people": ["bo@example.org"], "from": "2026-10-15T08:00", "to": "2026-10-15T08:00"}),
            "`to` is after `from`, and no more than 62 days after",
        ),
        (
            json!({"people": ["bo@example.org"], "from": "2026-10-15T08:00", "to": "2027-10-15T08:00"}),
            "`to` is after `from`, and no more than 62 days after",
        ),
        (json!({"people": ["bo@example.org"], "to": "2026-10-15T08:00"}), "`from` is required"),
    ] {
        assert_eq!(s.run("calendar.freeBusy", &params), Err(CmdError::Params(why.into())), "{params}");
    }
    let crowd: Vec<String> = (0..21).map(|n| format!("p{n}@example.org")).collect();
    assert!(s.run("calendar.freeBusy", &json!({"people": crowd, "from": "2026-10-15T08:00", "to": "2026-10-15T18:00"})).is_err());
}

/// The two events of [`served`] as the server has them, the first at `start` under the mark `key`.
fn both(start: Timestamp, key: &str) -> Vec<RemoteEvent> {
    vec![RemoteEvent { change_key: key.into(), ..remote("e-1", "Budget review", start, 60) }, remote("e-2", "Dentist", at(2026, 10, 13, 18, 0), 30)]
}

/// What a merge says of the changes that were sent: seen on the server, put back, still to be seen.
fn checked(merged: &Value) -> (u64, u64, u64) {
    let n = |name: &str| merged[name].as_u64().unwrap();
    (n("verified"), n("putBack"), n("unverified"))
}

fn said(merged: &Value) -> Vec<String> {
    merged["messages"].as_array().unwrap().iter().map(|line| line.as_str().unwrap().to_string()).collect()
}

#[test]
fn a_yes_from_the_server_is_checked_against_what_comes_down() {
    let (mut s, account, first, _, _) = served();
    let (monday, wednesday) = (at(2026, 10, 12, 13, 0), at(2026, 10, 14, 13, 0));
    // Moved to Wednesday, and the server says it took the change: that is noted, to be seen.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    let applied = push_done(&mut s, account);
    assert_eq!((applied.done, applied.unverified, s.mailbox.event_checks.len()), (1, 1, 1));
    assert!(super::recheck_due(&s, account) && s.mailbox.event_changes.is_empty());
    // A download that was answered from before the change (the old time, under the old mark):
    // the event stays where the owner put it, and the change is still to be seen.
    let merged = apply_events(&mut s, &october(account, both(monday, "ck-e-1-1"))).unwrap();
    assert_eq!((checked(&merged), counts(&merged)), ((0, 0, 1), (0, 0, 0, 2, 0)));
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 14, 9, 0)));
    assert_eq!(s.mailbox.event_checks[0].tries, 1);
    // The server has caught up: the change is there, and nothing is left to look for.
    let merged = apply_events(&mut s, &october(account, both(wednesday, "ck-new"))).unwrap();
    assert_eq!((checked(&merged), said(&merged).len()), ((1, 0, 0), 0));
    assert!(s.mailbox.event_checks.is_empty() && !super::recheck_due(&s, account));
    assert_eq!(s.mailbox.event(first).map(|e| (e.start, e.origin.change_key.clone())), Some((clock(2026, 10, 14, 9, 0), "ck-new".to_string())));

    // A server that says yes and never shows it: after three looks the event is put back to
    // what the server has, and the owner is told which one and what of it.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-15T09:00"}));
    push_done(&mut s, account);
    for look in 1..=2 {
        let merged = apply_events(&mut s, &october(account, both(wednesday, "ck-older"))).unwrap();
        assert_eq!((checked(&merged), s.mailbox.event(first).map(|e| e.start)), ((0, 0, 1), Some(clock(2026, 10, 15, 9, 0))), "look {look}");
    }
    let merged = apply_events(&mut s, &october(account, both(wednesday, "ck-older"))).unwrap();
    assert_eq!(checked(&merged), (0, 1, 0));
    assert_eq!(
        said(&merged),
        [
            "\"Budget review\" was changed here and the server said it took the change, but it still has its time as before. It was put back to what the server has."
        ]
    );
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 14, 9, 0)));
    assert!(s.mailbox.event_checks.is_empty() && s.mailbox.event_changes.is_empty());

    // A server whose own version of the change is not what was sent (it is under the mark the
    // change was answered with, and at another time): that needs no second look.
    run(&mut s, "calendar.edit", json!({"id": first, "start": "2026-10-16T09:00", "title": "Budget review, final"}));
    push_done(&mut s, account);
    let theirs = vec![RemoteEvent { change_key: "ck-new".into(), ..remote("e-1", "Budget review, final", wednesday, 60) }];
    let merged = apply_events(&mut s, &october(account, theirs)).unwrap();
    assert_eq!(checked(&merged), (0, 1, 0));
    assert_eq!(
        said(&merged),
        [
            "The server took the change to \"Budget review, final\" but has its time otherwise than it was sent. It is shown here as the server has it."
        ]
    );
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 14, 9, 0)));

    // What a download can't show again as it was sent (a description, a category) is not waited
    // for: there is nothing to hold it against.
    run(&mut s, "calendar.edit", json!({"id": first, "description": "Bring the numbers."}));
    assert_eq!(push_done(&mut s, account).unverified, 0);
    // A change that is waited for and never comes into any download is let go after a day.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2027-03-01T09:00"}));
    push_done(&mut s, account);
    let merged = apply_events(&mut s, &october(account, Vec::new())).unwrap();
    assert_eq!((checked(&merged), s.mailbox.event(first).is_some()), ((0, 0, 1), true));
    s.clock.now = Some(s.now() + 90_000);
    assert_eq!(checked(&apply_events(&mut s, &october(account, Vec::new())).unwrap()), (0, 0, 0));
    assert!(s.mailbox.clone().repair().is_empty());
}

#[test]
fn what_was_made_or_deleted_is_looked_for_on_the_server_too() {
    let (mut s, account, first, second, server) = served();
    let monday = at(2026, 10, 12, 13, 0);
    // A new event that the server said it made, and that the next download doesn't have yet:
    // it is not taken for one the server lost.
    let lunch = run(&mut s, "calendar.new", json!({"title": "Lunch", "start": "2026-10-15T12:00"}))["id"].as_u64().unwrap();
    push_done(&mut s, account);
    let merged = apply_events(&mut s, &october(account, both(monday, "ck-e-1-1"))).unwrap();
    assert_eq!((checked(&merged), merged["removed"].as_u64()), ((0, 0, 1), Some(0)));
    assert_eq!(s.mailbox.event(lunch).map(|e| e.origin.id.as_str()), Some("made-0"));
    let mut with_lunch = both(monday, "ck-e-1-1");
    with_lunch.push(RemoteEvent { reminder_minutes: Some(15), ..remote("made-0", "Lunch", at(2026, 10, 15, 16, 0), 30) });
    assert_eq!(checked(&apply_events(&mut s, &october(account, with_lunch.clone())).unwrap()), (1, 0, 0));

    // One the server said it made and never shows: after three looks it is kept on this
    // computer, where the owner still has it, and says that it is not in the Outlook calendar.
    let ghost = run(&mut s, "calendar.new", json!({"title": "Ghost", "start": "2026-10-16T12:00"}))["id"].as_u64().unwrap();
    let mut answer = all_done(&event_upload(&s, account).unwrap());
    answer.outcomes[0].remote = "made-9".into();
    apply_event_outcomes(&mut s, &answer).unwrap();
    for _ in 0..2 {
        assert_eq!(checked(&apply_events(&mut s, &october(account, with_lunch.clone())).unwrap()), (0, 0, 1));
    }
    let merged = apply_events(&mut s, &october(account, with_lunch.clone())).unwrap();
    assert_eq!(checked(&merged), (0, 1, 0));
    assert_eq!(
        said(&merged),
        [
            "\"Ghost\" was made here and the server said it took it, but it is not in the server's calendar. It is kept on this computer only: it is not in your Outlook calendar."
        ]
    );
    let kept = s.mailbox.event(ghost).unwrap();
    assert!(kept.origin.is_local() && kept.calendar != server && super::stays_here(&s.mailbox, ghost));

    // A deletion the server said it made, in a download that still has the event: it is not
    // brought back for that; when it is gone there too, the deletion has arrived.
    run(&mut s, "calendar.delete", json!({"id": second}));
    push_done(&mut s, account);
    let merged = apply_events(&mut s, &october(account, with_lunch.clone())).unwrap();
    assert_eq!((checked(&merged), merged["added"].as_u64()), ((0, 0, 1), Some(0)));
    assert!(!titles(&s, account).contains(&"Dentist".to_string()));
    let without: Vec<RemoteEvent> = with_lunch.iter().filter(|e| e.id != "e-2").cloned().collect();
    assert_eq!(checked(&apply_events(&mut s, &october(account, without.clone())).unwrap()), (1, 0, 0));
    // And one that the server keeps all the same comes back, with a word about it.
    run(&mut s, "calendar.delete", json!({"id": first}));
    push_done(&mut s, account);
    for _ in 0..2 {
        assert_eq!(checked(&apply_events(&mut s, &october(account, without.clone())).unwrap()), (0, 0, 1));
        assert!(!titles(&s, account).contains(&"Budget review".to_string()));
    }
    let merged = apply_events(&mut s, &october(account, without)).unwrap();
    assert_eq!((checked(&merged), merged["added"].as_u64()), ((0, 1, 0), Some(1)));
    assert_eq!(
        said(&merged),
        ["\"Budget review\" was deleted here and the server said it deleted it too, but it still has it. It is back in the calendar."]
    );
    assert!(titles(&s, account).contains(&"Budget review".to_string()));
    assert!(s.mailbox.event_checks.is_empty() && s.mailbox.clone().repair().is_empty());
    // What is noted of sent changes is kept with the mailbox, and a mailbox without any says nothing of it.
    run(&mut s, "calendar.move", json!({"id": lunch, "start": "2026-10-15T13:00"}));
    push_done(&mut s, account);
    let json = serde_json::to_string(&s.mailbox).unwrap();
    assert!(json.contains(r#""eventChecks":[{"account":"#), "{:.200}", json);
    assert_eq!(serde_json::from_str::<mailcraft_model::Mailbox>(&json).unwrap().event_checks, s.mailbox.event_checks);
    s.mailbox.event_checks.clear();
    assert!(!serde_json::to_string(&s.mailbox).unwrap().contains("eventChecks"));
}

#[test]
fn a_change_that_cannot_be_sent_is_given_up_with_a_word_and_put_back() {
    let unasked = "it would have made the server write to the people invited, and nobody said to send that";
    let reason = "it would make the server write to the people invited, and nobody said to send that";
    // A change to a meeting the owner called that waits without the owner's yes (only a file
    // from elsewhere holds such a thing): it is not sent, and it doesn't just go either.
    let (mut s, account, own, _) = meetings();
    run(&mut s, "calendar.move", json!({"id": own, "start": "2026-10-12T10:00", "send": true}));
    s.mailbox.event_changes[0].send = false;
    let answer = run(&mut s, "sync.eventChanges", json!({"account": account}));
    assert_eq!((answer["ops"].as_array().map(Vec::len), answer["dropped"]["count"].as_u64()), (Some(0), Some(1)));
    assert_eq!(answer["dropped"]["reasons"], json!([format!("Update: {reason}")]));
    assert_eq!(
        answer["dropped"]["messages"],
        json!([format!("The change to \"Planning\" was not sent: {unasked}. It was put back to what the server has.")])
    );
    // Back where the server has it, with nothing left waiting; asked again, nothing more is said.
    assert_eq!(s.mailbox.event(own).map(|e| e.start), Some(clock(2026, 10, 12, 9, 0)));
    assert!(s.mailbox.event_changes.is_empty());
    assert_eq!(run(&mut s, "sync.eventChanges", json!({"account": account}))["dropped"]["count"], 0);

    // A deletion of the kind: the meeting is still on the server, and the owner hears that it comes back.
    run(&mut s, "calendar.delete", json!({"id": own, "send": true}));
    s.mailbox.event_changes[0].send = false;
    let dropped = super::settle_changes(&mut s);
    assert_eq!(dropped.iter().map(|d| (d.account, d.kind, d.reason)).collect::<Vec<_>>(), [(account, EventChangeKind::Delete, reason)]);
    assert_eq!(
        dropped[0].message,
        format!("Deleting \"Planning\" was not sent to the server: {unasked}. It is still there, and comes back into the calendar.")
    );
    // A new meeting of the kind: kept on this computer, where nobody is invited to anything.
    let made = run(&mut s, "calendar.newMeeting", json!({"title": "Sync", "start": "2026-10-15T10:00", "attendees": "bo@example.org", "send": true}));
    let made = made["id"].as_u64().unwrap();
    s.mailbox.event_changes[0].send = false;
    let merged = apply_events(&mut s, &october(account, Vec::new())).unwrap();
    assert_eq!(merged["dropped"]["count"], 1);
    assert_eq!(said(&merged), [format!("\"Sync\" was not sent to the server: {unasked}. It is kept on this computer only.")]);
    assert!(s.mailbox.event(made).is_some_and(|e| e.origin.is_local()) && s.mailbox.event_changes.is_empty());

    // A note that says nothing any more (the event is as the server has it) just goes: there
    // was nothing to send, and it must not keep the event from following the server.
    let (mut s, account, first, _, _) = served();
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    let base = s.mailbox.event_changes[0].base.clone().unwrap();
    *s.mailbox.event_mut(first).unwrap() = *base;
    assert!(ops(&s, account).is_empty());
    let merged = apply_events(&mut s, &october(account, both(at(2026, 10, 13, 13, 0), "ck-2"))).unwrap();
    assert_eq!((merged["dropped"]["count"].as_u64(), s.mailbox.event_changes.len()), (Some(0), 0));
    // (The next download moves it as the server has it.)
    apply_events(&mut s, &october(account, both(at(2026, 10, 13, 13, 0), "ck-2"))).unwrap();
    assert_eq!(s.mailbox.event(first).map(|e| e.start), Some(clock(2026, 10, 13, 9, 0)));
}

#[test]
fn an_event_of_another_calendar_is_sent_through_that_calendar() {
    use mailcraft_model::remote::{CalendarsDownload, RemoteCalendar};
    let (mut s, account, first, _, _) = served();
    let entry = |id: &str, name: &str, main: bool| RemoteCalendar { id: id.into(), name: name.into(), main, ..Default::default() };
    let list = CalendarsDownload {
        account,
        email: "ada@example.org".into(),
        complete: true,
        calendars: vec![entry("cal-main", "Calendar", true), entry("cal-team", "Team", false)],
        ..Default::default()
    };
    super::apply_calendars(&mut s, &list).unwrap();
    let in_team = EventsDownload { calendar: "cal-team".into(), ..october(account, vec![remote("t-1", "Retro", at(2026, 10, 15, 13, 0), 60)]) };
    apply_events(&mut s, &in_team).unwrap();
    let retro = event_called(&s, account, "t-1").id;
    let team = s.mailbox.event(retro).unwrap().calendar;
    // A change, a new event and a deletion there all say which calendar of the server it is.
    run(&mut s, "calendar.move", json!({"id": retro, "start": "2026-10-15T10:00"}));
    run(&mut s, "calendar.new", json!({"title": "Planning poker", "start": "2026-10-16T10:00", "calendar": team}));
    let told: Vec<(EventChangeKind, String)> = ops(&s, account).into_iter().map(|op| (op.kind, op.calendar)).collect();
    assert_eq!(told, [(EventChangeKind::Update, "cal-team".to_string()), (EventChangeKind::Create, "cal-team".to_string())]);
    push_done(&mut s, account);
    run(&mut s, "calendar.delete", json!({"id": retro}));
    assert_eq!(
        ops(&s, account).into_iter().map(|op| (op.kind, op.calendar)).collect::<Vec<_>>(),
        [(EventChangeKind::Delete, "cal-team".to_string())]
    );
    // One of the main calendar says nothing: it is the account's own.
    run(&mut s, "calendar.move", json!({"id": first, "start": "2026-10-14T09:00"}));
    assert!(ops(&s, account).iter().any(|op| op.remote == "e-1" && op.calendar.is_empty()));
}

#[test]
fn what_is_only_on_this_computer_says_so_and_can_be_moved_to_the_account() {
    // The sample's own events, beside a server calendar that has two.
    let (mut s, account, first, _, server) = served();
    let local = s.mailbox.calendars.iter().find(|c| c.origin.is_local()).map(|c| (c.id, c.name.clone())).unwrap();
    assert_eq!(local.1, "On this computer");
    let here: Vec<Event> = own(&s).0;
    let meetings = here.iter().filter(|e| !e.attendees.is_empty()).count();
    assert!(meetings > 0 && here.len() > meetings, "the sample has appointments and meetings of its own");
    assert_eq!(super::only_here(&s.mailbox), [(local.0, here.len())]);
    assert!(super::stays_here(&s.mailbox, here[0].id) && !super::stays_here(&s.mailbox, first));
    // A copy of something the server has already: the same title at the same time.
    let twin =
        run(&mut s, "calendar.new", json!({"title": "Budget review", "start": "2026-10-12T09:00", "end": "2026-10-12T10:00", "calendar": local.0}));
    let twin = twin["id"].as_u64().unwrap();
    assert!(s.mailbox.event_changes.is_empty());

    // Meetings are among them, and nothing was said of invitations: nothing moves.
    let why = refused(&mut s, "calendar.moveToAccount", json!({}));
    assert!(why.contains("with people invited") && why.contains("Nothing was moved."), "{why}");
    assert_eq!(own(&s).0.len(), here.len() + 1);
    // Without invitations: the appointments move and are made on the server, the copy of what
    // is there already just goes, and the meetings stay where they are and say why.
    let moved = run(&mut s, "calendar.moveToAccount", json!({"send": false}));
    let appointments = here.len() - meetings;
    assert_eq!((moved["moved"].as_u64(), moved["already"].as_u64(), moved["calendar"].as_u64()), (Some(appointments as u64), Some(1), Some(server)));
    assert_eq!(moved["kept"].as_array().map(Vec::len), Some(meetings));
    assert_eq!(moved["kept"][0]["why"], "People are invited to it, and no invitations were to be sent.");
    assert!(s.mailbox.event(twin).is_none());
    let told = ops(&s, account);
    assert_eq!((told.len(), told.iter().all(|op| op.kind == EventChangeKind::Create && op.fields.attendees.is_none())), (appointments, true));
    assert_eq!(super::only_here(&s.mailbox), [(local.0, meetings)]);
    // One step, to be taken back like any other.
    assert!(s.undo());
    assert_eq!((own(&s).0.len(), s.mailbox.event_changes.len()), (here.len() + 1, 0));

    // With the owner's yes to the invitations, everything goes, and the calendar that is left
    // empty goes away.
    let moved = run(&mut s, "calendar.moveToAccount", json!({"send": true}));
    assert_eq!(
        (moved["moved"].as_u64(), moved["kept"].as_array().map(Vec::len), moved["removed"].as_u64()),
        (Some(here.len() as u64), Some(0), Some(1))
    );
    assert!(own(&s).0.is_empty() && super::only_here(&s.mailbox).is_empty());
    assert!(s.mailbox.calendar(local.0).is_none());
    let told = ops(&s, account);
    assert_eq!(told.iter().filter(|op| op.fields.attendees.as_ref().is_some_and(|people| !people.is_empty())).count(), meetings);
    assert!(s.mailbox.event_changes.iter().all(|change| change.kind == EventChangeKind::Create));
    assert!(s.mailbox.clone().repair().is_empty());
    // With no server calendar there is nowhere to move anything to, and nothing to say.
    let mut alone = Session::at(crate::sample::mailbox(at(2026, 10, 14, 16, 0), 0), at(2026, 10, 14, 16, 0), 0);
    assert!(super::only_here(&alone.mailbox).is_empty());
    assert!(refused(&mut alone, "calendar.moveToAccount", json!({})).starts_with("No account's server keeps a calendar here yet"));
}
