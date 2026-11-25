// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Spelling dictionaries for the desktop window (#1797).
//!
//! Windows and macOS ask the operating system, and every platform also has the US English list
//! built into MailCraft. The system's checker is asked on the UI thread.

#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::cell::RefCell;
#[cfg(any(target_os = "windows", target_os = "macos", test))]
use std::ops::Range;
#[cfg(any(target_os = "windows", target_os = "macos", test))]
use std::time::{Duration, Instant};

use mailcraft_ui_egui::spelling::BundledChecker;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use mailcraft_ui_egui::spelling::MAX_ISSUES;
#[cfg(any(target_os = "windows", target_os = "macos", test))]
use mailcraft_ui_egui::spelling::{EN_US_TAG, HostChecker};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use spellkit::Checker;

/// The spelling host for this platform: the system's dictionaries beside the built-in list, or
/// the built-in list on its own.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub(crate) fn host() -> mailcraft_ui_egui::spelling::Host {
    Box::new(CombinedChecker::<NativeChecker>::default())
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn host() -> mailcraft_ui_egui::spelling::Host {
    Box::new(BundledChecker)
}

#[cfg(any(target_os = "windows", target_os = "macos", test))]
const RETRY_OPEN: Duration = Duration::from_secs(5);
#[cfg(any(target_os = "windows", target_os = "macos"))]
const REFRESH_LOCALES: Duration = Duration::from_secs(30);

/// The operating system's dictionaries, plus the built-in US English list.
///
/// `check` and `suggest` send the built-in US English tag to the built-in list and every other tag to `native`.
#[cfg(any(target_os = "windows", target_os = "macos", test))]
pub(crate) struct CombinedChecker<N> {
    native: N,
}

#[cfg(any(target_os = "windows", target_os = "macos", test))]
impl<N: Default> Default for CombinedChecker<N> {
    fn default() -> Self {
        Self { native: N::default() }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos", test))]
impl<N: HostChecker> HostChecker for CombinedChecker<N> {
    fn locales(&self) -> Vec<String> {
        let mut languages = self.native.locales();
        languages.push(EN_US_TAG.to_owned());
        languages.sort_unstable();
        languages.dedup();
        languages
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        if locale == EN_US_TAG { BundledChecker.check(locale, text) } else { self.native.check(locale, text) }
    }

    fn suggest(&self, locale: &str, word: &str) -> Vec<String> {
        if locale == EN_US_TAG { BundledChecker.suggest(locale, word) } else { self.native.suggest(locale, word) }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Default)]
pub(crate) struct NativeChecker {
    locales: RefCell<Option<LocaleList>>,
    active: RefCell<Option<ActiveChecker<Checker>>>,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
struct LocaleList {
    languages: Vec<String>,
    refresh_at: Instant,
}

#[cfg(any(target_os = "windows", target_os = "macos", test))]
struct ActiveChecker<T> {
    locale: String,
    state: OpenState<T>,
}

#[cfg(any(target_os = "windows", target_os = "macos", test))]
enum OpenState<T> {
    Ready(T),
    RetryAt(Instant),
}

#[cfg(any(target_os = "windows", target_os = "macos", test))]
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
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
