// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Czech: its name in the picker, its locale, plural rule, and date conventions.
use crate::Lang;
use crate::date::{Day, long, numeric, time};
use crate::plural::Rule;

#[test]
fn czech_is_offered_by_its_own_name_and_chosen_for_its_locales() {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    assert!(Lang::available().contains(&Lang::Czech));
    assert_eq!(Lang::Czech.picker_name(), "Čeština (náhled)");
    assert_eq!(Lang::from_pref("auto", Some("cs_CZ.UTF-8")), Lang::Czech);
    assert_eq!(Lang::from_pref("auto", Some("cs-CZ")), Lang::Czech);
    assert_eq!(Lang::from_pref("cs", None), Lang::Czech);
}

#[test]
fn czech_has_three_plural_forms() {
    let cs = Rule::from_header("nplurals=3; plural=(n==1) ? 0 : (n>=2 && n<=4) ? 1 : 2;");
    assert_eq!(cs, Rule::Czech);
    assert_eq!(Rule::from_header("nplurals=3; plural=(n==1 ? 0 : (n>=2 && n<=4) ? 1 : 2);"), Rule::Czech);
    let forms: Vec<usize> = [1, 0, 2, 3, 4, 5, 12, 21, 22, 24, 25, 100, 101, 1000].iter().map(|n| cs.index(*n)).collect();
    assert_eq!(forms, [0, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2]);
    assert_eq!(cs.forms(), 3);
    assert_eq!(Lang::Czech.tn(1, "{n} message", "{n} messages"), "{n} zpráva");
    assert_eq!(Lang::Czech.tn(3, "{n} message", "{n} messages"), "{n} zprávy");
    assert_eq!(Lang::Czech.tn(5, "{n} message", "{n} messages"), "{n} zpráv");
}

#[test]
fn czech_writes_dates_its_own_way() {
    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };
    let c = Lang::Czech.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (false, 0, true, true));
    let lang = Lang::Czech;
    assert_eq!(
        (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false)),
        ("neděle 11. října 2026".into(), "03.10.2025".into(), "21:05".into())
    );
}
