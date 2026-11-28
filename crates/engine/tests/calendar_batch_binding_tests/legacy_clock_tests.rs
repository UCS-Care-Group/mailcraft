// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A legacy occurrence question is held to the day it resolves to, whatever the clock is called:
//! a fixed offset and a named zone as before, and an anonymous host clock by that resolved day
//! rather than a blanket refusal. An event that changes while the question waits is still refused.

use super::*;

use std::sync::Arc;

use mailcraft_engine::time::Zone;

/// The clocks a legacy occurrence question is asked under: rules that are named or fixed, and
/// the anonymous system fallback that names none of its own.
fn clocks() -> [Zone; 3] {
    [Zone::fixed(0).expect("UTC exists"), Zone::named("Africa/Abidjan").expect("a named zone"), Zone::LOCAL]
}

/// The occurrence question about the fixture's series, as a form saves it: one day of it, the
/// day the series starts on.
fn occurrence_params(series: Id, event: &Event) -> Value {
    json!({
        "id": series,
        "scope": "occurrence",
        "occurrence": event.start.date.iso(),
        "title": "Clock binding control"
    })
}

#[test]
fn legacy_occurrence_approval_holds_the_resolved_day_on_a_fixed_named_or_anonymous_clock() {
    for zone in clocks() {
        let f = fixture();
        let series = f.series;
        let mut session = Session::in_zone(f.s.mailbox, NOW, zone);
        assert_eq!(session.zone(), zone, "fixture: the effective clock is the requested one");
        // A host-independent fixture: even the anonymous clock reads a fixed, zero offset here,
        // so this arm resolves the same wherever the tests run.
        session.fallback_offset = Some(Arc::new(|_instant: i64| 0));
        if zone.is_local() {
            assert!(zone.id().is_empty(), "fixture: the anonymous fallback has no stable zone identifier");
        } else {
            assert!(!zone.id().is_empty(), "fixture: a fixed or named clock identifies its rules");
        }
        let event = session.mailbox.event(series).expect("fixture: the local series exists");
        assert!(event.recurrence.is_some(), "fixture: an occurrence, not a standalone appointment");
        let params = occurrence_params(series, event);
        let mut probe = Session::in_zone(session.mailbox.clone(), NOW, zone);
        probe.fallback_offset = Some(Arc::new(|_instant: i64| 0));
        assert!(probe.run("calendar.edit", &params).is_ok(), "fixture: the occurrence edit is a valid operation");
        let what = agent::describe(&session, "calendar.edit", &params);
        let shown = agent::bind::Shown::new(&session, "calendar.edit", &params, &what)
            .expect("a readable occurrence question is captured whatever the clock's name");
        assert!(
            shown.check(&session, "calendar.edit", &params).is_ok(),
            "an unchanged occurrence question stays answerable on a fixed, named or anonymous clock"
        );
    }
}

#[test]
fn legacy_occurrence_approval_is_refused_once_the_event_it_named_changes_on_any_clock() {
    for zone in clocks() {
        let f = fixture();
        let series = f.series;
        let mut session = Session::in_zone(f.s.mailbox, NOW, zone);
        session.fallback_offset = Some(Arc::new(|_instant: i64| 0));
        let event = session.mailbox.event(series).expect("fixture: the local series exists");
        let params = occurrence_params(series, event);
        let what = agent::describe(&session, "calendar.edit", &params);
        let shown = agent::bind::Shown::new(&session, "calendar.edit", &params, &what)
            .expect("a readable occurrence question is captured whatever the clock's name");
        assert!(shown.check(&session, "calendar.edit", &params).is_ok(), "fixture: answerable while it is untouched");
        session.mailbox.event_mut(series).expect("fixture: the series is there").title = "Changed while asked".into();
        assert!(
            shown.check(&session, "calendar.edit", &params).is_err(),
            "a yes is refused once the event it named has changed, whatever the clock is called"
        );
    }
}
