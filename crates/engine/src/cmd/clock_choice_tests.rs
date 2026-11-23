// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1800: the clock the owner reads is Automatic (the language's), 12-hour or 24-hour; what is
//! sent to others keeps the language's.

use mailcraft_i18n::Lang;
use mailcraft_model::Mailbox;
use serde_json::json;

use crate::{Prefs, Session};

fn session(lang: Lang, region: Option<&str>) -> Session {
    let mut s = Session::at(Mailbox::default(), 0, 0);
    s.interface_language = lang;
    s.date_region = region.map(str::to_string);
    s
}

fn twelve(s: &Session) -> bool {
    s.prefs.twelve_hour
}

#[test]
fn each_choice_in_english_a_24_hour_language_and_a_12_hour_region() {
    // (language, region, what Automatic gives)
    for (lang, region, own) in
        [(Lang::English, None, true), (Lang::English, Some("GB"), false), (Lang::German, None, false), (Lang::English, Some("AU"), true)]
    {
        let mut s = session(lang, region);
        s.run("file.options", &json!({"clock": "12"})).unwrap();
        assert!(twelve(&s));
        assert_eq!(s.prefs.clock_chosen, Some(true));
        s.run("file.options", &json!({"clock": "24"})).unwrap();
        assert!(!twelve(&s));
        s.run("file.options", &json!({"clock": "auto"})).unwrap();
        assert_eq!((twelve(&s), s.prefs.clock_chosen), (own, Some(false)), "{lang:?} {region:?}");
        // The times the owner reads follow it.
        let at = mailcraft_time::DateTime::from_timestamp(15 * 3600, 0);
        assert_eq!(s.time_said(at), if own { "3:00 PM" } else { "15:00" });
    }
}

#[test]
fn a_chosen_clock_survives_the_language_and_is_saved() {
    let mut s = session(Lang::German, None);
    s.run("file.options", &json!({"clock": "12"})).unwrap();
    s.prefs.follow_conventions(Lang::German.conventions());
    assert!(twelve(&s));
    let back: Prefs = serde_json::from_str(&serde_json::to_string(&s.prefs).unwrap()).unwrap();
    assert_eq!((back.twelve_hour, back.clock_chosen), (true, Some(true)));
    s.run("file.options", &json!({"clock": "auto"})).unwrap();
    let back: Prefs = serde_json::from_str(&serde_json::to_string(&s.prefs).unwrap()).unwrap();
    assert_eq!((back.twelve_hour, back.clock_chosen), (false, Some(false)));
}

#[test]
fn a_clock_that_is_none_of_them_is_refused() {
    let mut s = session(Lang::English, None);
    assert!(s.run("file.options", &json!({"clock": "13"})).is_err());
    assert_eq!(s.prefs.clock_chosen, None);
}
