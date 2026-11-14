// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::offline::*;

#[test]
fn thinning_is_due_at_the_first_look_at_a_new_opening_once_a_day_and_when_asked() {
    let mut t = Thinning::default();
    assert!(t.due(1, 0.0), "before the first");
    t.started(1, 1_000.0);
    assert!(!t.due(1, 2_000.0));
    assert!(t.due(2, 2_000.0), "another mailbox was opened");
    assert!(!t.due(1, 1_000.0 + DAY_MS - 1.0));
    assert!(t.due(1, 1_000.0 + DAY_MS));
    t.now = true;
    assert!(t.due(1, 2_000.0), "more to do");
    t.started(1, 2_000.0);
    assert!(!t.now);
}

#[test]
fn a_clock_that_went_back_is_not_a_reason_and_not_a_crash() {
    let mut t = Thinning::default();
    t.started(1, 5.0 * DAY_MS);
    assert!(!t.due(1, 0.0));
    assert!(!t.due(1, f64::NAN));
}

#[test]
fn a_session_with_no_account_that_has_a_window_does_nothing_and_does_not_ask_again_the_same_day() {
    let mut s = crate::Session::new(mailcraft_model::Mailbox::with_local_account("", ""));
    assert_eq!(s.offline_tick(0.0), Thinned::Idle);
    assert!(!s.driver.thinning.due(s.openings(), 1.0));
}
