// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox's calendars, read through Graph (#70): at `/users/{address}/…`, never at `/me`,
//! held against the stand-in ([`standin`]). Nothing here leaves this machine.

use mailcraft_time::Date;

use super::standin::{CalendarAccess, StandIn, TOKEN};
use super::*;

const SHARED: &str = "support@example.org";

fn at(year: i32, month: u8, day: u8, hour: u8) -> Timestamp {
    DateTime::new(Date::new(year, month, day).unwrap(), hour, 0, 0).unwrap().to_timestamp(0)
}

fn query(shared: &str, calendar: &str) -> EventsQuery {
    EventsQuery { calendar: calendar.into(), from: at(2026, 10, 1, 0), to: at(2026, 11, 1, 0), series: Vec::new(), shared: shared.into() }
}

/// What the stand-in was asked, as the pieces of each address after the version.
fn asked(server: &StandIn) -> Vec<String> {
    server.state().requests.iter().map(|asked| format!("{} {}", asked.method, asked.path.join("/"))).collect()
}

/// A stand-in with a shared mailbox whose calendars are open to the owner: a main one with a
/// meeting in it and a team one with another.
fn served() -> (StandIn, String) {
    let server = StandIn::start().unwrap();
    let team = {
        let mut state = server.state();
        state.add_other(SHARED, true);
        let main = state.add_other_calendar(SHARED, "Calendar", true);
        let team = state.add_other_calendar(SHARED, "Team", false);
        state.add_other_event(SHARED, &main, "Stand-up", at(2026, 10, 14, 9), "busy");
        state.add_other_event(SHARED, &team, "Rota", at(2026, 10, 15, 9), "busy");
        state.share_calendars(SHARED, CalendarAccess::Open);
        // The owner's own calendar has something in it too, which must never be mistaken for it.
        let own = state.main_calendar();
        state.add_event(&own, "Dentist", at(2026, 10, 14, 15));
        team
    };
    (server, team)
}

#[test]
fn a_shared_mailbox_lists_its_calendars_and_reads_them_without_touching_me() {
    let (server, team) = served();
    let list = calendars_at(&server.url(), TOKEN, Some(SHARED)).unwrap();
    let names: Vec<(&str, bool, Rights, Option<&str>)> =
        list.calendars.iter().map(|c| (c.name.as_str(), c.main, c.rights, c.owner.as_ref().map(|o| o.address.as_str()))).collect();
    assert_eq!(
        names,
        vec![("Calendar", true, Rights::Edit, Some(SHARED)), ("Team", false, Rights::Read, Some(SHARED))],
        "its own list, with who it belongs to and what the owner may do"
    );
    assert!(list.complete && list.calendars.iter().all(|c| c.group.is_empty()));

    let main = events_in_at(&server.url(), TOKEN, &query(SHARED, "")).unwrap();
    let titles: Vec<&str> = main.events.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, ["Stand-up"], "not the owner's Dentist");
    assert!(main.complete && !main.free_busy);
    let rota = events_in_at(&server.url(), TOKEN, &query(SHARED, &team)).unwrap();
    assert_eq!(rota.events.iter().map(|e| e.title.as_str()).collect::<Vec<_>>(), ["Rota"]);

    // Everything went to the mailbox's own address; not one request touched `/me`.
    let seen = asked(&server);
    assert!(!seen.is_empty() && seen.iter().all(|line| line.contains("users/support@example.org")), "{seen:?}");
    assert!(seen.iter().all(|line| !line.contains("/me/") && !line.ends_with("/me")), "{seen:?}");
}

#[test]
fn a_shared_mailbox_series_are_asked_for_in_its_own_mailbox() {
    let (server, _) = served();
    let series = {
        let mut state = server.state();
        let main = state.other(SHARED).and_then(|o| o.calendars.iter().find(|c| c.main).map(|c| c.id.clone())).unwrap();
        state.add_other_event(SHARED, &main, "Weekly", at(2026, 10, 5, 8), "busy")
    };
    let mut ask = query(SHARED, "");
    ask.series = vec![series];
    events_in_at(&server.url(), TOKEN, &ask).unwrap();
    let seen = asked(&server);
    assert!(seen.iter().any(|line| line.contains("$batch")), "{seen:?}");
    let batched: Vec<String> =
        server.state().requests.iter().filter(|asked| asked.path.last().is_some_and(|p| p == "$batch")).map(|asked| asked.body.clone()).collect();
    assert!(batched.iter().all(|body| body.contains("/users/support@example.org/events/") && !body.contains("\"url\":\"/me/")), "{batched:?}");
    // The batch is answered for the mailbox: the owner's own events aren't in it. (Nothing at /me.)
    assert!(server.state().requests.iter().all(|asked| asked.path.get(1).is_none_or(|second| second != "me")));
}

#[test]
fn an_address_that_could_name_another_path_is_not_asked_at_all() {
    let (server, _) = served();
    for address in ["support@example.org/../me", "a b@example.org", "x@example.org?q=1", "support", ""] {
        let err = calendars_at(&server.url(), TOKEN, Some(address)).and_then(|_| events_in_at(&server.url(), TOKEN, &query(address, "")).map(|_| ()));
        if !address.is_empty() {
            assert!(err.is_err(), "{address}");
        }
    }
    assert!(asked(&server).iter().all(|line| !line.contains("/me")), "nothing for a doubtful address went to /me");
}

#[test]
fn a_calendar_shared_for_free_and_busy_only_is_drawn_from_its_schedule_without_subjects() {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        state.add_other(SHARED, true);
        let main = state.add_other_calendar(SHARED, "Calendar", true);
        state.add_other_event(SHARED, &main, "Salary review", at(2026, 10, 14, 9), "busy");
        state.add_other_event(SHARED, &main, "Maybe lunch", at(2026, 10, 14, 12), "tentative");
        state.add_other_event(SHARED, &main, "Holiday", at(2026, 10, 15, 0), "outOfOffice");
        state.add_other_event(SHARED, &main, "From home", at(2026, 10, 16, 9), "workingElsewhere");
        state.share_calendars(SHARED, CalendarAccess::FreeBusy);
    }
    // The list of calendars is refused: that is what free and busy only means.
    assert!(matches!(calendars_at(&server.url(), TOKEN, Some(SHARED)), Err(NetError::Forbidden(_))));
    let blocks = events_in_at(&server.url(), TOKEN, &query(SHARED, "")).unwrap();
    assert!(blocks.free_busy && blocks.complete);
    let shown: Vec<(ShowAs, Timestamp)> = blocks.events.iter().map(|e| (e.show_as, e.start)).collect();
    assert_eq!(
        shown,
        vec![
            (ShowAs::Busy, at(2026, 10, 14, 9)),
            (ShowAs::Tentative, at(2026, 10, 14, 12)),
            (ShowAs::OutOfOffice, at(2026, 10, 15, 0)),
            (ShowAs::WorkingElsewhere, at(2026, 10, 16, 9)),
        ]
    );
    // Not a word of what they are: no subject, no place, nobody.
    let text = serde_json::to_string(&blocks.events).unwrap();
    for secret in ["Salary", "lunch", "Holiday", "home"] {
        assert!(!text.contains(secret), "{secret}");
    }
    assert!(blocks.events.iter().all(|e| e.title.is_empty() && e.location.is_empty() && e.id.starts_with("freebusy:")));
    // Asked again, the same blocks have the same ids: the mailbox knows them again.
    let again = events_in_at(&server.url(), TOKEN, &query(SHARED, "")).unwrap();
    assert_eq!(again.events.iter().map(|e| &e.id).collect::<Vec<_>>(), blocks.events.iter().map(|e| &e.id).collect::<Vec<_>>());
    // All of it for the mailbox, and none at /me.
    let seen = asked(&server);
    assert!(seen.iter().any(|line| line.ends_with("calendar/getSchedule")), "{seen:?}");
    assert!(seen.iter().all(|line| line.contains("users/support@example.org")), "{seen:?}");
}

#[test]
fn a_window_longer_than_a_schedule_may_cover_is_asked_for_in_stretches() {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        state.add_other(SHARED, true);
        let main = state.add_other_calendar(SHARED, "Calendar", true);
        state.add_other_event(SHARED, &main, "Early", at(2026, 10, 14, 9), "busy");
        state.add_other_event(SHARED, &main, "Late", at(2027, 6, 14, 9), "busy");
        state.share_calendars(SHARED, CalendarAccess::FreeBusy);
    }
    let wide = EventsQuery { from: at(2026, 9, 1, 0), to: at(2027, 10, 1, 0), shared: SHARED.into(), ..Default::default() };
    let blocks = events_in_at(&server.url(), TOKEN, &wide).unwrap();
    assert_eq!(blocks.events.len(), 2, "both ends of the window");
    assert!(blocks.complete);
    let asks = asked(&server).iter().filter(|line| line.ends_with("getSchedule")).count();
    assert!((6..=MAX_SCHEDULE_ASKS).contains(&asks), "{asks}");
}

#[test]
fn a_shared_mailbox_nothing_is_shared_from_says_so_and_nothing_goes_to_me() {
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    assert!(matches!(calendars_at(&server.url(), TOKEN, Some(SHARED)), Err(NetError::Forbidden(_))));
    assert!(matches!(events_in_at(&server.url(), TOKEN, &query(SHARED, "")), Err(NetError::Forbidden(_))));
    // A mailbox that isn't there is an error too, and the owner's own calendar is not shown for it.
    assert!(events_in_at(&server.url(), TOKEN, &query("nobody@example.org", "")).is_err());
    assert!(asked(&server).iter().all(|line| !line.contains("/me/") && !line.ends_with("/me")));
}
