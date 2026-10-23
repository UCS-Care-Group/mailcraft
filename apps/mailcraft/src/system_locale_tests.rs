// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The system's language from the environment's variables, and from locale names.

use super::*;
use std::collections::HashMap;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |name| map.get(name).cloned()
}

#[test]
fn encoding_and_modifier_are_dropped() {
    assert_eq!(tag("nl_NL.UTF-8@euro").as_deref(), Some("nl-NL"));
    assert_eq!(tag("de_DE.ISO-8859-1").as_deref(), Some("de-DE"));
    assert_eq!(tag("sr_RS@latin").as_deref(), Some("sr-RS"));
    assert_eq!(tag("fr").as_deref(), Some("fr"));
    assert_eq!(tag("en-GB").as_deref(), Some("en-GB"));
    assert_eq!(tag("zh-Hant-TW").as_deref(), Some("zh-Hant-TW"));
    assert_eq!(tag(" pt_BR.utf8 ").as_deref(), Some("pt-BR"));
}

#[test]
fn the_c_locales_and_junk_name_no_language() {
    for junk in [
        "",
        "C",
        "c",
        "POSIX",
        "C.UTF-8",
        ".UTF-8",
        "@euro",
        "1",
        "x",
        "english",
        "nl NL",
        "nl_NL;rm",
        "../..",
        "nl_\u{e9}",
        "a-b-c-d-e-f-g-h-i-j-k-l-m-n-o-p-q-r-s-t-u-v-w-x-y-z",
        "nl--NL",
    ] {
        assert_eq!(tag(junk), None, "{junk:?}");
    }
}

#[test]
fn lc_all_beats_lc_messages_beats_lang() {
    let all = env(&[("LC_ALL", "fr_FR.UTF-8"), ("LC_MESSAGES", "de_DE"), ("LANG", "nl_NL")]);
    assert_eq!(from_environment(all).as_deref(), Some("fr-FR"));
    let messages = env(&[("LC_MESSAGES", "de_DE"), ("LANG", "nl_NL")]);
    assert_eq!(from_environment(messages).as_deref(), Some("de-DE"));
    assert_eq!(from_environment(env(&[("LANG", "nl_NL.UTF-8@euro")])).as_deref(), Some("nl-NL"));
}

#[test]
fn an_empty_variable_is_as_good_as_unset() {
    let e = env(&[("LC_ALL", ""), ("LC_MESSAGES", "  "), ("LANG", "es_ES.UTF-8")]);
    assert_eq!(from_environment(e).as_deref(), Some("es-ES"));
}

#[test]
fn the_c_locale_set_first_wins_and_names_nothing() {
    let e = env(&[("LC_ALL", "C"), ("LANG", "nl_NL")]);
    assert_eq!(from_environment(e), None);
    assert_eq!(from_environment(env(&[])), None);
}
