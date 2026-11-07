// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A to-do made from an appointment or a contact (#831), as Outlook makes one when either is
//! dragged onto the To-Do Bar's tasks: named as the item, with what it says in the notes.

use mailcraft_model::{Address, Attendee, Contact, Event, Freq, Id, Labeled, PostalAddress, Recurrence};
use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use crate::agent::{Access, Class, Verdict, check, classify, describe};
use crate::{Module, Session, sample};

/// The sample mailbox on Wednesday 2026-10-14, 16:00 UTC.
fn session() -> Session {
    let now = DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0);
    Session::at(sample::mailbox(now, 0), now, 0)
}

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn at(day: u8, hour: u8, minute: u8) -> DateTime {
    DateTime::new(Date::new(2026, 10, day).unwrap(), hour, minute, 0).unwrap()
}

fn event(s: &mut Session, title: &str) -> Id {
    let calendar = s.mailbox.calendars.first().map(|c| c.id).unwrap();
    s.mailbox
        .add_event(Event {
            calendar,
            title: title.into(),
            location: "Room 4, Larchmere Studio".into(),
            description: "Bring the birch samples.\nAnd the price list.".into(),
            start: at(16, 10, 0),
            end: at(16, 11, 30),
            organizer: Some(Address::new("Ines Varga", "ines@larchmere.example")),
            attendees: vec![Attendee { address: Address::new("Tom Arden", "tom@larchmere.example"), ..Default::default() }],
            ..Default::default()
        })
        .unwrap()
}

fn contact(s: &mut Session) -> Id {
    s.mailbox.add_contact(Contact {
        first: "Odile".into(),
        last: "Frey".into(),
        company: "Frey Joinery".into(),
        title: "Owner".into(),
        emails: vec![Labeled::new("work", "odile@freyjoinery.example")],
        phones: vec![Labeled::new("mobile", "+44 7700 900123")],
        addresses: vec![PostalAddress { street: "3 Mill Lane".into(), city: "Ashby".into(), ..Default::default() }],
        notes: "Prefers calls before noon.".into(),
        ..Default::default()
    })
}

fn notes(s: &Session, task: &Value) -> String {
    s.mailbox.task(task["id"].as_u64().unwrap()).unwrap().notes.clone()
}

#[test]
fn an_appointment_becomes_a_task_named_as_it_with_when_and_where_in_its_notes() {
    let mut s = session();
    let id = event(&mut s, "Review the joinery quote");
    let before = s.mailbox.tasks.len();
    let made = run(&mut s, "tasks.fromEvent", json!({"id": id, "show": false}));
    assert_eq!(s.mailbox.tasks.len(), before + 1);
    let task = s.mailbox.task(made["id"].as_u64().unwrap()).unwrap().clone();
    assert_eq!(task.title, "Review the joinery quote");
    // No due date of its own, as in Outlook, and no message behind it.
    assert_eq!((task.due, task.message, task.completed), (None, None, None));
    for words in [
        "When:",
        "Friday, October 16, 2026",
        "10:00",
        "11:30",
        "Where: Room 4, Larchmere Studio",
        "Tom Arden",
        "Ines Varga",
        "Bring the birch samples.",
        "And the price list.",
    ] {
        assert!(task.notes.contains(words), "{words:?} in {:?}", task.notes);
    }
    // The module in front stays (a drop on the To-Do Bar); the task is the one selected.
    assert_eq!(s.view.module, Module::Mail);
    assert_eq!(s.sel.task, Some(task.id));
    // The appointment is as it was.
    assert_eq!(s.mailbox.event(id).map(|e| e.title.as_str()), Some("Review the joinery quote"));
    // Asked for from the ribbon or a script, Tasks comes to the front, as for a message.
    run(&mut s, "tasks.fromEvent", json!({"id": id, "due": "2026-10-20"}));
    assert_eq!(s.view.module, Module::Tasks);
    assert_eq!(s.sel.task.and_then(|t| s.mailbox.task(t)).and_then(|t| t.due), Date::new(2026, 10, 20));
    // Undo takes the task back.
    let made = s.sel.task.unwrap();
    assert!(s.undo());
    assert!(s.mailbox.task(made).is_none());
}

#[test]
fn an_occurrence_of_a_series_says_its_own_day_and_a_day_it_skips_is_refused() {
    let mut s = session();
    let id = event(&mut s, "Stand-up");
    s.mailbox.event_mut(id).unwrap().recurrence = Some(Recurrence { freq: Freq::Daily, interval: 1, count: Some(5), ..Default::default() });
    let made = run(&mut s, "tasks.fromEvent", json!({"id": id, "day": "2026-10-18", "show": false}));
    assert!(notes(&s, &made).contains("Sunday, October 18, 2026"), "{}", notes(&s, &made));
    // The selected occurrence, when none is named.
    run(&mut s, "calendar.select", json!({"id": id, "day": "2026-10-17"}));
    let made = run(&mut s, "tasks.fromEvent", json!({"show": false}));
    assert!(notes(&s, &made).contains("Saturday, October 17, 2026"), "{}", notes(&s, &made));
    let tasks = s.mailbox.tasks.len();
    for day in ["2026-10-30", "2026-10-15"] {
        assert!(s.run("tasks.fromEvent", &json!({"id": id, "day": day})).is_err(), "{day}");
    }
    assert!(s.run("tasks.fromEvent", &json!({"id": id, "day": "the 18th"})).is_err());
    assert_eq!(s.mailbox.tasks.len(), tasks);
}

#[test]
fn a_contact_becomes_a_task_named_as_them_with_their_details_in_its_notes() {
    let mut s = session();
    let id = contact(&mut s);
    let made = run(&mut s, "tasks.fromContact", json!({"id": id, "show": false}));
    let task = s.mailbox.task(made["id"].as_u64().unwrap()).unwrap().clone();
    assert_eq!(task.title, "Odile Frey");
    for words in
        ["Owner, Frey Joinery", "Email: odile@freyjoinery.example", "+44 7700 900123 (mobile)", "3 Mill Lane, Ashby", "Prefers calls before noon."]
    {
        assert!(task.notes.contains(words), "{words:?} in {:?}", task.notes);
    }
    assert_eq!(s.view.module, Module::Mail);
    // The selected contact, when none is named; with none selected, nothing is made.
    run(&mut s, "people.select", json!({"id": id}));
    run(&mut s, "tasks.fromContact", json!({}));
    assert_eq!(s.view.module, Module::Tasks);
    run(&mut s, "people.select", json!({"id": null}));
    s.sel.event = None;
    let tasks = s.mailbox.tasks.len();
    assert!(s.run("tasks.fromContact", &json!({})).is_err());
    assert!(s.run("tasks.fromEvent", &json!({})).is_err());
    assert_eq!(s.mailbox.tasks.len(), tasks);
}

#[test]
fn what_an_item_says_is_copied_within_bounds_and_named_wrongly_it_makes_nothing() {
    let mut s = session();
    let id = event(&mut s, "  \r\n ");
    {
        let e = s.mailbox.event_mut(id).unwrap();
        e.description = format!("{}\u{7}bell\n{}", "x".repeat(20_000), "line\n".repeat(5_000));
        e.location = "Upstairs\r\nInjected: header".into();
    }
    let made = run(&mut s, "tasks.fromEvent", json!({"id": id, "show": false}));
    let task = s.mailbox.task(made["id"].as_u64().unwrap()).unwrap().clone();
    // A title of nothing but blanks is "(no title)", as the calendar shows it.
    assert_eq!(task.title, "(no title)");
    assert!(task.notes.chars().count() < 12_000, "{}", task.notes.len());
    assert!(task.notes.lines().count() <= 220);
    assert!(!task.notes.contains('\u{7}') && !task.notes.contains('\r'));
    assert!(task.notes.contains("Where: Upstairs Injected: header"), "{}", task.notes);
    // Nothing that turns or hides text comes through, in the title or the notes.
    let turned = event(&mut s, "Pay \u{202E}eciovni\u{200B} now\u{1b}");
    s.mailbox.event_mut(turned).unwrap().description = "Line \u{202E}one".into();
    let made = run(&mut s, "tasks.fromEvent", json!({"id": turned, "show": false}));
    let task = s.mailbox.task(made["id"].as_u64().unwrap()).unwrap().clone();
    for c in ['\u{202E}', '\u{200B}', '\u{1b}'] {
        assert!(!task.title.contains(c) && !task.notes.contains(c), "{c:?} in {task:?}");
    }
    let tasks = s.mailbox.tasks.len();
    for wrong in [json!({"id": 999_999}), json!({"id": "7"}), json!({"id": null}), json!({"id": id, "extra": 1}), json!({"id": id, "show": "no"})] {
        assert!(s.run("tasks.fromEvent", &wrong).is_err(), "{wrong}");
        assert!(s.run("tasks.fromContact", &wrong).is_err(), "{wrong}");
    }
    assert_eq!(s.mailbox.tasks.len(), tasks);
}

#[test]
fn an_agent_may_make_one_here_and_is_asked_before_it_goes_to_an_account() {
    let mut s = session();
    let (ev, c) = (event(&mut s, "Kiln check"), contact(&mut s));
    assert_eq!(classify(&s, "tasks.fromEvent", &json!({"id": ev})), Access::Safe);
    assert_eq!(classify(&s, "tasks.fromContact", &json!({"id": c})), Access::Safe);
    // Named wrongly, it is refused, never taken for the selection.
    for wrong in [json!({"id": null}), json!({"id": "7"}), json!({"id": 0})] {
        assert!(matches!(check(&s, "tasks.fromEvent", &wrong, &[Class::Send]), Verdict::Refuse(_)), "{wrong}");
        assert!(matches!(check(&s, "tasks.fromContact", &wrong, &[Class::Send]), Verdict::Refuse(_)), "{wrong}");
    }
    // With the list on an account's To Do, a new task would go there: that is asked about.
    let work = run(&mut s, "account.add", json!({"email": "work@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"]
        .as_u64()
        .unwrap();
    run(&mut s, "view.taskAccount", json!({"value": work}));
    assert_eq!(classify(&s, "tasks.fromEvent", &json!({"id": ev})), Access::Owner(Class::Send));
    assert_eq!(classify(&s, "tasks.fromContact", &json!({"id": c})), Access::Owner(Class::Send));
    assert!(describe(&s, "tasks.fromContact", &json!({"id": c})).contains("work@example.org"));
}
