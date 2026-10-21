// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Interface text. Every string the window shows goes through [`t`] (the `tl!` macro) or, for a
//! count, [`tn`] (`tln!`), keyed by its English text and looked up in the catalogue of the
//! window's language (`mailcraft-i18n`). The window's language is set once a frame from the
//! owner's choice ([`set_current`]); the engine is told it through `Session::interface_language`.

use std::sync::atomic::{AtomicU8, Ordering};

pub use mailcraft_i18n::{AUTO, Lang, fmt};

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// The window's language from now on.
pub fn set_current(lang: Lang) {
    let i = Lang::ALL.iter().position(|l| *l == lang).unwrap_or(0);
    CURRENT.store(u8::try_from(i).unwrap_or(0), Ordering::Relaxed);
}

/// The window's language.
pub fn current() -> Lang {
    Lang::ALL.get(usize::from(CURRENT.load(Ordering::Relaxed))).copied().unwrap_or_default()
}

/// `s` in the interface language.
pub fn t(s: &str) -> &str {
    current().t(s)
}

/// `s` in `context` (`msgctxt`), in the interface language.
pub fn tc<'a>(context: &str, s: &'a str) -> &'a str {
    current().tc(context, s)
}

/// The form for a count of `n` of `one` / `other` (English singular and plural), in the interface
/// language, with `{n}` filled in (other placeholders are left to [`fmt`]).
pub fn tn(n: u64, one: &str, other: &str) -> String {
    fmt(current().tn(n, one, other), &[("n", &n.to_string())])
}

/// A number of things, for [`tn`]: any integer (a negative one counts as none).
pub trait Count {
    fn count(self) -> u64;
}

macro_rules! count {
    ($($t:ty),*) => {$(
        impl Count for $t {
            fn count(self) -> u64 {
                u64::try_from(self).unwrap_or(0)
            }
        }
    )*};
}
count!(u8, u16, u32, u64, usize, i32, i64, isize);

/// A command's location (`Home › Respond`) in the interface language, part by part.
pub fn location(loc: &str) -> String {
    mailcraft_i18n::location(current(), loc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_by_default() {
        // (The window's language is shared by the tests running alongside: only English here.)
        assert_eq!(t("Reply"), "Reply");
        assert_eq!(tn(1, "{n} message", "{n} messages"), "1 message");
        assert_eq!(tn(3, "{n} message", "{n} messages"), "3 messages");
        assert_eq!(location("Home › Respond"), "Home › Respond");
    }
}
