// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The task form.

use super::*;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn session() -> Session {
    Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0)
}

fn task_id(s: &Session, title: &str) -> Id {
    s.mailbox.tasks.values().find(|t| t.title == title).map(|t| t.id).unwrap()
}

#[test]
fn a_new_task_sends_what_was_filled_in() {
    let mut s = session();
    let mut f = TaskForm::new(&s, &json!({"title": "Book flights", "due": "2026-10-20"}));
    assert_eq!((f.name(), f.title().as_str(), f.ready()), ("task", "New Task", true));
    assert_eq!(f.command(), ("tasks.new", json!({"title": "Book flights", "due": "2026-10-20"})));
    assert_eq!(f.completion(), None);
    f.start = "20261015".into();
    f.importance = Importance::High;
    f.notes = "Aisle seat".into();
    f.categories = vec!["Red category".into()];
    // A reminder with a day and no time is for the start of the working day.
    f.remind_date = "2026-10-19".into();
    assert_eq!(
        f.command().1,
        json!({"title": "Book flights", "due": "2026-10-20", "start": "2026-10-15", "importance": "high", "notes": "Aisle seat",
            "categories": ["Red category"], "reminder": "2026-10-19T08:00"})
    );
    f.remind_time = "5:30pm".into();
    assert_eq!(f.command().1["reminder"], "2026-10-19T17:30");
    let (command, params) = f.command();
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    let t = s.mailbox.task(id).unwrap();
    assert_eq!(
        (t.title.as_str(), t.due, t.start, t.importance),
        ("Book flights", Date::new(2026, 10, 20), Date::new(2026, 10, 15), Importance::High)
    );
    assert_eq!(t.reminder.map(|r| s.local(r).iso()).as_deref(), Some("2026-10-19T17:30:00"));
    // Prefilled the other ways.
    let f = TaskForm::new(&s, &json!({"title": 5, "importance": "LOW", "notes": "n", "start": "2026-10-15"}));
    assert_eq!((f.title.as_str(), f.importance, f.ready()), ("", Importance::Low, false));
    assert_eq!(f.command().1, json!({"importance": "low", "notes": "n", "start": "2026-10-15"}));
}

#[test]
fn an_existing_task_fills_the_form_and_sends_its_changes() {
    let mut s = session();
    // Nothing selected and nothing named: no form.
    assert!(TaskForm::edit(&s, &json!({})).is_none());
    assert!(TaskForm::edit(&s, &json!({"id": 999_999})).is_none());
    let id = task_id(&s, "Review the Q4 roadmap draft");
    s.run("tasks.edit", &json!({"id": id, "reminder": "2026-10-15T09:30"})).unwrap();
    s.sel.task = Some(id);
    let mut f = TaskForm::edit(&s, &json!({})).unwrap();
    assert_eq!((f.name(), f.title().as_str(), f.id), ("editTask", "Task", Some(id)));
    assert_eq!(
        (f.title.as_str(), f.due.as_str(), f.start.as_str(), f.importance),
        ("Review the Q4 roadmap draft", "2026-10-14", "", Importance::High)
    );
    assert_eq!(
        (f.remind_date.as_str(), f.remind_time.as_str(), f.completed, f.notes.as_str()),
        ("2026-10-15", "9:30 AM", false, "Bring the Atlas estimate as a range.")
    );
    // Untouched, nothing is sent but which task it is, and it isn't completed.
    assert_eq!((f.command(), f.completion()), (("tasks.edit", json!({"id": id})), None));
    f.due.clear();
    f.remind_date.clear();
    f.remind_time.clear();
    f.importance = Importance::Normal;
    f.completed = true;
    assert_eq!(f.command().1, json!({"id": id, "due": null, "reminder": null, "importance": "normal"}));
    assert_eq!(f.completion(), Some(("tasks.complete", json!({"id": id, "value": true}))));
    let (command, params) = f.command();
    s.run(command, &params).unwrap();
    let (command, params) = f.completion().unwrap();
    s.run(command, &params).unwrap();
    let t = s.mailbox.task(id).unwrap();
    assert_eq!((t.due, t.reminder, t.importance, t.is_complete()), (None, None, Importance::Normal, true));
    // A finished task opens ticked, and clearing the box reopens it.
    let done = task_id(&s, "Renew the studio domain");
    let mut f = TaskForm::edit(&s, &json!({"id": done})).unwrap();
    assert!(f.completed && f.completion().is_none());
    f.completed = false;
    assert_eq!(f.completion(), Some(("tasks.complete", json!({"id": done, "value": false}))));
}

#[test]
fn dates_and_reminders_are_checked_before_ok() {
    let s = session();
    let mut f = TaskForm::new(&s, &json!({"title": "x"}));
    assert!(f.ready());
    f.due = "next week".into();
    assert!(!f.ready());
    f.due = "2026-10-19".into();
    // A reminder time needs its day.
    f.remind_time = "9".into();
    assert!(!f.ready());
    f.remind_date = "2026-10-18".into();
    assert!(f.ready());
    assert_eq!(f.reminder(), json!("2026-10-18T09:00"));
    f.remind_time = "soon".into();
    assert!(!f.ready());
    assert_eq!(f.reminder(), json!("2026-10-18Tsoon"));
    f.title = "  ".into();
    f.remind_time.clear();
    assert!(!f.ready());
    // Next week starts on its Monday.
    let day = |d: u8| Date::new(2026, 10, d).unwrap();
    assert_eq!((next_week(day(14)), next_week(day(18)), next_week(day(19))), (day(19), day(19), day(26)));
}

#[test]
fn hostile_text_goes_through_without_harm() {
    let mut s = session();
    let junk = "\u{0}\u{202e}é\n".repeat(40_000);
    let mut f = TaskForm::new(&s, &json!({"title": junk, "due": junk, "start": junk, "notes": junk, "importance": junk}));
    assert!(!f.ready());
    f.remind_date.clone_from(&junk);
    f.remind_time.clone_from(&junk);
    f.categories = vec![junk.clone(); 20];
    // Dates that aren't dates go as typed, and the command refuses them.
    let (command, params) = f.command();
    assert!(s.run(command, &params).unwrap_err().to_string().contains("must be a date"));
    (f.due, f.start, f.remind_date, f.remind_time) = (String::new(), String::new(), String::new(), String::new());
    let (command, params) = f.command();
    let id = s.run(command, &params).unwrap()["id"].as_u64().unwrap();
    assert!(s.mailbox.task(id).unwrap().title.chars().count() <= 500);
}
