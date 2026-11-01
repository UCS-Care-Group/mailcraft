// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each account's tasks apart in Tasks (#632).

use egui_kittest::Harness;
use serde_json::json;

use super::*;
use crate::people_view::testing;

fn add(w: &mut Harness<'_, MailApp>, email: &str) -> Id {
    let params = json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"});
    w.state_mut().execute("account.add", params).unwrap()["id"].as_u64().unwrap()
}

#[test]
fn the_counts_and_the_list_follow_the_choice() {
    let mut w = testing::window("view.tasks");
    let work = add(&mut w, "work@example.org");
    let ctx = w.state().ctx.clone().unwrap();
    w.state_mut().execute("view.taskFilter", json!({"value": "all"})).unwrap();
    let everything = lists(w.state(), &ctx).counts[4];
    let params = json!({"title": "For work", "account": work});
    w.state_mut().execute("tasks.new", params).unwrap();
    w.state_mut().execute("view.taskAccount", json!({"value": work})).unwrap();
    let one = lists(w.state(), &ctx);
    assert_eq!((one.counts[4], listed(&one.layout)), (1, 1));
    w.state_mut().execute("view.taskAccount", json!({"value": "here"})).unwrap();
    assert_eq!(lists(w.state(), &ctx).counts[4], everything);
    w.state_mut().execute("view.taskAccount", json!({"value": "all"})).unwrap();
    assert_eq!(lists(w.state(), &ctx).counts[4], everything + 1);
    // Every way of showing them paints.
    w.run();
}

#[test]
fn the_field_above_the_list_says_where_a_task_goes() {
    let mut w = testing::window("view.tasks");
    // With no account that keeps tasks there is nothing to say.
    assert_eq!(add_hint(w.state(), false), "Add a task");
    let work = add(&mut w, "work@example.org");
    assert_eq!(add_hint(w.state(), false), "Add a task on this computer for now");
    assert_eq!(add_hint(w.state(), true), "Add a task due today on this computer for now");
    w.state_mut().execute("view.taskAccount", json!({"value": work})).unwrap();
    assert_eq!(add_hint(w.state(), false), "Add a task to work@example.org");
    assert_eq!(add_hint(w.state(), true), "Add a task due today to work@example.org");
    // An account that hasn't let tasks through yet: they wait here, and the field says so.
    w.state_mut().tasks.perm.insert(work, crate::task_sync::Permission::Needed);
    let waits = "Add a task to work@example.org (it waits here until To Do is allowed)";
    assert_eq!(add_hint(w.state(), false), waits);
    w.state_mut().tasks.perm.insert(work, crate::task_sync::Permission::Given);
    assert_eq!(add_hint(w.state(), false), "Add a task to work@example.org");
}

#[test]
fn the_choice_is_remembered_with_the_other_view_choices() {
    let mut w = testing::window("view.tasks");
    let work = add(&mut w, "work@example.org");
    w.state_mut().execute("view.taskAccount", json!({"value": work})).unwrap();
    let saved = serde_json::to_value(&w.state().session.view).unwrap();
    let back: mailcraft_engine::ViewState = serde_json::from_value(saved).unwrap();
    assert_eq!(back.task_account, AccountChoice::Account(work));
}
