// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::stopwatch::Stopwatch;
use std::time::Duration;

#[test]
fn a_stopwatch_counts_up_from_zero() {
    let watch = Stopwatch::start();
    let first = watch.so_far();
    assert!(first < Duration::from_secs(60), "{first:?}");
    std::thread::sleep(Duration::from_millis(5));
    let later = watch.so_far();
    assert!(later >= first && later >= Duration::from_millis(5), "{first:?} then {later:?}");
}
