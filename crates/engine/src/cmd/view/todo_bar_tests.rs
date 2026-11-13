// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The To-Do Bar's parts, module by module (#831), and a message made a task from it.

use serde_json::{Value, json};

use crate::{Module, Session, TodoParts, ViewState};

const NOW: i64 = 1_790_000_000;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

#[test]
fn the_bar_is_closed_everywhere_until_it_is_opened() {
    let s = Session::empty();
    for m in Module::ALL {
        assert!(!s.view.todo_bar.of(m).any(), "{m:?}");
    }
    // A view saved before the bar existed reads back with it closed.
    let old: ViewState = serde_json::from_value(json!({"module": "mail", "folderPane": false})).unwrap();
    assert_eq!(old.todo_bar, crate::TodoBar::default());
    assert!(!old.folder_pane);
}

#[test]
fn each_part_toggles_in_the_module_in_front_and_off_closes_the_bar() {
    let mut s = Session::empty();
    let r = run(&mut s, "view.todoBar", json!({"part": "calendar"}));
    assert_eq!((r["module"].as_str(), r["calendar"].as_bool(), r["open"].as_bool()), (Some("mail"), Some(true), Some(true)));
    run(&mut s, "view.todoBar", json!({"part": "tasks", "value": true}));
    run(&mut s, "view.todoBar", json!({"part": "People"}));
    assert_eq!(s.view.todo_bar.mail, TodoParts { calendar: true, people: true, tasks: true });
    // Toggled again, a part goes; asked for on, it stays.
    run(&mut s, "view.todoBar", json!({"part": "people"}));
    run(&mut s, "view.todoBar", json!({"part": "tasks", "value": true}));
    assert_eq!(s.view.todo_bar.mail, TodoParts { calendar: true, people: false, tasks: true });
    let r = run(&mut s, "view.todoBar", json!({"part": "off"}));
    assert_eq!(r["open"], false);
    assert!(!s.view.todo_bar.mail.any());
}

#[test]
fn each_module_keeps_its_own_bar() {
    let mut s = Session::empty();
    run(&mut s, "view.todoBar", json!({"part": "tasks"}));
    run(&mut s, "view.calendar", json!({}));
    assert!(!s.view.todo_bar.of(Module::Calendar).any(), "the calendar's is closed");
    run(&mut s, "view.todoBar", json!({"part": "people"}));
    // Named, another module's bar changes without that module coming to the front.
    run(&mut s, "view.todoBar", json!({"part": "calendar", "module": "tasks"}));
    assert_eq!(s.view.module, Module::Calendar);
    assert_eq!(s.view.todo_bar.mail, TodoParts { tasks: true, ..TodoParts::default() });
    assert_eq!(s.view.todo_bar.calendar, TodoParts { people: true, ..TodoParts::default() });
    assert_eq!(s.view.todo_bar.tasks, TodoParts { calendar: true, ..TodoParts::default() });
    assert!(!s.view.todo_bar.people.any());
    // Reset View arranges the view again and leaves the bar as it is, as Outlook does.
    run(&mut s, "view.reset", json!({}));
    assert_eq!(s.view.todo_bar.mail, TodoParts { tasks: true, ..TodoParts::default() });
    // The bar is part of what View › Inspect says.
    assert_eq!(run(&mut s, "view.inspect", json!({}))["view"]["todoBar"]["calendar"]["people"], true);
}

#[test]
fn junk_is_refused_and_changes_nothing() {
    let mut s = Session::empty();
    for junk in [
        json!({}),
        json!({"part": ""}),
        json!({"part": "weather"}),
        json!({"part": 3}),
        json!({"part": null}),
        json!({"part": "tasks", "module": "notes"}),
        json!({"part": "tasks", "module": 2}),
        json!({"part": "tasks", "module": ["mail"]}),
        // A switch that is no true or false is refused, not taken as "toggle".
        json!({"part": "tasks", "value": "yes"}),
        json!({"part": "tasks", "value": "false"}),
        json!({"part": "calendar", "value": 1}),
    ] {
        assert!(s.run("view.todoBar", &junk).is_err(), "{junk}");
    }
    assert_eq!(s.view.todo_bar, crate::TodoBar::default());
}

#[test]
fn a_message_dropped_on_the_bar_becomes_a_task_where_the_owner_is() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let message = *s.mailbox.messages.keys().next().unwrap();
    let made = run(&mut s, "tasks.fromMessage", json!({"id": message, "show": false}));
    assert_eq!(made["existing"], false);
    assert_eq!(s.view.module, Module::Mail, "the mail stays in front");
    let id = made["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.task(id).and_then(|t| t.message), Some(message));
    // Without `show`, Tasks comes to the front as before.
    let other = *s.mailbox.messages.keys().find(|m| **m != message).unwrap();
    run(&mut s, "tasks.fromMessage", json!({"id": other}));
    assert_eq!(s.view.module, Module::Tasks);
}

#[test]
fn a_show_that_is_no_true_or_false_is_refused_and_makes_no_task() {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let message = *s.mailbox.messages.keys().next().unwrap();
    let tasks = s.mailbox.tasks.len();
    for junk in [json!("false"), json!(0), json!(null), json!([false])] {
        let params = json!({"id": message, "show": junk});
        assert!(s.run("tasks.fromMessage", &params).is_err(), "{params}");
        assert_eq!(s.mailbox.tasks.len(), tasks, "{params}");
        assert_eq!(s.view.module, Module::Mail, "{params}");
    }
}
