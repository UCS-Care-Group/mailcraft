// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send Status Report: a new message saying how a task stands, which nothing sends.

use mailcraft_model::{Importance, Task};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::agent::{Access, classify};
use crate::{Session, sample};

/// The sample mailbox on Wednesday 2026-10-14, 16:00 UTC.
fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn task(s: &mut Session) -> u64 {
    s.mailbox.add_task(Task {
        title: "Order the birch panels".into(),
        notes: "Two quotes in.\nWaiting on the third.".into(),
        start: Date::new(2026, 10, 1),
        due: Date::new(2026, 10, 10),
        importance: Importance::High,
        categories: vec!["Studio".into()],
        ..Default::default()
    })
}

#[test]
fn a_report_says_how_the_task_stands_and_sends_nothing() {
    let mut s = session();
    let id = task(&mut s);
    let messages = s.mailbox.messages.len();
    run(&mut s, "tasks.statusReport", json!({"id": id}));
    let c = s.compose.as_ref().unwrap();
    assert_eq!((c.subject.as_str(), c.to.as_str()), ("Task Status Report: Order the birch panels", ""));
    for line in
        ["Task: Order the birch panels", "Status: In Progress", "(overdue)", "Importance: High", "Categories: Studio", "Waiting on the third."]
    {
        assert!(c.body.plain().contains(line), "{line} in {}", c.body.plain());
    }
    assert_eq!(s.mailbox.messages.len(), messages);
}

#[test]
fn a_finished_task_says_when() {
    let mut s = session();
    let id = task(&mut s);
    run(&mut s, "tasks.complete", json!({"id": id, "value": true}));
    let text = super::status::text(&s, s.mailbox.task(id).unwrap());
    assert!(text.contains("Status: Completed (") && !text.contains("overdue"), "{text}");
    let later = s.mailbox.add_task(Task { title: "Later".into(), start: Date::new(2027, 1, 4), ..Default::default() });
    let text = super::status::text(&s, s.mailbox.task(later).unwrap());
    assert!(text.contains("Status: Not Started") && text.contains("Due date: None"), "{text}");
}

#[test]
fn a_report_names_its_task_strictly() {
    let mut s = session();
    let id = task(&mut s);
    s.sel.task = Some(id);
    for params in [json!({"id": "x"}), json!({"id": 999_999}), json!({"id": id, "to": "someone@partner.example"})] {
        assert!(s.run("tasks.statusReport", &params).is_err(), "{params}");
        assert!(s.compose.is_none(), "{params}");
    }
    run(&mut s, "tasks.statusReport", json!({}));
    assert!(s.compose.is_some());
}

#[test]
fn a_report_saves_the_message_being_written_and_opens_beside_it() {
    let mut s = session();
    let id = task(&mut s);
    run(&mut s, "mail.new", json!({}));
    run(&mut s, "compose.set", json!({"subject": "Unsaved", "body": "Half a thought"}));
    run(&mut s, "tasks.statusReport", json!({"id": id}));
    // The owner's is saved to Drafts first (#286) and stays open.
    assert!(s.mailbox.messages.values().any(|m| m.draft && m.subject == "Unsaved"));
    assert!(s.composing.iter().any(|c| !c.dirty && c.subject == "Unsaved"));
    assert_eq!(classify(&s, "tasks.statusReport", &json!({"id": id})), Access::Safe);
}
