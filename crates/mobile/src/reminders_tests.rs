// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reminders a phone schedules: what the core hands over, that a key holds from one ask to
//! the next and changes with its event, and that a change to them is said.

use serde_json::{Value, json};

use crate::fake_tests::{block_on, now, sample_core};
use crate::{Core, CoreError, Event, MAX_SCHEDULED, ReminderKind, ScheduledReminder};

fn exec(core: &Core, id: &str, params: Value) -> Result<Value, CoreError> {
    block_on(core.execute(id.to_string(), params.to_string())).map(|s| serde_json::from_str(&s).unwrap())
}

fn schedule(core: &Core) -> Vec<ScheduledReminder> {
    block_on(core.reminder_schedule(MAX_SCHEDULED)).unwrap().reminders
}

/// Whether a `ReminderScheduleChanged` is among the events waiting (the queue is read to its end).
/// A call goes first: the session thread takes one job after another, so the events of the jobs
/// before it are all in the queue once it is answered.
fn said_changed(core: &Core) -> bool {
    let _ = schedule(core);
    let mut said = false;
    for _ in 0..core.events.len() {
        said |= block_on(core.next_event()) == Some(Event::ReminderScheduleChanged);
    }
    said
}

#[test]
fn the_schedule_is_soonest_first_bounded_and_keyed_the_same_at_every_ask() {
    let core = sample_core();
    let first = schedule(&core);
    assert!(!first.is_empty());
    assert!(first.len() <= MAX_SCHEDULED as usize);
    assert!(first.windows(2).all(|w| w[0].at <= w[1].at));
    assert!(first.iter().all(|r| r.kind == ReminderKind::Event && r.starts > now() && r.at <= r.starts));
    assert_eq!(schedule(&core), first);
    // The same mailbox in another core (the app started again) gives the same keys.
    assert_eq!(schedule(&sample_core()).iter().map(|r| r.key).collect::<Vec<_>>(), first.iter().map(|r| r.key).collect::<Vec<_>>());
    let keys: std::collections::HashSet<u64> = first.iter().map(|r| r.key).collect();
    assert_eq!(keys.len(), first.len());
    assert!(block_on(core.reminder_schedule(0)).unwrap().reminders.is_empty());
    assert!(block_on(core.reminder_schedule(u32::MAX)).unwrap().reminders.len() <= MAX_SCHEDULED as usize);
    assert_eq!(block_on(core.reminder_schedule(1)).unwrap().reminders, first[..1]);
}

#[test]
fn a_new_moved_or_deleted_event_changes_the_schedule_and_says_so() {
    let core = sample_core();
    let _ = said_changed(&core);
    let made = exec(
        &core,
        "calendar.new",
        json!({"title": "Hygienist at the clinic", "start": "2026-10-15T08:00", "end": "2026-10-15T08:30", "reminder": 30}),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap();
    assert!(said_changed(&core));
    let dentist = schedule(&core).into_iter().find(|r| r.title == "Hygienist at the clinic").unwrap();
    assert_eq!(dentist.at, dentist.starts - 30 * 60);
    // Moved: another key, so the app cancels the old alarm and sets a new one.
    exec(&core, "calendar.move", json!({"id": made, "start": "2026-10-15T09:00"})).unwrap();
    assert!(said_changed(&core));
    let moved = schedule(&core).into_iter().find(|r| r.title == "Hygienist at the clinic").unwrap();
    assert_ne!(moved.key, dentist.key);
    assert_eq!(moved.starts - dentist.starts, 3600);
    // A change that doesn't touch a reminder says nothing.
    let inbox = block_on(core.folders(0)).unwrap().folders.into_iter().find(|f| f.kind == "inbox").unwrap().id;
    let row = block_on(core.messages(inbox, "all".into(), 0, 1)).unwrap().rows[0].id;
    exec(&core, "mail.flag", json!({"id": row})).unwrap();
    assert!(!said_changed(&core));
    exec(&core, "calendar.delete", json!({"id": made})).unwrap();
    assert!(said_changed(&core));
    assert!(schedule(&core).iter().all(|r| r.title != "Hygienist at the clinic"));
}

#[test]
fn a_task_reminder_is_scheduled_until_the_task_is_done() {
    let core = sample_core();
    let task = exec(&core, "tasks.new", json!({"title": "Call the plumber", "reminder": "2026-10-14T18:00"})).unwrap()["id"].as_u64().unwrap();
    let found = schedule(&core).into_iter().find(|r| r.title == "Call the plumber").unwrap();
    assert_eq!(found.kind, ReminderKind::Task);
    assert_eq!((found.at, found.starts, found.all_day, found.location.as_str()), (found.starts, found.at, false, ""));
    exec(&core, "tasks.complete", json!({"id": task})).unwrap();
    assert!(schedule(&core).iter().all(|r| r.title != "Call the plumber"));
}

#[test]
fn a_long_title_is_cut() {
    let core = sample_core();
    exec(&core, "calendar.new", json!({"title": "é".repeat(5_000), "start": "2026-10-15T08:00", "end": "2026-10-15T08:30", "reminder": 5})).unwrap();
    let long = schedule(&core).into_iter().find(|r| r.title.starts_with('é')).unwrap();
    assert_eq!(long.title.chars().count(), crate::text::MAX_TITLE);
}
