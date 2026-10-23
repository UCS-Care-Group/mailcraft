// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The system's language, as a BCP 47 tag (`nl-NL`), for an interface language left on Automatic.
//!
//! - **Linux, FreeBSD and the other Unix systems:** the first of `LC_ALL`, `LC_MESSAGES` and
//!   `LANG` that is set and not empty (the order POSIX gives for the messages category); the
//!   encoding and modifier are dropped (`nl_NL.UTF-8@euro` is `nl-NL`). The `C` and `POSIX`
//!   locales name no language.
//! - **Windows:** the user's locale name in the registry,
//!   `HKEY_CURRENT_USER\Control Panel\International\LocaleName` (already a BCP 47 tag).
//! - **macOS:** the first of `NSLocale`'s preferred languages.
//!
//! Anything that does not look like a language tag is `None`, so a hostile environment can only
//! mean English.

/// The system's language tag, if it says one.
pub(crate) fn system_locale() -> Option<String> {
    #[cfg(windows)]
    {
        windows_locale()
    }
    #[cfg(target_os = "macos")]
    {
        macos_locale()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        from_environment(|name| std::env::var_os(name).and_then(|v| v.into_string().ok()))
    }
}

/// The tag from the three variables, read through `get` (so a test can give it any environment).
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
pub(crate) fn from_environment(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let value = ["LC_ALL", "LC_MESSAGES", "LANG"].into_iter().find_map(|name| get(name).filter(|v| !v.trim().is_empty()))?;
    tag(&value)
}

/// A locale name (`nl_NL.UTF-8@euro`, `en-GB`, `C`) as a BCP 47 tag, or `None` for the `C` and
/// `POSIX` locales and for anything that is not a language with an optional region or script.
pub(crate) fn tag(name: &str) -> Option<String> {
    let name = name.trim();
    let name = name.split(['.', '@']).next().unwrap_or_default();
    if name.is_empty() || name.eq_ignore_ascii_case("c") || name.eq_ignore_ascii_case("posix") {
        return None;
    }
    let tag = name.replace('_', "-");
    let mut parts = tag.split('-');
    let language = parts.next().unwrap_or_default();
    let language_ok = (2..=3).contains(&language.len()) && language.bytes().all(|b| b.is_ascii_alphabetic());
    let rest_ok = parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()));
    (language_ok && rest_ok && tag.len() <= 35).then_some(tag)
}

#[cfg(windows)]
fn windows_locale() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(r"Control Panel\International", KEY_READ).ok()?;
    let name: String = key.get_value("LocaleName").ok()?;
    tag(&name)
}

#[cfg(target_os = "macos")]
fn macos_locale() -> Option<String> {
    use objc2_foundation::NSLocale;
    let first = NSLocale::preferredLanguages().firstObject()?;
    tag(&first.to_string())
}

#[cfg(test)]
#[path = "system_locale_tests.rs"]
mod system_locale_tests;
