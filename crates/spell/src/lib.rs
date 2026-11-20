// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A clean-room spelling checker for the Hunspell `.aff` / `.dic` file format, and the
//! US English word list bundled with MailCraft.
//!
//! The reader follows the format described by the hunspell(5) manual page (directives,
//! single-character flags, prefix and suffix rules, compound patterns). It does not share
//! code with another spell checker. Suggestions are scored by our own edit distance.
//!
//! Nothing here panics on hostile input: a bad file yields [`Error`], and [`en_us`]
//! yields `None` if the embedded list cannot be read.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::ops::Range;
use std::sync::OnceLock;

mod affix;
mod dict;
mod error;
mod fold;
mod scan;
mod suggest;

pub use dict::Dictionary;
pub use error::Error;

/// How many misspellings a host should report before calling the rest partial.
/// A host may return one extra range (`MAX_ISSUES + 1`) so the caller can tell.
pub const MAX_ISSUES: usize = 256;

/// The SCOWL release the bundled `en_US` list was cut from.
pub const EN_US_VERSION: &str = "2026.02.25";

/// Locale tag for the dictionary compiled into this crate.
pub const EN_US_TAG: &str = "en-US-x-builtin";

const EN_US_AFF: &str = include_str!("../../../assets/dictionaries/en_US/en_US.aff");
const EN_US_DIC: &str = include_str!("../../../assets/dictionaries/en_US/en_US.dic");

/// A spelling dictionary a host can ask, without MailCraft knowing where the words come from.
///
/// `check` returns byte ranges of misspelt words. `None` means this host has no dictionary
/// for `locale`. A host may return one range past [`MAX_ISSUES`] to mark a partial report.
pub trait HostChecker {
    /// Locales this host can check, as language tags.
    fn locales(&self) -> Vec<String>;
    /// Byte ranges of misspelt words in `text`, or `None` when `locale` is not available.
    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>>;
    /// Spellings to offer for `word`. Empty when `locale` is not available.
    fn suggest(&self, locale: &str, word: &str) -> Vec<String>;
}

/// The US English dictionary embedded in this crate.
///
/// Built on the first call. `None` if those files do not parse, which does not panic.
pub fn en_us() -> Option<&'static Dictionary> {
    static CELL: OnceLock<Option<Dictionary>> = OnceLock::new();
    CELL.get_or_init(|| Dictionary::parse(EN_US_AFF, EN_US_DIC).ok()).as_ref()
}

/// [`HostChecker`] for the bundled US English list and no other locale.
pub struct BuiltIn;

impl HostChecker for BuiltIn {
    fn locales(&self) -> Vec<String> {
        vec![EN_US_TAG.to_owned()]
    }

    fn check(&self, locale: &str, text: &str) -> Option<Vec<Range<usize>>> {
        if locale != EN_US_TAG {
            return None;
        }
        Some(en_us()?.check_text(text, MAX_ISSUES.saturating_add(1)))
    }

    fn suggest(&self, locale: &str, word: &str) -> Vec<String> {
        if locale != EN_US_TAG {
            return Vec::new();
        }
        en_us().map(|dict| dict.suggest(word, &[])).unwrap_or_default()
    }
}

#[cfg(test)]
mod lookup_tests;
#[cfg(test)]
mod parse_tests;
#[cfg(test)]
mod prop_tests;
#[cfg(test)]
mod scan_tests;
#[cfg(test)]
mod suggest_tests;
