// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Polish: its name in the picker, its locale, plural rule, and date conventions.
use crate::Lang;
use crate::date::{Day, long, numeric, time};
use crate::plural::Rule;

#[test]
fn polish_is_offered_by_its_own_name_and_chosen_for_its_locales() {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    assert!(Lang::available().contains(&Lang::Polish));
    assert_eq!(Lang::Polish.picker_name(), "Polski (wersja zapoznawcza)");
    assert_eq!(Lang::from_pref("auto", Some("pl_PL.UTF-8")), Lang::Polish);
    assert_eq!(Lang::from_pref("pl", None), Lang::Polish);
}

#[test]
fn polish_has_three_plural_forms() {
    let pl = Rule::from_header("nplurals=3; plural=(n==1 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);");
    assert_eq!(pl, Rule::Polish);
    let forms: Vec<usize> = [1, 0, 2, 4, 5, 12, 14, 21, 22, 24, 25, 100, 101, 112, 122, 1000].iter().map(|n| pl.index(*n)).collect();
    assert_eq!(forms, [0, 2, 1, 1, 2, 2, 2, 2, 1, 1, 2, 2, 2, 2, 1, 2]);
    assert_eq!(pl.forms(), 3);
}

#[test]
fn polish_writes_dates_its_own_way() {
    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };
    let c = Lang::Polish.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (false, 0, true, true));
    let lang = Lang::Polish;
    assert_eq!(
        (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false)),
        ("niedziela, 11 października 2026".into(), "03.10.2025".into(), "21:05".into())
    );
}
