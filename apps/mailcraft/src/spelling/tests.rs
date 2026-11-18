// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::time::{Duration, Instant};

use super::{ActiveChecker, RETRY_OPEN};

#[test]
fn reopens_a_failed_checker_after_a_delay_and_keeps_a_successful_one() {
    let mut active = None;
    let now = Instant::now();
    assert_eq!(ActiveChecker::open_if_needed(&mut active, "en-US", now, || None), None);
    assert_eq!(ActiveChecker::open_if_needed(&mut active, "en-US", now + Duration::from_secs(1), || panic!("too early")), None);
    assert_eq!(ActiveChecker::open_if_needed(&mut active, "en-US", now + RETRY_OPEN, || Some(1)), Some(&1));
    assert_eq!(ActiveChecker::open_if_needed(&mut active, "en-US", now + RETRY_OPEN * 2, || panic!("already open")), Some(&1));
    assert_eq!(ActiveChecker::open_if_needed(&mut active, "it-IT", now + RETRY_OPEN * 2, || Some(2)), Some(&2));
}
