// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Turkish: its name in the picker, its locale, plural rule, and date conventions.
use crate::Lang;
use crate::date::{Day, long, numeric, time};
use crate::plural::Rule;

#[test]
fn turkish_is_offered_by_its_own_name_and_chosen_for_its_locales() {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    assert!(Lang::available().contains(&Lang::Turkish));
    assert_eq!(Lang::Turkish.picker_name(), "Türkçe (önizleme)");
    assert_eq!(Lang::from_pref("auto", Some("tr_TR.UTF-8")), Lang::Turkish);
    assert_eq!(Lang::from_pref("auto", Some("tr-TR")), Lang::Turkish);
    assert_eq!(Lang::from_pref("tr", None), Lang::Turkish);
}

#[test]
fn turkish_plurals_and_dates() {
    let tr = Rule::from_header("nplurals=2; plural=(n != 1);");
    assert_eq!(tr, Rule::NotOne);
    assert_eq!(Lang::Turkish.tn(1, "{n} message", "{n} messages"), "{n} ileti");
    assert_eq!(Lang::Turkish.tn(5, "{n} message", "{n} messages"), "{n} ileti");

    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };
    let c = Lang::Turkish.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (false, 0, true, true));
    let lang = Lang::Turkish;
    assert_eq!(
        (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false)),
        ("11 Ekim 2026 Pazar".into(), "03.10.2025".into(), "21:05".into())
    );
}
