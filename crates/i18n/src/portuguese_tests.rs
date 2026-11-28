// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Brazilian Portuguese: its name in the picker, its locale, plural rule, and date conventions.
use crate::Lang;
use crate::date::{Day, long, numeric, time};
use crate::plural::Rule;

#[test]
fn portuguese_is_offered_by_its_own_name_and_chosen_for_its_locales() {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    assert_eq!(Lang::Portuguese.code(), "pt_BR");
    assert!(Lang::available().contains(&Lang::Portuguese));
    assert_eq!(Lang::Portuguese.picker_name(), "Português (pré-visualização)");
    assert_eq!(Lang::from_pref("auto", Some("pt_BR.UTF-8")), Lang::Portuguese);
    assert_eq!(Lang::from_pref("auto", Some("pt-BR")), Lang::Portuguese);
    assert_eq!(Lang::from_pref("pt_BR", None), Lang::Portuguese);
    assert_eq!(Lang::from_pref("pt", None), Lang::Portuguese);
    assert_eq!(Lang::from_code("pt_PT"), Some(Lang::Portuguese));
}

#[test]
fn portuguese_has_two_plural_forms() {
    let pt = Rule::from_header("nplurals=2; plural=(n > 1);");
    assert_eq!(pt, Rule::AboveOne);
    assert_eq!(pt.index(1), 0);
    assert_eq!(pt.index(0), 0);
    assert_eq!(pt.index(2), 1);
    assert_eq!(pt.forms(), 2);
    assert_eq!(Lang::Portuguese.tn(1, "{n} message", "{n} messages"), "{n} mensagem");
    assert_eq!(Lang::Portuguese.tn(0, "{n} message", "{n} messages"), "{n} mensagem");
    assert_eq!(Lang::Portuguese.tn(2, "{n} message", "{n} messages"), "{n} mensagens");
}

#[test]
fn portuguese_writes_dates_its_own_way() {
    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };
    let c = Lang::Portuguese.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (false, 6, true, true));
    let lang = Lang::Portuguese;
    assert_eq!(
        (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false)),
        ("domingo, 11 de outubro de 2026".into(), "03/10/2025".into(), "21:05".into())
    );
}
