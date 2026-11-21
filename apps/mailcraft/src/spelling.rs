// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Use the operating system's local dictionaries on the UI thread.

use std::cell::RefCell;
use std::ops::Range;
use std::time::{Duration, Instant};

use mailcraft_ui_egui::spelling::{HostChecker, MAX_ISSUES};
use spellkit::Checker;

const RETRY_OPEN: Duration = Duration::from_secs(5);
const REFRESH_LOCALES: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(crate) struct NativeChecker {
    locales: RefCell<Option<LocaleList>>,
    active: RefCell<Option<ActiveChecker<Checker>>>,
}

struct LocaleList {
    languages: Vec<String>,
    refresh_at: Instant,
}

struct ActiveChecker<T> {
    locale: String,
    state: OpenState<T>,
}

enum OpenState<T> {
    Ready(T),
    RetryAt(Instant),
}

impl<T> ActiveChecker<T> {
    fn open_if_needed<'a>(active: &'a mut Option<Self>, locale: &str, now: Instant, open: impl FnOnce() -> Option<T>) -> Option<&'a T> {
        if active.as_ref().is_none_or(|current| current.locale != locale || matches!(current.state, OpenState::RetryAt(retry) if now >= retry)) {
            let state = open().map_or(OpenState::RetryAt(now + RETRY_OPEN), OpenState::Ready);
            *active = Some(Self { locale: locale.to_owned(), state });
        }
        match &active.as_ref()?.state {
            OpenState::Ready(checker) => Some(checker),
            OpenState::RetryAt(_) => None,
        }
    }
}

impl NativeChecker {
    fn with_checker<T>(&self, locale: &str, use_checker: impl FnOnce(&Checker) -> T) -> Option<T> {
        let mut active = self.active.try_borrow_mut().ok()?;
        ActiveChecker::open_if_needed(&mut active, locale, Instant::now(), || self.open_checker(locale)).map(use_checker)
    }

    fn open_checker(&self, locale: &str) -> Option<Checker> {
        if locale.is_empty() {
            return Checker::new().ok();
        }
        let checker = Checker::with_locale(locale).ok()?;
        #[cfg(target_os = "macos")]
        let checker = prefer_bare_locale(locale, checker, Checker::locale, || self.locales(), |tag| Checker::with_locale(tag).ok());
        Some(checker)
    }
}

#[cfg(any(target_os = "macos", test))]
fn prefer_bare_locale<T>(
    requested: &str,
    current: T,
    locale_of: impl Fn(&T) -> &str,
    installed: impl FnOnce() -> Vec<String>,
    open: impl FnOnce(&str) -> Option<T>,
) -> T {
    if requested.contains(['-', '_']) || locale_of(&current).eq_ignore_ascii_case(requested) {
        return current;
    }
    if !installed().iter().any(|tag| tag.eq_ignore_ascii_case(requested)) {
        return current;
    }
    // Spellkit 0.4.2 checks exact tags and their prefixes in one pass on macOS. A private-use
    // suffix bypasses the earlier regional match and reaches its language-only fallback.
    open(&format!("{requested}-x-mailcrft")).filter(|checker| locale_of(checker).eq_ignore_ascii_case(requested)).unwrap_or(current)
}

impl HostChecker for NativeChecker {
    fn locales(&self) -> Vec<String> {
        let mut cached = self.locales.borrow_mut();
        if cached.as_ref().is_none_or(|list| Instant::now() >= list.refresh_at) {
            let mut languages = Checker::available_locales();
            languages.sort_unstable();
            languages.dedup();
            let refresh = if languages.is_empty() { RETRY_OPEN } else { REFRESH_LOCALES };
            *cached = Some(LocaleList { languages, refresh_at: Instant::now() + refresh });
        }
        cached.as_ref().map_or_else(Vec::new, |list| list.languages.clone())
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        self.with_checker(locale, |checker| checker.check(text).take(MAX_ISSUES + 1).map(|error| error.range()).collect())
    }

    fn suggest(&self, locale: &str, word: &str) -> Vec<String> {
        self.with_checker(locale, |checker| checker.suggest(word)).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests;
