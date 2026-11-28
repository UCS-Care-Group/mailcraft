// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Galician: its name in the picker, its locale, plural rule, and date conventions.
use crate::Lang;
use crate::date::{Day, long, numeric, time};
use crate::plural::Rule;

#[test]
fn galician_is_offered_by_its_own_name_and_chosen_for_its_locales() {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    assert!(Lang::available().contains(&Lang::Galician));
    assert_eq!(Lang::Galician.picker_name(), "Galego (vista previa)");
    assert_eq!(Lang::from_pref("auto", Some("gl_ES.UTF-8")), Lang::Galician);
    assert_eq!(Lang::from_pref("auto", Some("gl-ES")), Lang::Galician);
    assert_eq!(Lang::from_pref("gl", None), Lang::Galician);
}

#[test]
fn galician_has_two_plural_forms() {
    let gl = Rule::from_header("nplurals=2; plural=(n != 1);");
    assert_eq!(gl, Rule::NotOne);
    assert_eq!(gl.forms(), 2);
    assert_eq!(Lang::Galician.tn(1, "{n} message", "{n} messages"), "{n} mensaxe");
    assert_eq!(Lang::Galician.tn(0, "{n} message", "{n} messages"), "{n} mensaxes");
    assert_eq!(Lang::Galician.tn(2, "{n} message", "{n} messages"), "{n} mensaxes");
    assert_eq!(Lang::Galician.tn(5, "{n} message", "{n} messages"), "{n} mensaxes");
}

#[test]
fn galician_writes_dates_its_own_way() {
    const SUN: Day = Day { year: 2026, month: 10, day: 11, weekday: 6 };
    const OLD: Day = Day { year: 2025, month: 10, day: 3, weekday: 4 };
    let c = Lang::Galician.conventions();
    assert_eq!((c.twelve_hour, c.week_start, c.pad_numeric, c.day_first), (false, 0, true, true));
    let lang = Lang::Galician;
    assert_eq!(
        (long(&lang, SUN), numeric(&lang, OLD), time(&lang, 21, 5, false)),
        ("domingo, 11 de outubro de 2026".into(), "03/10/2025".into(), "21:05".into())
    );
}

#[test]
fn galician_folder_names() {
    assert_eq!(Lang::Galician.folder("Archive"), "Arquivo");
    assert_eq!(Lang::Galician.t("Archive"), "Arquivar");
    assert_eq!(Lang::Galician.folder("Inbox"), "Caixa de entrada");
    assert_eq!(Lang::Galician.folder("Sent Items"), "Elementos enviados");
    assert_eq!(Lang::Galician.folder("Drafts"), "Borradores");
    assert_eq!(Lang::Galician.folder("Deleted Items"), "Elementos eliminados");
    assert_eq!(Lang::Galician.folder("Junk Email"), "Correo lixo");
    assert_eq!(Lang::Galician.folder("Outbox"), "Caixa de saída");
}
