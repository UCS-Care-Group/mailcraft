// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::Mailbox;
use mailcraft_model::remote::EventsDownload;
use serde_json::json;

use super::*;
use crate::Session;

fn facts(busy: Busy) -> CalendarFacts {
    CalendarFacts { focused: true, backstage: false, reachable: true, busy }
}

#[test]
fn a_session_with_no_account_to_ask_has_nothing_due_and_the_wish_to_ask_ends_with_the_look() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    s.driver.calendar.due = true;
    let look = s.calendar_due(&facts(Busy::default()), 0.0);
    assert!(look.plans.is_empty() && look.dropped.is_empty() && look.may_ask);
    s.calendar_none_started(&look);
    assert!(!s.driver.calendar.due);
}

#[test]
fn a_look_that_is_not_enabled_or_that_waits_for_another_job_asks_for_nothing_and_does_not_ask_the_owner() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let off = CalendarFacts { reachable: false, ..facts(Busy::default()) };
    assert!(!s.calendar_due(&off, 0.0).may_ask);
    let busy = Busy { mail_run: true, ..Busy::default() };
    assert!(!s.calendar_due(&facts(busy), 0.0).may_ask);
}

#[test]
fn a_look_at_other_days_waits_for_other_work_and_for_the_owners_permission() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let day = mailcraft_time::Date::new(2026, 1, 5).unwrap();
    assert_eq!(s.calendar_around(1, day, true).unwrap_err(), AroundRefused::Busy);
    s.driver.calendar.consent_needed.insert(1);
    assert_eq!(s.calendar_around(1, day, false).unwrap_err(), AroundRefused::Consent);
    s.driver.calendar.consent_needed.clear();
    let plan = s.calendar_around(1, day, false).unwrap();
    assert!(plan.around && !plan.push && !plan.directory && !plan.settings);
    assert!(plan.window.1 > plan.window.0);
    s.calendar_started(&plan, 5.0);
    assert!(s.driver.calendar.book.checked(1).is_none(), "says nothing of the days on screen");
}

#[test]
fn a_permission_that_waits_or_was_refused_blocks_a_shared_mailboxs_calendars() {
    for (perm, blocks) in [
        (None, false),
        (Some(SharedPermission::Given), false),
        (Some(SharedPermission::Allowed), false),
        (Some(SharedPermission::Needed), true),
        (Some(SharedPermission::Asking), true),
        (Some(SharedPermission::Refused("no".into())), true),
    ] {
        assert_eq!(SharedPermission::blocks(perm.as_ref()), blocks, "{perm:?}");
    }
}

#[test]
fn an_answer_with_nothing_in_it_counts_as_a_failure_and_a_job_that_carried_changes_without_outcomes_too() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let out = s.calendar_answered(1, false, 10.0, 20.0, &CalendarCame::default());
    assert!(!out.taken);
    assert_eq!(s.driver.calendar.book.failures(1), 1);
    assert!(out.errors.is_empty() && out.status.is_empty());
}

#[test]
fn a_look_held_off_leaves_the_wish_to_ask_for_when_it_can_be_asked() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    s.driver.calendar.due = true;
    let busy = Busy { mail_run: true, ..Busy::default() };
    let look = s.calendar_due(&facts(busy), 0.0);
    s.calendar_none_started(&look);
    assert!(s.driver.calendar.due, "another job is at work");
    let off = CalendarFacts { reachable: false, ..facts(Busy::default()) };
    let look = s.calendar_due(&off, 0.0);
    s.calendar_none_started(&look);
    assert!(s.driver.calendar.due, "automatic checking is off");
}

fn microsoft_session() -> (Session, Id) {
    let mut s = Session::empty();
    let added = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap();
    let id = added["id"].as_u64().unwrap();
    (s, id)
}

#[test]
fn a_part_the_host_could_not_bring_down_asks_for_the_wide_stretch_again_at_the_next_job() {
    let (mut s, a) = microsoft_session();
    let events = EventsDownload { account: a, email: "ada@example.org".into(), complete: true, ..Default::default() };
    let came = CalendarCame { events: Some(&events), job_errors: true, ..Default::default() };
    s.driver.calendar.directory = true;
    let out = s.calendar_answered(a, false, 10.0, 20.0, &came);
    assert!(out.taken && out.errors.is_empty(), "what came was taken in");
    assert!(!s.driver.calendar.checked.contains_key(&a), "but the host's own error says a part is missing");
    assert_eq!(s.driver.calendar.book.failures(a), 1, "and the partial answer is a failure, so the next job waits out the back-off");
    s.driver.calendar.directory = true;
    s.calendar_answered(a, false, 10.0, 30.0, &CalendarCame { job_errors: false, ..came });
    assert_eq!(s.driver.calendar.checked.get(&a), Some(&30.0));
    assert_eq!(s.driver.calendar.book.failures(a), 0, "a whole answer ends the failures");
}

#[test]
fn a_partial_answer_that_carried_changes_does_not_hold_up_their_retry() {
    let (mut s, a) = microsoft_session();
    let events = EventsDownload { account: a, email: "ada@example.org".into(), complete: true, ..Default::default() };
    let came = CalendarCame { events: Some(&events), job_errors: true, ..Default::default() };
    s.driver.calendar.directory = true;
    s.calendar_answered(a, true, 10.0, 20.0, &came);
    // (The carried changes with no outcome count as one failure already, and only one.)
    assert_eq!(s.driver.calendar.book.failures(a), 1);
}

#[test]
fn the_wide_stretch_is_worked_out_at_the_ends_of_time_without_overflow() {
    let plan = CalendarPlan { account: 1, push: false, changes: None, directory: true, settings: false, window: (0, 1), around: false };
    assert_eq!(plan.range(i64::MAX).1, i64::MAX);
    assert_eq!(plan.range(i64::MIN).0, i64::MIN);
    assert_eq!(plan.range(1_000_000_000), (1_000_000_000 - WIDE_BEHIND, 1_000_000_000 + WIDE_AHEAD));
}

#[test]
fn the_owners_permission_is_kept_from_the_answer_without_a_window() {
    let (mut s, a) = microsoft_session();
    let consent = CalendarCame { consent_needed: true, ..Default::default() };
    assert!(s.calendar_answered(a, false, 1.0, 2.0, &consent).consent_asked);
    assert!(s.driver.calendar.consent_needed.contains(&a), "the next look holds the job back");
    assert!(!s.calendar_answered(a, false, 1.0, 3.0, &consent).consent_asked, "asked once");
}

#[test]
fn a_shared_mailboxs_calendar_permission_follows_what_its_jobs_say() {
    let (mut s, owner) = microsoft_session();
    let added = s.run("account.add", &json!({"email": "team@example.org", "kind": "microsoft"})).unwrap();
    let shared = added["id"].as_u64().unwrap();
    s.mailbox.accounts.iter_mut().filter(|a| a.id == shared).for_each(|a| a.shared_of = Some(owner));
    assert!(s.mailbox.account(shared).unwrap().is_shared());
    let said = |s: &mut Session, came: CalendarCame<'_>| s.calendar_answered(shared, false, 1.0, 2.0, &came);
    said(&mut s, CalendarCame { consent_needed: true, ..Default::default() });
    assert_eq!(s.driver.calendar.shared_perm.get(&shared), Some(&SharedPermission::Needed));
    assert!(s.shared_calendars_blocked(shared));
    s.driver.calendar.shared_perm.insert(shared, SharedPermission::Asking);
    said(&mut s, CalendarCame { consent_needed: true, ..Default::default() });
    assert_eq!(s.driver.calendar.shared_perm.get(&shared), Some(&SharedPermission::Asking), "the question is not undone");
    let out = said(&mut s, CalendarCame { sign_in_refused: true, ..Default::default() });
    assert_eq!(out.errors.len(), 1);
    said(&mut s, CalendarCame { token_refused: true, ..Default::default() });
    assert_ne!(s.driver.calendar.shared_perm.get(&shared), Some(&SharedPermission::Given));
    s.driver.calendar.shared_perm.clear();
    said(&mut s, CalendarCame::default());
    assert_eq!(s.driver.calendar.shared_perm.get(&shared), Some(&SharedPermission::Given));
}

#[test]
fn the_host_switch_stops_the_calendar_jobs_that_start_by_themselves() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    s.driver.set_auto_sync(false);
    s.driver.calendar.due = true;
    let look = s.calendar_due(&facts(Busy::default()), 0.0);
    assert!(look.plans.is_empty() && !look.may_ask);
}
