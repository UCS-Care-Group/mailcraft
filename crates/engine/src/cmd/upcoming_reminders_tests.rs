// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reminders still to come, for a phone to have the system raise them while the app is closed.

use mailcraft_model::Mailbox;
use mailcraft_time::{Date, DateTime, Timestamp, Zone};
use serde_json::{Value, json};

use super::view::{UpcomingFor, upcoming_reminders};
use crate::Session;

/// Monday 2026-10-12, 15:00 UTC (16:00 in London).
const NOW: Timestamp = 1_791_817_200;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn session() -> Session {
    Session::in_zone(Mailbox::with_local_account("", ""), NOW, Zone::named("Europe/London").unwrap())
}

fn instant(y: i32, m: u8, d: u8, h: u8, min: u8) -> Timestamp {
    DateTime::new(Date::new(y, m, d).unwrap(), h, min, 0).unwrap().to_timestamp(0)
}

fn new_event(s: &mut Session, title: &str, start: &str, end: &str, reminder: Value) -> u64 {
    run(s, "calendar.new", json!({"title": title, "start": start, "end": end, "reminder": reminder}))["id"].as_u64().unwrap()
}

#[test]
fn events_and_tasks_come_soonest_first_with_the_instant_each_is_due() {
    let mut s = session();
    // 17:00 London (BST, UTC+1) is 16:00 UTC; fifteen minutes before is 15:45 UTC.
    let standup = new_event(&mut s, "Standup", "2026-10-12T17:00", "2026-10-12T17:15", json!(15));
    let review = new_event(&mut s, "Review", "2026-10-13T09:00", "2026-10-13T10:00", json!(60));
    new_event(&mut s, "No reminder", "2026-10-12T18:00", "2026-10-12T19:00", Value::Null);
    run(&mut s, "tasks.new", json!({"title": "Pay rent", "reminder": "2026-10-12T20:00"}));
    let got = upcoming_reminders(&s, NOW, 64);
    let seen: Vec<(&str, Timestamp)> = got.iter().map(|u| (u.title.as_str(), u.at)).collect();
    assert_eq!(
        seen,
        [("Standup", instant(2026, 10, 12, 15, 45)), ("Pay rent", instant(2026, 10, 12, 19, 0)), ("Review", instant(2026, 10, 13, 7, 0))]
    );
    assert!(matches!(got[0].what, UpcomingFor::Event { event, .. } if event == standup));
    assert_eq!(got[0].starts, instant(2026, 10, 12, 16, 0));
    assert!(matches!(got[2].what, UpcomingFor::Event { event, .. } if event == review));
    assert!(matches!(got[1].what, UpcomingFor::Task { .. }));
}

#[test]
fn a_started_event_a_finished_task_and_an_old_task_reminder_are_left_out() {
    let mut s = session();
    // Started an hour ago: its reminder was for then, and the phone has nothing to raise.
    new_event(&mut s, "Running", "2026-10-12T15:00", "2026-10-12T17:00", json!(15));
    // Due already but not started: the phone was off when it was due, so it is raised now.
    new_event(&mut s, "Soon", "2026-10-12T16:30", "2026-10-12T17:00", json!(60));
    let done = run(&mut s, "tasks.new", json!({"title": "Done", "reminder": "2026-10-12T18:00"}))["id"].as_u64().unwrap();
    run(&mut s, "tasks.complete", json!({"id": done}));
    run(&mut s, "tasks.new", json!({"title": "Yesterday noon", "reminder": "2026-10-11T12:00"}));
    run(&mut s, "tasks.new", json!({"title": "This morning", "reminder": "2026-10-12T09:00"}));
    let titles: Vec<String> = upcoming_reminders(&s, NOW, 64).into_iter().map(|u| u.title).collect();
    assert_eq!(titles, ["This morning", "Soon"]);
}

#[test]
fn a_series_gives_each_occurrence_and_the_limit_holds() {
    let mut s = session();
    run(
        &mut s,
        "calendar.new",
        json!({"title": "Daily", "start": "2026-10-13T09:00", "end": "2026-10-13T09:15", "reminder": 5, "recurrence": {"freq": "daily"}}),
    );
    let all = upcoming_reminders(&s, NOW, 500);
    // Four weeks ahead and a day, as `due_reminders` looks.
    assert!((28..=30).contains(&all.len()), "{}", all.len());
    assert!(all.windows(2).all(|w| w[0].at < w[1].at));
    assert_eq!(upcoming_reminders(&s, NOW, 3).len(), 3);
    assert!(upcoming_reminders(&s, NOW, 0).is_empty());
}

#[test]
fn a_cancelled_event_has_no_reminder() {
    let mut s = session();
    let id = new_event(&mut s, "Off", "2026-10-13T09:00", "2026-10-13T10:00", json!(15));
    std::sync::Arc::make_mut(s.mailbox.events.get_mut(&id).unwrap()).cancelled = true;
    assert!(upcoming_reminders(&s, NOW, 64).is_empty());
}
