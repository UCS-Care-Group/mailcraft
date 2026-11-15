// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The dark theme's two styles (#1384): neutral greys by default, or the blue-tinted greys of
//! before #1242; the command, `ui.json`, what the window shows, and that the accent and high
//! contrast still apply to both.

use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::theme::{Accent, DarkStyle, Tokens};
use crate::{MailApp, Services, UiState};

const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a
}

/// The tokens the window is drawn with after a frame.
fn shown(a: &mut MailApp, ctx: &egui::Context) -> Tokens {
    a.logic(ctx);
    Tokens::get(ctx)
}

#[test]
fn neutral_is_the_default_and_the_command_refuses_anything_else() {
    let mut a = app();
    assert_eq!(a.ui.dark_style, DarkStyle::Neutral);
    assert_eq!(a.execute("ui.darkStyle", json!({"value": "blue"})).unwrap()["value"], "blue");
    assert_eq!(a.ui.dark_style, DarkStyle::Blue);
    for junk in [json!({}), json!(null), json!([]), json!("blue"), json!({"value": 1}), json!({"value": "green"}), json!({"value": null})] {
        assert!(a.execute("ui.darkStyle", junk).is_err());
    }
    assert_eq!(a.ui.dark_style, DarkStyle::Blue);
    assert_eq!(a.execute("ui.darkStyle", json!({"value": " Neutral "})).unwrap()["value"], "neutral");
}

#[test]
fn the_style_is_saved_and_a_missing_or_unreadable_one_is_neutral() {
    let ui = UiState { dark_style: DarkStyle::Blue, ..UiState::default() };
    let saved: Value = serde_json::to_value(&ui).unwrap();
    assert_eq!(saved["darkStyle"], "blue");
    let back: UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(back.dark_style, DarkStyle::Blue);
    let old: UiState = serde_json::from_value(json!({"theme": "dark", "accent": "teal"})).unwrap();
    assert_eq!((old.dark_style, old.accent), (DarkStyle::Neutral, Accent::Teal));
    for junk in [json!("mauve"), json!(5), json!(null), json!([1]), json!({"a": 1})] {
        let bad: UiState = serde_json::from_value(json!({"theme": "dark", "darkStyle": junk, "accent": "violet"})).unwrap();
        assert_eq!((bad.dark_style, bad.accent), (DarkStyle::Neutral, Accent::Violet));
    }
}

#[test]
fn the_window_shows_the_style_in_dark_only_with_the_accent_and_high_contrast() {
    let mut a = app();
    let ctx = egui::Context::default();
    a.execute("ui.theme", json!({"value": "dark"})).unwrap();
    let neutral = shown(&mut a, &ctx);
    assert_eq!(neutral.canvas, Tokens::dark().canvas);

    a.execute("ui.darkStyle", json!({"value": "blue"})).unwrap();
    let blue = shown(&mut a, &ctx);
    assert_eq!(blue.canvas, Tokens::dark_blue().canvas);
    assert_ne!(blue.canvas, neutral.canvas);
    // Blue-tinted: more blue than red in the window's grey.
    assert!(blue.canvas.b() > blue.canvas.r());
    assert_eq!(blue.accent, Accent::Blue.ink_on_blue(false).accent);

    // The accent and high contrast apply to it as to the neutral greys.
    a.execute("ui.accent", json!({"accent": "teal"})).unwrap();
    a.execute("ui.highContrast", json!({"on": true})).unwrap();
    let strong = shown(&mut a, &ctx);
    assert!(strong.high_contrast && strong.dark);
    assert_eq!(strong.accent, Accent::Teal.ink_on_blue(true).accent);
    assert_eq!(strong.selection, Accent::Teal.ink_on_blue(true).selection);
    assert!(strong.canvas.b() > strong.canvas.r());
    assert_ne!(strong.canvas, Tokens::build(true, Accent::Teal, true).canvas);

    // The light theme is the same in both styles.
    a.execute("ui.theme", json!({"value": "light"})).unwrap();
    let light_blue = shown(&mut a, &ctx);
    a.execute("ui.darkStyle", json!({"value": "neutral"})).unwrap();
    let light_neutral = shown(&mut a, &ctx);
    assert_eq!(light_blue.canvas, light_neutral.canvas);
    assert_eq!(light_blue.canvas, Tokens::build(false, Accent::Teal, true).canvas);
}

/// The blue style is the palette that shipped before #1242.
#[test]
fn the_blue_style_is_the_palette_from_before() {
    let t = Tokens::build_dark(DarkStyle::Blue, Accent::Blue, false);
    let c = egui::Color32::from_rgb;
    assert_eq!((t.canvas, t.panel, t.surface, t.ribbon), (c(0x12, 0x16, 0x1D), c(0x16, 0x1B, 0x23), c(0x1B, 0x21, 0x2B), c(0x20, 0x27, 0x33)));
    assert_eq!((t.text, t.text_dim, t.border), (c(0xF1, 0xF4, 0xF9), c(0xB4, 0xBC, 0xCB), c(0x2C, 0x34, 0x42)));
    assert_eq!((t.accent_soft, t.selection, t.today), (c(0x1F, 0x33, 0x58), c(0x1F, 0x2B, 0x40), c(0x1B, 0x2B, 0x45)));
    // Neutral is what `Tokens::build` gives, as before.
    assert_eq!(Tokens::build_dark(DarkStyle::Neutral, Accent::Violet, true).canvas, Tokens::build(true, Accent::Violet, true).canvas);
}
