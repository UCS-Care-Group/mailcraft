// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! In a plain-text message, Font, Font Size, Font Color and Text Highlight Color are greyed with
//! the formatting buttons beside them: a menu that only formats stands for its command, and is
//! usable when that command can run ([`super::usable`], which every layout draws by).

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use mailcraft_engine::Session;
use serde_json::json;

use super::{Menu, describe_tab, submenu, usable};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A new message, in plain text or HTML.
fn writing(plain: bool) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("mail.new", json!({})).unwrap();
    app.execute(if plain { "compose.formatPlain" } else { "compose.formatHtml" }, json!({})).unwrap();
    app
}

#[test]
fn each_font_menu_stands_for_the_command_its_entries_run() {
    assert_eq!(Menu::Font.command(), Some("format.font"));
    assert_eq!(Menu::FontSize.command(), Some("format.size"));
    assert_eq!(Menu::FontColor.command(), Some("format.color"));
    assert_eq!(Menu::Highlight.command(), Some("format.highlight"));
    // A menu of choices of its own is always there.
    assert_eq!(Menu::MoveMessages.command(), None);
}

#[test]
fn in_a_plain_text_message_the_font_menus_are_greyed_like_bold() {
    for tab in ["Message", "Format Text"] {
        for (plain, want) in [(true, false), (false, true)] {
            let app = writing(plain);
            let described = describe_tab(&app, tab);
            let item = |key: &str| described.items().find(|i| i.key == key).cloned().unwrap_or_else(|| panic!("{tab}: no {key}"));
            assert_eq!(usable(&app, &item("format.bold")), want, "{tab}, plain {plain}: Bold");
            for key in ["font", "fontSize", "fontColor", "highlight"] {
                assert_eq!(usable(&app, &item(key)), want, "{tab}, plain {plain}: {key}");
            }
        }
    }
}

/// Whether the entry `submenu` draws for `menu`, named `name`, is greyed, as a screen reader is
/// told.
fn entry_greyed(app: MailApp, menu: Menu, name: &'static str) -> bool {
    let mut h =
        Harness::builder().with_accessibility_check(false).build_ui_state(move |ui, app: &mut MailApp| submenu(ui, app, name.to_string(), menu), app);
    h.run_steps(2);
    h.get_by_label(name).accesskit_node().is_disabled()
}

/// Behind a `…` or a `▾` (the simplified row's and the classic groups'), a font menu is an
/// entry greyed the same way, and one that can be used is not.
#[test]
fn behind_a_more_menu_the_font_menus_are_greyed_in_a_plain_text_message() {
    for (menu, name) in
        [(Menu::Font, "Font"), (Menu::FontSize, "Font Size"), (Menu::FontColor, "Font Color"), (Menu::Highlight, "Text Highlight Color")]
    {
        assert!(entry_greyed(writing(true), menu, name), "{name} in plain text");
        assert!(!entry_greyed(writing(false), menu, name), "{name} in HTML");
    }
}
