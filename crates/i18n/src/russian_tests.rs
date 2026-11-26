// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Russian: its name in the picker, its locale, plural rule, and date conventions.
use crate::Lang;
use crate::date::{Day, long, numeric, time};
use crate::plural::Rule;

#[test]
fn russian_is_offered_by_its_own_name_and_chosen_for_its_locales() {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    assert!(Lang::available().contains(&Lang::Russian));
    assert_eq!(Lang::Russian.picker_name(), "Русский (предварительная версия)");
    assert_eq!(Lang::from_pref("auto", Some("ru_RU.UTF-8")), Lang::Russian);
    assert_eq!(Lang::from_pref("ru", None), Lang::Russian);
}

#[test]
fn russian_has_three_plural_forms() {
    let ru = Rule::from_header("nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);");
    assert_eq!(ru, Rule::Russian);
    let forms: Vec<usize> = [1, 0, 2, 4, 5, 11, 12, 14, 21, 22, 24, 25, 100, 101, 111, 122, 1000].iter().map(|n| ru.index(*n)).collect();
    assert_eq!(forms, [0, 2, 1, 1, 2, 2, 2, 2, 0, 1, 1, 2, 2, 0, 2, 1, 2]);
    assert_eq!(ru.forms(), 3);
    assert_eq!(Lang::Russian.tn(1, "{n} message", "{n} messages"), "{n} сообщение");
    assert_eq!(Lang::Russian.tn(3, "{n} message", "{n} messages"), "{n} сообщения");
    assert_eq!(Lang::Russian.tn(5, "{n} message", "{n} messages"), "{n} сообщений");
    assert_eq!(Lang::Russian.tn(21, "{n} message", "{n} messages"), "{n} сообщение");
    assert_eq!(Lang::Russian.tn(22, "{n} message", "{n} messages"), "{n} сообщения");
}

#[test]
fn russian_writes_dates_its_own_way() {
    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };
    let c = Lang::Russian.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (false, 0, true, true));
    let lang = Lang::Russian;
    assert_eq!(
        (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false)),
        ("воскресенье, 11 октября 2026".into(), "03.10.2025".into(), "21:05".into())
    );
}
