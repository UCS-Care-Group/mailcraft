// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::time::{Duration, Instant};

use super::{ActiveChecker, RETRY_OPEN, prefer_bare_locale};

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

#[test]
fn a_bare_locale_prefers_the_exact_dictionary_when_the_platform_picks_a_region_first() {
    let tags = || vec!["en-GB".to_owned(), "en".to_owned()];
    let found = prefer_bare_locale("en", "en-GB".to_owned(), String::as_str, tags, |request| {
        assert_eq!(request, "en-x-mailcrft");
        Some("en".to_owned())
    });
    assert_eq!(found, "en");
    assert_eq!(prefer_bare_locale("en", "en-GB".to_owned(), String::as_str, tags, |_| Some("en-US".to_owned())), "en-GB");
    assert_eq!(prefer_bare_locale("en", "en-GB".to_owned(), String::as_str, || vec!["en-GB".into()], |_| panic!("no bare locale")), "en-GB");
    assert_eq!(prefer_bare_locale("en-GB", "en_GB".to_owned(), String::as_str, || panic!("regional locale"), |_| panic!("already exact")), "en_GB");
}
