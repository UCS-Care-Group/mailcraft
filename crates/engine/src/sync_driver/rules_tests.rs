// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_i18n::Lang;
use mailcraft_model::Id;
use mailcraft_model::remote_rules::RulesDownload;
use serde_json::json;

use super::rules::*;
use super::{Busy, Permission, fingerprint};
use crate::Session;

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap();
    let id = added["id"].as_u64().unwrap();
    (s, id)
}

fn facts() -> RulesFacts {
    RulesFacts { reachable: true, running: false, busy: Busy::default() }
}

fn tokens(_: Id) -> Option<String> {
    Some("refresh".into())
}

fn answered(s: &mut Session, account: Id, came: &RulesCame<'_>) {
    s.rules_answered(account, 1_000.0, fingerprint("refresh"), Lang::English, came);
}

#[test]
fn rules_are_due_once_and_read_as_ever_while_the_permission_is_missing() {
    let (mut s, a) = session();
    assert_eq!(s.rules_due(&facts(), tokens, 1e6), vec![RulesPlan { account: a, writes: true }]);
    assert!(s.rules_due(&RulesFacts { reachable: false, ..facts() }, tokens, 1e6).is_empty());
    assert!(s.rules_due(&RulesFacts { running: true, ..facts() }, tokens, 1e6).is_empty());
    s.rules_started(a, 1e6);
    assert!(s.rules_due(&facts(), tokens, 1e6 + 1_000.0).is_empty());
    // Not allowed to change them yet: the job only reads.
    s.driver.rules.needed(a);
    assert!(s.rules_held(a));
    assert_eq!(s.rules_due(&facts(), tokens, 1e9), vec![RulesPlan { account: a, writes: false }]);
}

#[test]
fn a_failure_waits_out_its_back_off_before_the_next_look_and_then_the_refresh_time() {
    let (mut s, a) = session();
    s.rules_started(a, 0.0);
    answered(&mut s, a, &RulesCame::default());
    assert!(s.rules_due(&facts(), tokens, 10_000.0).is_empty(), "within the back-off");
    // Past the back-off but not the refresh time: nothing waits, so nothing is asked.
    assert!(s.rules_due(&facts(), tokens, 40_000.0).is_empty());
    assert_eq!(s.rules_due(&facts(), tokens, 400_000.0).len(), 1);
}

#[test]
fn a_sign_in_that_does_not_cover_changing_the_rules_asks_the_owner() {
    let (mut s, a) = session();
    answered(&mut s, a, &RulesCame { consent_needed: true, ..Default::default() });
    assert_eq!(s.driver.rules.to_ask(), Some(a));
    s.driver.rules.asking(a);
    s.driver.rules.unanswered(false);
    assert_eq!(s.driver.rules.perm.get(&a), Some(&Permission::Declined));
    s.driver.rules.allowed(a);
    assert_eq!(s.driver.rules.perm.get(&a), None);
    assert_eq!(s.driver.rules.settling.get(&a), Some(&super::SETTLING_TRIES));
}

#[test]
fn what_was_read_is_noted_and_a_dead_sign_in_waits_for_a_new_one() {
    let (mut s, a) = session();
    let download = RulesDownload { account: a, email: "ada@example.org".into(), complete: true, ..Default::default() };
    answered(&mut s, a, &RulesCame { download: Some(&download), ..Default::default() });
    assert!(s.driver.rules.read.contains(&a));
    answered(&mut s, a, &RulesCame { unreadable: true, ..Default::default() });
    assert!(s.driver.rules.unreadable.contains(&a));
    answered(&mut s, a, &RulesCame { sign_in_refused: true, ..Default::default() });
    assert!(matches!(s.driver.rules.perm.get(&a), Some(Permission::SignInOver(_))));
    assert!(s.rules_signed_out(a, Some("refresh")) && !s.rules_signed_out(a, Some("newer")));
    assert!(s.rules_due(&facts(), tokens, 1e9).is_empty());
}

#[test]
fn an_answer_the_host_itself_found_wanting_is_a_failure_even_when_what_came_was_taken_in() {
    let (mut s, a) = session();
    let download = RulesDownload { account: a, email: "ada@example.org".into(), complete: true, ..Default::default() };
    answered(&mut s, a, &RulesCame { download: Some(&download), job_errors: true, ..Default::default() });
    assert_eq!(s.driver.rules.book.failures(a), 1);
    answered(&mut s, a, &RulesCame { download: Some(&download), ..Default::default() });
    assert_eq!(s.driver.rules.book.failures(a), 0);
}

#[test]
fn a_refused_sign_in_the_account_may_renew_is_a_failure_that_backs_the_job_off() {
    let (mut s, a) = session();
    s.rules_started(a, 0.0);
    answered(&mut s, a, &RulesCame { sign_in_refused: true, new_tokens: true, ..Default::default() });
    assert_eq!(s.driver.rules.book.failures(a), 1);
    assert!(s.rules_due(&facts(), tokens, 10_000.0).is_empty(), "within the back-off");
    assert_eq!(s.rules_due(&facts(), tokens, 1_100_000.0).len(), 1);
}

#[test]
fn a_success_in_the_answer_that_was_a_settling_refusal_does_not_forget_its_failure() {
    let (mut s, a) = session();
    s.driver.rules.allowed(a);
    let download = RulesDownload { account: a, email: "ada@example.org".into(), complete: true, ..Default::default() };
    answered(&mut s, a, &RulesCame { consent_needed: true, download: Some(&download), ..Default::default() });
    assert_eq!(s.driver.rules.book.failures(a), 1);
}

#[test]
fn a_removed_account_is_no_longer_asked_about() {
    let (mut s, a) = session();
    let other = s.run("account.add", &json!({"email": "bo@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.driver.rules.allowed(a);
    s.driver.rules.needed(a);
    s.driver.rules.needed(other);
    s.run("account.remove", &json!({"id": a})).unwrap();
    assert_eq!(s.driver.rules.to_ask(), Some(other));
    assert!(!s.driver.rules.perm.contains_key(&a) && !s.driver.rules.settling.contains_key(&a));
}

#[test]
fn a_late_answer_for_a_removed_account_asks_nothing() {
    let (mut s, a) = session();
    let other = s.run("account.add", &json!({"email": "bo@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.driver.rules.needed(other);
    s.run("account.remove", &json!({"id": a})).unwrap();
    answered(&mut s, a, &RulesCame { consent_needed: true, ..Default::default() });
    assert!(!s.driver.rules.perm.contains_key(&a));
    assert_eq!(s.driver.rules.to_ask(), Some(other));
}

#[test]
fn the_host_switch_and_working_offline_stop_the_jobs_that_start_by_themselves() {
    let (mut s, a) = session();
    s.driver.set_auto_sync(false);
    assert!(s.rules_due(&facts(), tokens, 1e6).is_empty());
    s.driver.set_auto_sync(true);
    s.prefs.offline = true;
    assert!(s.rules_due(&facts(), tokens, 1e6).is_empty());
    s.prefs.offline = false;
    assert_eq!(s.rules_due(&facts(), tokens, 1e6), vec![RulesPlan { account: a, writes: true }]);
}
