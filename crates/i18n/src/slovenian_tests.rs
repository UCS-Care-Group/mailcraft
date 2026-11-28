// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Slovenian: its name in the picker, its locale, plural rule, and date conventions.
use crate::Lang;
use crate::date::{Day, long, numeric, time};
use crate::plural::Rule;

#[test]
fn slovenian_is_offered_by_its_own_name_and_chosen_for_its_locales() {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    assert!(Lang::available().contains(&Lang::Slovenian));
    assert_eq!(Lang::Slovenian.picker_name(), "Slovenščina (predogled)");
    assert_eq!(Lang::from_pref("auto", Some("sl_SI.UTF-8")), Lang::Slovenian);
    assert_eq!(Lang::from_pref("sl", None), Lang::Slovenian);
    assert_eq!(Lang::from_code("sl-SI"), Some(Lang::Slovenian));
}

#[test]
fn slovenian_has_four_plural_forms() {
    let sl = Rule::from_header("nplurals=4; plural=(n%100==1 ? 0 : n%100==2 ? 1 : n%100==3 || n%100==4 ? 2 : 3);");
    assert_eq!(sl, Rule::Slovenian);
    let forms: Vec<usize> = [0, 1, 2, 3, 4, 5, 11, 12, 21, 22, 100, 101, 102, 103, 104, 111, 112].iter().map(|n| sl.index(*n)).collect();
    assert_eq!(forms, [3, 0, 1, 2, 2, 3, 3, 3, 3, 3, 3, 0, 1, 2, 2, 3, 3]);
    assert_eq!(sl.forms(), 4);
    assert_eq!(Lang::Slovenian.tn(1, "{n} message", "{n} messages"), "{n} sporočilo");
    assert_eq!(Lang::Slovenian.tn(2, "{n} message", "{n} messages"), "{n} sporočili");
    assert_eq!(Lang::Slovenian.tn(3, "{n} message", "{n} messages"), "{n} sporočila");
    assert_eq!(Lang::Slovenian.tn(5, "{n} message", "{n} messages"), "{n} sporočil");
    assert_eq!(Lang::Slovenian.tn(101, "{n} message", "{n} messages"), "{n} sporočilo");
}

#[test]
fn slovenian_writes_dates_its_own_way() {
    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };
    let c = Lang::Slovenian.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (false, 0, true, true));
    let lang = Lang::Slovenian;
    assert_eq!(
        (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false)),
        ("nedelja, 11. oktober 2026".into(), "03.10.2025".into(), "21:05".into())
    );
    assert_eq!(time(&lang, 9, 5, false), "09:05");
}
