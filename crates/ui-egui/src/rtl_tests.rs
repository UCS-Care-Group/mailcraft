// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use mailcraft_i18n::Lang;

fn in_language<R>(lang: Lang, f: impl FnOnce() -> R) -> R {
    let before = crate::i18n::current();
    crate::i18n::set_current(lang);
    let out = f();
    crate::i18n::set_current(before);
    out
}

#[test]
fn a_left_to_right_window_keeps_egui_s_layouts() {
    in_language(Lang::English, || {
        assert_eq!(start_to_end(Align::Center), Layout::left_to_right(Align::Center));
        assert_eq!(end_to_start(Align::Min), Layout::right_to_left(Align::Min));
        assert_eq!(start_align(), Align::Min);
    });
}

#[test]
fn an_arabic_window_swaps_the_start_and_the_end() {
    in_language(Lang::Arabic, || {
        assert_eq!(start_to_end(Align::Center), Layout::right_to_left(Align::Center));
        assert_eq!(end_to_start(Align::Min), Layout::left_to_right(Align::Min));
        assert_eq!(start_align(), Align::Max);
    });
}

#[test]
fn a_row_in_an_arabic_window_puts_its_first_item_at_the_right() {
    in_language(Lang::Arabic, || {
        let ctx = egui::Context::default();
        let mut first = egui::Rect::NOTHING;
        let mut second = egui::Rect::NOTHING;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            horizontal(ui, |ui| {
                first = ui.label("a").rect;
                second = ui.label("b").rect;
            });
        });
        assert!(first.min.x > second.min.x, "{first:?} {second:?}");
    });
}
