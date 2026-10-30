// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Account list in the task window (#632).

use mailcraft_engine::Session;
use mailcraft_model::{Category, Id, Rgb};
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

#[test]
fn another_account_counts_as_a_change_to_the_form() {
    let (s, work, _) = session();
    let mut form = TaskForm::new(&s, &json!({"title": "Same"}));
    assert!(!form.dirty());
    form.place = Some(work);
    assert!(form.dirty(), "closing the form asks before that choice is lost");
}

#[test]
fn the_categories_on_offer_follow_the_account_list() {
    let (mut s, work, home) = session();
    let list = |name: &str| vec![Category { name: name.into(), color: Rgb(1, 2, 3) }];
    s.mailbox.account_categories.insert(work, list("Clients"));
    s.mailbox.account_categories.insert(home, list("Garden"));
    let mut form = TaskForm::new(&s, &json!({"title": "x", "account": work}));
    let offered = |form: &TaskForm, s: &Session| s.mailbox.categories_of(form.category_account()).first().map(|c| c.name.clone());
    assert_eq!(offered(&form, &s).as_deref(), Some("Clients"));
    form.place = Some(home);
    assert_eq!(offered(&form, &s).as_deref(), Some("Garden"));
    form.place = None;
    assert_eq!(form.category_account(), 0, "the list every task on this computer uses");
    // A task that exists keeps the list of the account it is in.
    let id = s.run("tasks.new", &json!({"title": "Stays", "account": home})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(TaskForm::edit(&s, &json!({"id": id})).unwrap().category_account(), home);
}
