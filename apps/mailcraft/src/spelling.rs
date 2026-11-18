// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Use the operating system's local dictionaries on the UI thread.

use std::cell::{OnceCell, RefCell};
use std::ops::Range;

use mailcraft_ui_egui::spelling::HostChecker;
use spellkit::Checker;

#[derive(Default)]
pub(crate) struct NativeChecker {
    locales: OnceCell<Vec<String>>,
    active: RefCell<Option<ActiveChecker>>,
}

struct ActiveChecker {
    locale: String,
    checker: Option<Checker>,
}

impl NativeChecker {
    fn with_checker<T>(&self, locale: &str, use_checker: impl FnOnce(&Checker) -> T) -> Option<T> {
        let mut active = self.active.try_borrow_mut().ok()?;
        if active.as_ref().is_none_or(|current| current.locale != locale) {
            let checker = if locale.is_empty() { Checker::new() } else { Checker::with_locale(locale) };
            *active = Some(ActiveChecker { locale: locale.to_owned(), checker: checker.ok() });
        }
        active.as_ref()?.checker.as_ref().map(use_checker)
    }
}

impl HostChecker for NativeChecker {
    fn locales(&self) -> Vec<String> {
        self.locales
            .get_or_init(|| {
                let mut locales = Checker::available_locales();
                locales.sort();
                locales.dedup();
                locales
            })
            .clone()
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        self.with_checker(locale, |checker| checker.check(text).map(|error| error.range()).collect())
    }

    fn suggest(&self, locale: &str, word: &str) -> Vec<String> {
        self.with_checker(locale, |checker| checker.suggest(word)).unwrap_or_default()
    }
}
