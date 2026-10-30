// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Account list in the task window (#632).

use mailcraft_engine::Session;
use mailcraft_model::Id;
use serde_json::{Value, json};

use super::task::TaskForm;

const NOW: i64 = 1_791_993_600;

fn session() -> (Session, Id, Id) {
    let mut s = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let add = |s: &mut Session, email: &str| {
        s.run("account.add", &json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"})).unwrap()["id"].as_u64().unwrap()
    };
    let (work, home) = (add(&mut s, "work@example.org"), add(&mut s, "home@example.org"));
    (s, work, home)
}

/// What OK would run for a new task.
fn account_sent(s: &Session, params: Value) -> Option<Value> {
    TaskForm::new(s, &params).command().1.get("account").cloned()
}

#[test]
fn a_new_task_opens_on_the_place_it_would_go() {
    let (mut s, work, home) = session();
    // Nobody has said: nothing is said for them.
    assert_eq!(account_sent(&s, json!({"title": "x"})), None);
    s.run("view.taskAccount", &json!({"value": work})).unwrap();
    assert_eq!(account_sent(&s, json!({"title": "x"})), Some(json!(work)));
    s.run("view.taskAccount", &json!({"value": "here"})).unwrap();
    assert_eq!(account_sent(&s, json!({"title": "x"})), Some(json!(0)));
    // What opened it can name another.
    assert_eq!(account_sent(&s, json!({"title": "x", "account": home})), Some(json!(home)));
}

#[test]
fn a_task_that_exists_stays_where_it_is() {
    let (mut s, work, _) = session();
    s.run("tasks.new", &json!({"title": "Stays", "account": work})).unwrap();
    let (command, params) = TaskForm::edit(&s, &json!({})).unwrap().command();
    assert_eq!(command, "tasks.edit");
    assert_eq!(params.get("account"), None);
}
