// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Russian: its name in the picker, its locale, plural rule, and date conventions.
use crate::Lang;
use crate::date::{Day, duration, long, month_year, numeric, time};
use crate::plural::Rule;

#[test]
fn russian_is_offered_by_its_own_name_and_chosen_for_its_locales() {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    assert!(Lang::available().contains(&Lang::Russian));
    assert_eq!(Lang::Russian.picker_name(), "Русский (предварительная версия)");
    assert_eq!(Lang::from_code("ru-RU"), Some(Lang::Russian));
    assert_eq!(Lang::from_pref("auto", Some("ru_RU.UTF-8")), Lang::Russian);
    assert_eq!(Lang::from_pref("ru", None), Lang::Russian);
}

/// 1, 21, 101 take one form; 2 to 4, 22 to 24 another; the rest, 11 to 14 among them, the third.
/// Not Polish's rule: Polish says 21 with the third form.
#[test]
fn russian_has_three_plural_forms() {
    let ru = Rule::from_header("nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);");
    assert_eq!(ru, Rule::Slavic);
    assert_eq!(ru.forms(), 3);
    let forms: Vec<usize> = [0, 1, 2, 4, 5, 11, 12, 14, 21, 22, 25, 101, 111, 112].iter().map(|n| ru.index(*n)).collect();
    assert_eq!(forms, [2, 0, 1, 1, 2, 2, 2, 2, 0, 1, 2, 0, 2, 2]);
    assert_eq!(duration(&Lang::Russian, 1), "1 минута");
    assert_eq!(
        (duration(&Lang::Russian, 2), duration(&Lang::Russian, 5), duration(&Lang::Russian, 21)),
        ("2 минуты".into(), "5 минут".into(), "21 минута".into())
    );
}

#[test]
fn russian_writes_dates_its_own_way() {
    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };
    let c = Lang::Russian.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (false, 0, true, true));
    let lang = Lang::Russian;
    // The month in a date is in the genitive.
    assert_eq!(
        (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false)),
        ("воскресенье, 11 октября 2026 г.".into(), "03.10.2025".into(), "21:05".into())
    );
    // Standing alone, in the nominative.
    assert_eq!(month_year(&lang, 2026, 10), "Октябрь 2026 г.");
}
