// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::{Id, LocalTasks};
use serde_json::json;

use super::SharedPermission;
use super::ask::*;
use crate::{Module, Session};

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(1_790_000_000, 0), 1_790_000_000, 0);
    let id = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    (s, id)
}

#[test]
fn a_question_waits_for_a_window_with_nothing_in_the_way() {
    let (mut s, a) = session();
    s.mailbox.local_tasks = LocalTasks::NotNow;
    assert_eq!(s.tasks_question(&AskWindow::default()), None);
    s.driver.tasks.needed(a);
    assert_eq!(s.tasks_question(&AskWindow::default()), Some(TasksQuestion::Permission(a)));
    for window in [
        AskWindow { dialog_open: true, ..Default::default() },
        AskWindow { backstage: true, ..Default::default() },
        AskWindow { signing_in: true, ..Default::default() },
    ] {
        assert_eq!(s.tasks_question(&window), None, "{window:?}");
    }
    s.driver.tasks.asking(a);
    assert_eq!(s.tasks_question(&AskWindow::default()), None, "asked already");
}

#[test]
fn the_local_tasks_offer_stands_until_it_is_answered() {
    let (mut s, a) = session();
    s.mailbox.local_tasks = LocalTasks::Unasked;
    assert_eq!(s.tasks_question(&AskWindow::default()), Some(TasksQuestion::LocalTasks));
    s.driver.tasks.needed(a);
    assert_eq!(s.tasks_question(&AskWindow::default()), Some(TasksQuestion::Permission(a)), "the permission first");
    s.mailbox.local_tasks = LocalTasks::NotNow;
    s.driver.tasks.allowed(a);
    assert_eq!(s.tasks_question(&AskWindow::default()), None);
}

#[test]
fn the_rules_question_waits_while_working_offline() {
    let (mut s, a) = session();
    s.driver.rules.needed(a);
    assert_eq!(s.rules_question(&AskWindow::default()), Some(a));
    assert_eq!(s.rules_question(&AskWindow { dialog_open: true, ..Default::default() }), None);
    s.prefs.offline = true;
    assert_eq!(s.rules_question(&AskWindow::default()), None);
}

#[test]
fn the_shared_calendars_question_is_put_with_the_calendar_in_front_and_only_over_no_dialog() {
    let (mut s, owner) = session();
    let shared = s.run("account.addSharedMailbox", &json!({"account": owner, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    s.driver.calendar.shared_perm.insert(shared, SharedPermission::Needed);
    s.view.module = Module::Mail;
    assert_eq!(s.shared_calendars_question(&AskWindow::default()), None);
    s.view.module = Module::Calendar;
    assert_eq!(s.shared_calendars_question(&AskWindow::default()), Some(shared));
    assert_eq!(s.shared_calendars_question(&AskWindow { dialog_open: true, ..Default::default() }), None);
    assert_eq!(s.shared_calendars_question(&AskWindow { backstage: true, ..Default::default() }), None);
    // (A sign-in doesn't stop this one, as it never did.)
    assert_eq!(s.shared_calendars_question(&AskWindow { signing_in: true, ..Default::default() }), Some(shared));
    s.driver.calendar.shared_perm.insert(shared, SharedPermission::Asking);
    assert_eq!(s.shared_calendars_question(&AskWindow::default()), None);
}
