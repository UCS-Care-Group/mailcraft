// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's choice of accent and high contrast (#703): the commands, `ui.json`, and what the
//! window shows.

use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::theme::{Accent, Tokens};
use crate::{MailApp, Services, UiState};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

#[test]
fn the_commands_set_the_choice_and_refuse_anything_else() {
    let mut a = app();
    assert_eq!(a.execute("ui.accent", json!({"accent": "teal"})).unwrap()["accent"], "teal");
    assert_eq!(a.ui.accent, Accent::Teal);
    assert_eq!(a.execute("ui.highContrast", json!({"on": true})).unwrap()["on"], true);
    assert!(a.ui.high_contrast);
    for junk in [
        json!({}),
        json!(null),
        json!([]),
        json!("x"),
        json!({"accent": 7}),
        json!({"accent": "pink"}),
        json!({"accent": ["teal"]}),
        json!({"accent": null}),
    ] {
        assert!(a.execute("ui.accent", junk).is_err());
    }
    for junk in [json!({}), json!(null), json!([]), json!({"on": "yes"}), json!({"on": 1}), json!({"on": null}), json!({"on": [true]})] {
        assert!(a.execute("ui.highContrast", junk).is_err());
    }
    assert_eq!(a.ui.accent, Accent::Teal);
    assert!(a.ui.high_contrast);
}

#[test]
fn the_choice_is_saved_and_a_missing_or_unreadable_one_falls_back() {
    let ui = UiState { accent: Accent::Violet, high_contrast: true, ..UiState::default() };
    let saved: Value = serde_json::to_value(&ui).unwrap();
    assert_eq!(saved["accent"], "violet");
    assert_eq!(saved["highContrast"], true);
    let back: UiState = serde_json::from_value(saved).unwrap();
    assert_eq!((back.accent, back.high_contrast), (Accent::Violet, true));
    // Keys from before this existed.
    let old: UiState = serde_json::from_value(json!({"theme": "dark"})).unwrap();
    assert_eq!((old.accent, old.high_contrast, old.theme), (Accent::Blue, false, crate::theme::Appearance::Dark));
    // Values that are not what they should be read as the defaults, and the rest is kept.
    for junk in [json!("mauve"), json!(5), json!(null), json!([1]), json!({"a": 1})] {
        let bad: UiState = serde_json::from_value(json!({"theme": "dark", "accent": junk.clone(), "highContrast": junk})).unwrap();
        assert_eq!((bad.accent, bad.high_contrast, bad.theme), (Accent::Blue, false, crate::theme::Appearance::Dark));
    }
}

#[test]
fn the_window_shows_the_accent_and_high_contrast_at_once() {
    let mut a = app();
    a.execute("ui.theme", json!({"value": "light"})).unwrap();
    a.execute("ui.accent", json!({"accent": "graphite"})).unwrap();
    a.execute("ui.highContrast", json!({"on": true})).unwrap();
    let ctx = egui::Context::default();
    a.logic(&ctx);
    let t = Tokens::get(&ctx);
    assert_eq!(t.accent, Accent::Graphite.ink(false, true, None).accent);
    assert!(t.high_contrast);
    a.execute("ui.highContrast", json!({"on": false})).unwrap();
    a.execute("ui.accent", json!({"accent": "blue"})).unwrap();
    a.logic(&ctx);
    assert_eq!(Tokens::get(&ctx).accent, Tokens::light().accent);
}

#[test]
fn a_custom_accent_takes_its_colour_from_the_command_and_keeps_it() {
    let mut a = app();
    // "custom" with a colour, in the forms the command takes them.
    let r = a.execute("ui.accent", json!({"accent": "custom", "color": "#ff8800"})).unwrap();
    assert_eq!(r["accent"], "custom");
    assert_eq!(r["color"], "#FF8800");
    assert_eq!((a.ui.accent, a.ui.custom_accent), (Accent::Custom, Some(egui::Color32::from_rgb(0xFF, 0x88, 0x00))));
    assert_eq!(a.execute("ui.accent", json!({"accent": "#0af"})).unwrap()["color"], "#00AAFF");
    assert_eq!(a.ui.custom_accent, Some(egui::Color32::from_rgb(0x00, 0xAA, 0xFF)));
    assert_eq!(a.execute("ui.accent", json!({"accent": "custom", "color": [10, 20, 30]})).unwrap()["color"], "#0A141E");
    assert_eq!(a.ui.custom_accent, Some(egui::Color32::from_rgb(10, 20, 30)));
    // "custom" without a colour keeps the one last picked.
    let r = a.execute("ui.accent", json!({"accent": "custom"})).unwrap();
    assert_eq!(r["color"], "#0A141E");
    assert_eq!(a.ui.custom_accent, Some(egui::Color32::from_rgb(10, 20, 30)));
    // A preset still works and leaves the custom colour where it was.
    assert_eq!(a.execute("ui.accent", json!({"accent": "teal"})).unwrap()["accent"], "teal");
    assert_eq!((a.ui.accent, a.ui.custom_accent), (Accent::Teal, Some(egui::Color32::from_rgb(10, 20, 30))));
    // Junk is refused and changes nothing.
    for junk in [
        json!({"accent": "custom", "color": "pink"}),
        json!({"accent": "custom", "color": "#12345"}),
        json!({"accent": "custom", "color": "#1234567"}),
        json!({"accent": "custom", "color": 7}),
        json!({"accent": "custom", "color": [1, 2]}),
        json!({"accent": "custom", "color": [1, 2, 300]}),
        json!({"accent": "custom", "color": null}),
        json!({"accent": "#"}),
        json!({"accent": ""}),
        json!({"accent": "cust0m"}),
        json!({"color": "#ff8800"}),
    ] {
        assert!(a.execute("ui.accent", junk.clone()).is_err(), "{junk} was taken");
    }
    assert_eq!((a.ui.accent, a.ui.custom_accent), (Accent::Teal, Some(egui::Color32::from_rgb(10, 20, 30))));
}

#[test]
fn a_custom_accent_is_saved_and_a_missing_or_unreadable_one_falls_back() {
    let ui = UiState { accent: Accent::Custom, custom_accent: Some(egui::Color32::from_rgb(0x12, 0x34, 0x56)), ..UiState::default() };
    let saved: Value = serde_json::to_value(&ui).unwrap();
    assert_eq!(saved["accent"], "custom");
    assert_eq!(saved["customAccent"], "#123456");
    let back: UiState = serde_json::from_value(saved).unwrap();
    assert_eq!((back.accent, back.custom_accent), (Accent::Custom, Some(egui::Color32::from_rgb(0x12, 0x34, 0x56))));
    // A custom accent with no colour saved (or an unreadable one) reads back with none, and the
    // window falls back to the default custom colour.
    for junk in [json!(null), json!("mauve"), json!(5), json!([1, 2, 3]), json!({"a": 1})] {
        let old: UiState = serde_json::from_value(json!({"accent": "custom", "customAccent": junk})).unwrap();
        assert_eq!(old.accent, Accent::Custom);
        assert_eq!(old.custom_accent, None);
        assert_eq!(old.custom_accent_color(), crate::theme::DEFAULT_CUSTOM_COLOR);
    }
    let old: UiState = serde_json::from_value(json!({"accent": "violet"})).unwrap();
    assert_eq!(old.custom_accent, None);
}

#[test]
fn the_window_shows_a_custom_accent_at_once() {
    let mut a = app();
    a.execute("ui.theme", json!({"value": "light"})).unwrap();
    let c = egui::Color32::from_rgb(0x12, 0x34, 0x56);
    a.execute("ui.accent", json!({"accent": "custom", "color": "#123456"})).unwrap();
    let ctx = egui::Context::default();
    a.logic(&ctx);
    let t = Tokens::get(&ctx);
    assert_eq!(t.accent, Tokens::build_choice(false, Accent::Custom, Some(c), false).accent);
    // A different colour re-themes the window, and a preset sets it back.
    a.execute("ui.accent", json!({"accent": "#FF8800"})).unwrap();
    a.logic(&ctx);
    let t2 = Tokens::get(&ctx);
    assert_ne!(t2.accent, t.accent);
    assert_eq!(t2.accent, Tokens::build_choice(false, Accent::Custom, Some(egui::Color32::from_rgb(0xFF, 0x88, 0x00)), false).accent);
    a.execute("ui.accent", json!({"accent": "violet"})).unwrap();
    a.logic(&ctx);
    assert_eq!(Tokens::get(&ctx).accent, Accent::Violet.ink(false, false, None).accent);
}
