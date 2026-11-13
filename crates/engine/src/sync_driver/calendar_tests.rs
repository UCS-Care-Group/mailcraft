// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use mailcraft_model::remote::{EventOp, EventsUpload};

use super::calendar::*;
use super::{CALENDAR_DIRECTORY_MS, CALENDAR_SETTINGS_MS, Pacing};

fn upload(n: usize) -> EventsUpload {
    EventsUpload { ops: (0..n).map(|_| EventOp::default()).collect(), ..Default::default() }
}

#[test]
fn coming_back_to_the_calendar_in_a_focused_window_asks_at_once() {
    let mut d = CalendarDriver::default();
    d.looked(true, false);
    assert!(!d.due, "not in front");
    d.looked(true, true);
    assert!(d.due, "switched to the calendar");
    d.due = false;
    d.looked(true, true);
    assert!(!d.due, "nothing came back");
    d.looked(false, true);
    d.looked(true, true);
    assert!(d.due, "the window came back to the front");
}

#[test]
fn a_change_goes_at_once_but_the_very_same_one_not_again_within_the_wait() {
    let mut d = CalendarDriver::default();
    assert!(d.quiet_over(1, 1, 0.0), "no quiet time");
    assert!(!d.quiet_over(1, 0, 1.0), "nothing waits");
    let up = upload(1);
    assert!(d.push_wanted(1, Some(&up), 0.0));
    assert!(!d.push_wanted(1, None, 0.0));
    d.sent_changes(1, &up, 0.0);
    assert!(!d.push_wanted(1, Some(&up), Pacing::CALENDAR.first_retry_ms - 1.0));
    assert!(d.push_wanted(1, Some(&up), Pacing::CALENDAR.first_retry_ms));
    assert!(d.push_wanted(1, Some(&upload(2)), 1.0), "another change is not the same");
    d.connected(1);
    assert!(d.push_wanted(1, Some(&up), 1.0), "Connect forgets what was sent");
}

#[test]
fn the_days_are_asked_for_by_the_clock_and_at_once_only_while_in_front() {
    let mut d = CalendarDriver::default();
    // Never asked: due.
    assert!(d.refresh_due(1, false, false, false, false, 0.0));
    d.started(1, 0.0, Some((0, 10)), false, false);
    assert!(!d.refresh_due(1, false, false, false, false, 1_000.0));
    assert!(d.refresh_due(1, false, false, false, false, Pacing::CALENDAR.refresh_ms));
    assert!(d.refresh_due(1, true, false, false, false, super::CALENDAR_IN_FRONT_MS));
    assert!(d.refresh_due(1, true, true, false, false, super::CALENDAR_RECHECK_MS), "a change not seen yet");
    assert!(!d.refresh_due(1, false, false, true, false, 1_000.0), "other days only count in front");
    assert!(d.refresh_due(1, true, false, true, false, 1_000.0));
    assert!(d.refresh_due(1, false, false, false, true, 1_000.0), "the wide stretch");
    d.due = true;
    assert!(d.refresh_due(1, true, false, false, false, 1_000.0));
    assert!(!d.refresh_due(1, false, false, false, false, 1_000.0));
}

#[test]
fn a_job_for_other_days_says_nothing_of_when_the_days_on_screen_were_asked_for() {
    let mut d = CalendarDriver::default();
    d.due = true;
    d.started(1, 5.0, None, true, true);
    assert!(d.directory && d.due && d.book.checked(1).is_none());
    assert!(!d.settings_due(1, 5.0 + CALENDAR_SETTINGS_MS - 1.0));
    assert!(d.settings_due(1, 5.0 + CALENDAR_SETTINGS_MS));
    d.started(1, 6.0, Some((1, 2)), false, false);
    assert!(!d.due && d.book.checked(1) == Some(6.0));
    assert!(d.elsewhere(1, true, (1, 3)) && !d.elsewhere(1, true, (1, 2)) && !d.elsewhere(1, false, (1, 3)));
}

#[test]
fn a_server_that_does_not_answer_is_given_longer_each_time_and_an_answer_ends_it() {
    let mut d = CalendarDriver::default();
    let p = Pacing::CALENDAR;
    d.answered(1, true, 0.0);
    assert!(!d.book.patient(&p, 1, p.first_retry_ms - 1.0));
    assert!(d.book.patient(&p, 1, p.first_retry_ms));
    d.answered(1, true, 100.0);
    assert!(!d.book.patient(&p, 1, 100.0 + p.first_retry_ms));
    d.answered(1, false, 200.0);
    assert!(d.book.patient(&p, 1, 200.0));
    d.answered(1, true, 300.0);
}

#[test]
fn the_wide_stretch_is_due_every_quarter_of_an_hour_and_at_start_up() {
    assert!(CalendarDriver::directory_due(None, 0.0));
    assert!(!CalendarDriver::directory_due(Some(0.0), CALENDAR_DIRECTORY_MS));
    assert!(CalendarDriver::directory_due(Some(0.0), CALENDAR_DIRECTORY_MS + 1.0));
}

#[test]
fn held_changes_are_told_once_for_as_long_as_they_stay_so() {
    let mut d = CalendarDriver::default();
    let held = vec![(7, "Rota".to_string(), 2)];
    assert_eq!(d.tell_held(held.clone()), [("Rota".to_string(), 2)]);
    assert!(d.tell_held(held).is_empty());
    assert_eq!(d.tell_held(vec![(7, "Rota".into(), 3)]).len(), 1, "another count");
    assert!(d.tell_held(Vec::new()).is_empty());
    assert_eq!(d.tell_held(vec![(7, "Rota".into(), 3)]).len(), 1, "told again once it had gone");
}

#[test]
fn the_waiting_changes_are_looked_over_once_for_each_state_of_the_mailbox() {
    let mut d = CalendarDriver::default();
    assert!(d.settle_pending(3) && d.shared_pending(3));
    d.settled(3);
    d.shared_done(3);
    assert!(!d.settle_pending(3) && !d.shared_pending(3));
    assert!(d.settle_pending(4));
}

#[test]
fn the_calendar_window_is_at_least_a_day_and_the_asks_of_an_empty_mailbox_are_none() {
    let s = crate::Session::new(mailcraft_model::Mailbox::with_local_account("", ""));
    let (from, to) = s.calendar_window();
    assert!(to - from >= mailcraft_time::DAY);
    let day = mailcraft_time::Date::new(2026, 10, 21).unwrap();
    let (from, to) = s.days_as_instants(day, day);
    assert_eq!(to - from, mailcraft_time::DAY);
    assert!(s.calendar_asks(999, true, None).is_empty());
    assert!(s.calendar_changes(999).is_none());
    assert!(s.microsoft_calendar_accounts().is_empty());
}

#[test]
fn consent_wait_says_whose_permission() {
    assert!(consent_wait(true).contains("shared mailboxes"));
    assert!(consent_wait(false).contains("Connect calendar and contacts"));
}
