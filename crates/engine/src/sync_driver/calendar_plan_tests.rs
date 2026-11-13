// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::Mailbox;

use super::*;
use crate::Session;

fn facts(busy: Busy) -> CalendarFacts {
    CalendarFacts { focused: true, backstage: false, enabled: true, busy }
}

#[test]
fn a_session_with_no_account_to_ask_has_nothing_due_and_the_wish_to_ask_ends_with_the_look() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    s.driver.calendar.due = true;
    let look = s.calendar_due(&facts(Busy::default()), 0.0);
    assert!(look.plans.is_empty() && look.dropped.is_empty() && look.may_ask);
    s.calendar_none_started();
    assert!(!s.driver.calendar.due);
}

#[test]
fn a_look_that_is_not_enabled_or_that_waits_for_another_job_asks_for_nothing_and_does_not_ask_the_owner() {
    let mut s = Session::new(Mailbox::with_local_account("", ""));
    let off = CalendarFacts { enabled: false, ..facts(Busy::default()) };
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
