// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::ops::Range;
use std::time::{Duration, Instant};

use mailcraft_ui_egui::spelling::{EN_US_TAG, HostChecker};

use super::{ActiveChecker, CombinedChecker, RETRY_OPEN, prefer_bare_locale};

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

/// The operating-system half of [`CombinedChecker`]: it must not be asked for the built-in tag.
struct RouteFake;

impl HostChecker for RouteFake {
    fn locales(&self) -> Vec<String> {
        vec!["fr-FR".to_string(), "en-US".to_string(), "en-US".to_string()]
    }

    fn check(&self, locale: &str, _text: &str) -> Option<Vec<Range<usize>>> {
        assert_ne!(locale, EN_US_TAG);
        (locale == "fr-FR").then(Vec::new)
    }

    fn suggest(&self, locale: &str, word: &str) -> Vec<String> {
        assert_ne!(locale, EN_US_TAG);
        vec![format!("native:{locale}:{word}")]
    }
}

#[test]
fn the_built_in_tag_is_routed_to_the_bundled_dictionary_and_listed_after_en_us() {
    let combined = CombinedChecker { native: RouteFake };
    assert_eq!(combined.locales(), vec!["en-US".to_string(), EN_US_TAG.to_string(), "fr-FR".to_string()]);
    let marked = combined.check(EN_US_TAG, "I saw teh cat").expect("built-in dictionary");
    assert!(marked.iter().any(|range| range == &(6..9)), "{marked:?}");
    assert_eq!(combined.check("fr-FR", "teh"), Some(Vec::new()));
    assert_eq!(combined.check("de-DE", "teh"), None);
    assert!(combined.suggest(EN_US_TAG, "teh").iter().any(|word| word == "the"));
    assert_eq!(combined.suggest("fr-FR", "teh"), vec!["native:fr-FR:teh".to_string()]);
}
