// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Arabic: its name in the picker, its locale, the six plural forms, and day-first dates.
use crate::Lang;
use crate::date::{Day, long, numeric, time};
use crate::plural::Rule;

#[test]
fn arabic_is_offered_by_its_own_name_on_desktop_only() {
    assert_eq!(Lang::Arabic.picker_name(), "العربية (معاينة)");
    assert_eq!(Lang::from_code("ar"), Some(Lang::Arabic));
    assert_eq!(Lang::Arabic.code(), "ar");
    assert!(Lang::Arabic.is_rtl());
    assert!(Lang::ALL.iter().filter(|l| l.is_rtl()).count() == 1);
    // The bundled faces have no Arabic glyphs and the browser has no system fallback.
    assert!(Lang::Arabic.supported_on(false));
    assert!(!Lang::Arabic.supported_on(true));
    if !cfg!(target_arch = "wasm32") {
        assert!(Lang::available().contains(&Lang::Arabic));
        assert_eq!(Lang::from_pref("auto", Some("ar_SA.UTF-8")), Lang::Arabic);
    }
}

#[test]
fn arabic_has_six_plural_forms() {
    let ar = Rule::from_header("nplurals=6; plural=n==0 ? 0 : n==1 ? 1 : n==2 ? 2 : n%100>=3 && n%100<=10 ? 3 : n%100>=11 ? 4 : 5;");
    assert_eq!(ar, Rule::Arabic);
    let forms: Vec<usize> = [0, 1, 2, 3, 10, 11, 99, 100, 102, 103, 111, 1000].iter().map(|n| ar.index(*n)).collect();
    assert_eq!(forms, [0, 1, 2, 3, 3, 4, 4, 5, 5, 3, 4, 5]);
    assert_eq!(ar.forms(), 6);
}

#[test]
fn arabic_writes_dates_day_first_from_its_catalogue() {
    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    let c = Lang::Arabic.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (true, 6, true, true));
    let lang = Lang::Arabic;
    let n = numeric(&lang, SUN);
    assert!(n.starts_with("11/10/"), "{n}");
    let l = long(&lang, SUN);
    assert!(l.find("11").is_some_and(|d| l.find("2026").is_some_and(|y| d < y)), "{l}");
    assert!(!time(&lang, 15, 5, true).is_empty());
}
