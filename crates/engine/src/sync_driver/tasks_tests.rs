// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_i18n::Lang;
use mailcraft_model::Id;
use mailcraft_model::remote_tasks::TasksDownload;
use serde_json::json;

use super::tasks::*;
use super::{Busy, Permission, fingerprint};
use crate::Session;

const NOW: i64 = 1_790_000_000;

fn session() -> (Session, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let added = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap();
    let id = added["id"].as_u64().unwrap();
    (s, id)
}

fn facts() -> TasksFacts {
    TasksFacts { reachable: true, running: false, busy: Busy::default() }
}

fn tokens(_: Id) -> Option<String> {
    Some("refresh".into())
}

fn answered(s: &mut Session, account: Id, came: &TasksCame<'_>) {
    let ran_for = s.mailbox.account(account).unwrap().clone();
    s.tasks_answered(account, 1_000.0, fingerprint("refresh"), &ran_for, Lang::English, came);
}

#[test]
fn nothing_is_due_while_the_host_cannot_or_something_else_goes_first() {
    let (mut s, a) = session();
    assert_eq!(s.tasks_due(&facts(), tokens, 1e6), vec![a]);
    assert!(s.tasks_due(&TasksFacts { reachable: false, ..facts() }, tokens, 1e6).is_empty());
    assert!(s.tasks_due(&TasksFacts { running: true, ..facts() }, tokens, 1e6).is_empty());
    let busy = Busy { signing_in: true, ..Default::default() };
    assert!(s.tasks_due(&TasksFacts { busy, ..facts() }, tokens, 1e6).is_empty());
    // Asked for: not again until the refresh time.
    s.tasks_started(a, 1e6);
    assert!(s.tasks_due(&facts(), tokens, 1e6 + 1_000.0).is_empty());
}

#[test]
fn a_sign_in_that_does_not_cover_to_dos_asks_the_owner_and_holds_the_job() {
    let (mut s, a) = session();
    answered(&mut s, a, &TasksCame { consent_needed: true, ..Default::default() });
    assert_eq!(s.driver.tasks.perm.get(&a), Some(&Permission::Needed));
    assert_eq!(s.driver.tasks.to_ask(), Some(a));
    assert!(s.tasks_blocked(a, Some("refresh")) && s.tasks_held(a));
    assert!(s.tasks_due(&facts(), tokens, 1e9).is_empty());
    s.driver.tasks.asking(a);
    s.driver.tasks.unanswered(false);
    assert_eq!(s.driver.tasks.perm.get(&a), Some(&Permission::Declined), "not asked again by itself");
}

#[test]
fn a_refusal_just_after_the_owner_allowed_is_tried_again_and_then_is_a_no() {
    let (mut s, a) = session();
    s.driver.tasks.allowed(a);
    let consent = TasksCame { consent_needed: true, ..Default::default() };
    for _ in 0..3 {
        answered(&mut s, a, &consent);
        assert_eq!(s.driver.tasks.perm.get(&a), None);
    }
    answered(&mut s, a, &consent);
    assert_eq!(s.driver.tasks.perm.get(&a), Some(&Permission::Needed));
}

#[test]
fn a_job_that_read_the_to_dos_says_they_work_and_a_dead_sign_in_waits_for_a_new_one() {
    let (mut s, a) = session();
    let download = TasksDownload { account: a, email: "ada@example.org".into(), complete: true, ..Default::default() };
    answered(&mut s, a, &TasksCame { download: Some(&download), ..Default::default() });
    assert_eq!(s.driver.tasks.perm.get(&a), Some(&Permission::Given));
    assert_eq!(s.tasks_working(), vec![a]);
    answered(&mut s, a, &TasksCame { sign_in_refused: true, ..Default::default() });
    assert!(matches!(s.driver.tasks.perm.get(&a), Some(Permission::SignInOver(_))));
    assert!(s.tasks_blocked(a, Some("refresh")), "the same sign-in is not tried again");
    assert!(!s.tasks_blocked(a, Some("a newer one")));
    s.driver.tasks.refused(a, "no".into());
    assert!(s.tasks_blocked(a, None));
}

#[test]
fn a_mailbox_that_takes_the_place_forgets_what_was_known_of_the_to_dos() {
    let (mut s, a) = session();
    s.driver.tasks.allowed(a);
    s.driver.tasks.needed(a);
    s.driver.reset();
    assert!(s.driver.tasks.perm.is_empty() && s.driver.tasks.settling.is_empty());
}

#[test]
fn an_answer_the_host_itself_found_wanting_is_a_failure_even_when_what_came_was_taken_in() {
    let (mut s, a) = session();
    let download = TasksDownload { account: a, email: "ada@example.org".into(), complete: true, ..Default::default() };
    answered(&mut s, a, &TasksCame { download: Some(&download), job_errors: true, ..Default::default() });
    assert_eq!(s.driver.tasks.book.failures(a), 1, "counted, so it is asked again after the back-off");
    answered(&mut s, a, &TasksCame { download: Some(&download), ..Default::default() });
    assert_eq!(s.driver.tasks.book.failures(a), 0);
}

#[test]
fn a_refused_sign_in_the_account_may_renew_is_a_failure_that_backs_the_job_off() {
    let (mut s, a) = session();
    s.tasks_started(a, 0.0);
    answered(&mut s, a, &TasksCame { sign_in_refused: true, new_tokens: true, ..Default::default() });
    assert_eq!(s.driver.tasks.book.failures(a), 1);
    assert!(s.tasks_due(&facts(), tokens, 10_000.0).is_empty(), "within the back-off");
    assert_eq!(s.tasks_due(&facts(), tokens, 1_100_000.0), vec![a]);
}

#[test]
fn a_success_in_the_answer_that_was_a_settling_refusal_does_not_forget_its_failure() {
    let (mut s, a) = session();
    s.driver.tasks.allowed(a);
    let download = TasksDownload { account: a, email: "ada@example.org".into(), complete: true, ..Default::default() };
    answered(&mut s, a, &TasksCame { consent_needed: true, download: Some(&download), ..Default::default() });
    assert_eq!(s.driver.tasks.book.failures(a), 1);
}

#[test]
fn a_removed_account_is_no_longer_asked_about() {
    let (mut s, a) = session();
    let other = s.run("account.add", &json!({"email": "bo@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.driver.tasks.needed(a);
    s.driver.tasks.allowed(a);
    s.driver.tasks.needed(a);
    s.driver.tasks.needed(other);
    s.run("account.remove", &json!({"id": a})).unwrap();
    assert_eq!(s.driver.tasks.to_ask(), Some(other));
    assert!(!s.driver.tasks.perm.contains_key(&a) && !s.driver.tasks.settling.contains_key(&a));
}

#[test]
fn a_late_answer_for_a_removed_account_asks_nothing() {
    let (mut s, a) = session();
    let other = s.run("account.add", &json!({"email": "bo@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.driver.tasks.needed(other);
    let ran_for = s.mailbox.account(a).unwrap().clone();
    s.run("account.remove", &json!({"id": a})).unwrap();
    let came = TasksCame { consent_needed: true, ..Default::default() };
    s.tasks_answered(a, 1_000.0, fingerprint("refresh"), &ran_for, Lang::English, &came);
    assert!(!s.driver.tasks.perm.contains_key(&a));
    assert_eq!(s.driver.tasks.to_ask(), Some(other));
}

#[test]
fn the_host_switch_and_working_offline_stop_the_jobs_that_start_by_themselves() {
    let (mut s, a) = session();
    s.driver.set_auto_sync(false);
    assert!(s.tasks_due(&facts(), tokens, 1e6).is_empty());
    s.driver.set_auto_sync(true);
    s.prefs.offline = true;
    assert!(s.tasks_due(&facts(), tokens, 1e6).is_empty());
    s.prefs.offline = false;
    assert_eq!(s.tasks_due(&facts(), tokens, 1e6), vec![a]);
}
