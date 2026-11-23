// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Classic Light (#1203): the `ui.theme` value, `ui.json`, what the window shows with each
//! accent and high contrast, that the choice survives a restart and a message being written
//! survives a switch, and that every module draws in a narrow window and at 200 %.

use egui::vec2;
use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::theme::{Accent, Appearance, Tokens};
use crate::{MailApp, Services, UiState};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    a
}

/// The tokens the window is drawn with after a frame.
fn shown(a: &mut MailApp, ctx: &egui::Context) -> Tokens {
    a.logic(ctx);
    Tokens::get(ctx)
}

fn window(a: MailApp, width: f32, scale: f32) -> Window {
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(vec2(width, 760.0))
        .with_pixels_per_point(scale)
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
    harness.run_steps(4);
    // The sample has reminders due, and their window would lie over the rest.
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    harness
}

#[test]
fn the_command_takes_classic_light_and_refuses_anything_else() {
    let mut a = app();
    assert_eq!(a.execute("ui.theme", json!({"value": "classicLight"})).unwrap()["value"], "classicLight");
    assert_eq!(a.ui.theme, Appearance::ClassicLight);
    for junk in [json!({}), json!(null), json!([]), json!("classicLight"), json!({"value": 1}), json!({"value": "classic"}), json!({"value": null})] {
        assert!(a.execute("ui.theme", junk).is_err());
    }
    assert_eq!(a.ui.theme, Appearance::ClassicLight);
    assert_eq!(a.execute("ui.theme", json!({"value": " ClassicLight "})).unwrap()["value"], "classicLight");
    // It is offered with the others, right after Light.
    let at = |a: Appearance| Appearance::ALL.iter().position(|x| *x == a);
    assert_eq!(at(Appearance::ClassicLight), at(Appearance::Light).map(|i| i + 1));
    assert!([Appearance::System, Appearance::Dark].iter().all(|a| at(*a).is_some()));
    assert_eq!(Appearance::ClassicLight.label(), "Classic Light");
}

#[test]
fn the_choice_is_saved_and_survives_a_restart() {
    let mut a = app();
    a.execute("ui.theme", json!({"value": "classicLight"})).unwrap();
    a.execute("ui.accent", json!({"accent": "teal"})).unwrap();
    let saved: Value = serde_json::to_value(a.prefs()).unwrap();
    assert_eq!(saved["theme"], "classicLight");
    // The next start reads it back and draws it.
    let mut b = app();
    b.apply_prefs(serde_json::from_value(saved).unwrap());
    assert_eq!((b.ui.theme, b.ui.accent), (Appearance::ClassicLight, Accent::Teal));
    let t = shown(&mut b, &egui::Context::default());
    assert!(t.dense && !t.dark);
    assert_eq!(t.canvas, Tokens::classic_light().canvas);
    assert_eq!(t.accent, Accent::Teal.ink(false, false, None).accent);
    // An unknown value still reads as following the system, and the rest of the file is kept.
    let bad: UiState = serde_json::from_value(json!({"theme": "classicDark", "accent": "violet"})).unwrap();
    assert_eq!((bad.theme, bad.accent), (Appearance::System, Accent::Violet));
}

#[test]
fn the_window_shows_classic_light_with_the_accent_and_high_contrast() {
    let mut a = app();
    let ctx = egui::Context::default();
    a.execute("ui.theme", json!({"value": "light"})).unwrap();
    let light = shown(&mut a, &ctx);
    assert!(!light.dense);

    a.execute("ui.theme", json!({"value": "classicLight"})).unwrap();
    let classic = shown(&mut a, &ctx);
    // A light window for egui and the window's decorations.
    assert_eq!(ctx.options(|o| o.theme_preference), egui::ThemePreference::Light);
    assert!(classic.dense && !classic.dark);
    assert_eq!(classic.canvas, Tokens::classic_light().canvas);
    assert_ne!(classic.canvas, light.canvas);
    // A white page in a grey frame, and a flatter ribbon.
    assert_eq!((classic.surface, classic.ribbon), (egui::Color32::WHITE, egui::Color32::WHITE));
    assert!(classic.sheet_radius < light.sheet_radius && classic.control_radius < light.control_radius);
    assert_eq!(classic.accent, light.accent);

    // The accent and high contrast apply to it as to the others.
    a.execute("ui.accent", json!({"accent": "violet"})).unwrap();
    a.execute("ui.highContrast", json!({"on": true})).unwrap();
    let strong = shown(&mut a, &ctx);
    assert!(strong.high_contrast && strong.dense && !strong.dark);
    assert_eq!(strong.accent, Accent::Violet.ink(false, true, None).accent);
    assert_eq!(strong.selection, Accent::Violet.ink(false, true, None).selection);
    assert_eq!(strong.text, Tokens::build(false, Accent::Violet, true).text);

    // The dark half is untouched; and back to Light is Light again.
    assert_eq!(crate::theme::ThemeChoice { appearance: Appearance::ClassicLight, ..Default::default() }.palette().dark.canvas, Tokens::dark().canvas);
    a.execute("ui.highContrast", json!({"on": false})).unwrap();
    a.execute("ui.accent", json!({"accent": "blue"})).unwrap();
    a.execute("ui.theme", json!({"value": "light"})).unwrap();
    let again = shown(&mut a, &ctx);
    assert_eq!((again.canvas, again.dense, again.accent), (light.canvas, false, light.accent));
}

/// A message being written is left as it is when the theme changes under it, both ways.
#[test]
fn a_message_being_written_survives_a_switch() {
    let mut harness = window(app(), 1280.0, 1.0);
    harness.state_mut().execute("mail.new", json!({"to": "alice@example.org", "subject": "Quarterly figures", "body": "Draft text"})).unwrap();
    harness.run_steps(4);
    let before = harness.state().session.compose.clone().unwrap();
    for theme in ["classicLight", "light", "classicLight", "dark", "classicLight"] {
        harness.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
        harness.run_steps(4);
        let now = harness.state().session.compose.clone().unwrap_or_else(|| panic!("the message closed on {theme}"));
        assert_eq!((now.to.as_str(), now.subject.as_str()), (before.to.as_str(), before.subject.as_str()), "{theme}");
        assert_eq!(now.body, before.body, "{theme}");
        assert_eq!(harness.state().session.compose_id(), Some(before.id), "{theme}");
    }
}

/// Mail, a message being written, the calendar and people draw in Classic Light in a narrow
/// window and at 200 %, with the ribbon's row inside its band.
#[test]
fn every_module_draws_narrow_and_at_twice_the_size() {
    for (width, scale) in [(640.0, 1.0), (1280.0, 2.0), (900.0, 1.5)] {
        let mut a = app();
        a.execute("ui.theme", json!({"value": "classicLight"})).unwrap();
        let mut harness = window(a, width, scale);
        for (module, command, params) in [
            ("mail", "view.mail", json!({})),
            ("compose", "mail.new", json!({"subject": "Narrow"})),
            ("calendar", "view.calendar", json!({})),
            ("people", "view.people", json!({})),
        ] {
            harness.state_mut().execute(command, params).unwrap();
            harness.run_steps(4);
            let name = format!("{module} at {width} and {scale}x");
            assert!(Tokens::get(&harness.ctx).dense, "{name}");
            let band = harness.state().hit.rect("ribbonBand", 0).unwrap_or_else(|| panic!("{name}: no ribbon"));
            assert!(band.max.x <= width + 0.5, "{name}: {band:?}");
            for b in crate::ribbon::row(&crate::ribbon::describe(harness.state())) {
                if let Some(r) = harness.state().hit.rect("ribbon", crate::ribbon::hit_id(b.item.key)) {
                    assert!(
                        r.min.y >= band.min.y - 0.5 && r.max.y <= band.max.y + 0.5 && r.max.x <= band.max.x + 0.5,
                        "{name}: {} at {r:?}",
                        b.item.key
                    );
                }
            }
            if module == "compose" {
                let _ = harness.state_mut().execute("compose.discard", json!({}));
                harness.run_steps(2);
            }
        }
    }
}

#[test]
fn classic_light_draws_its_bands_tabs_and_rows_its_own_way_and_light_does_not() {
    for accent in Accent::ALL {
        for high in [false, true] {
            let t = Tokens::build_classic(accent, None, high);
            assert!(t.status_band && t.tab_caps && t.file_tab_fill && t.row_bar, "{accent:?} high contrast {high}");
            assert_eq!((t.sheet_radius, t.control_radius), (0, 0));
            let light = Tokens::build(false, accent, high);
            assert!(!light.status_band && !light.tab_caps && !light.file_tab_fill && !light.row_bar);
            assert_eq!((light.sheet_radius, light.control_radius), (8, 6));
        }
    }
    let dark = Tokens::build(true, Accent::Blue, false);
    assert!(!dark.status_band && !dark.tab_caps && !dark.file_tab_fill && !dark.row_bar);
}

#[test]
fn the_status_band_is_drawn_in_the_accents_ink() {
    let t = Tokens::build_classic(Accent::Blue, None, false);
    let band = t.on_band(t.accent);
    assert_eq!(band.status_bar, t.accent);
    assert_eq!((band.text, band.text_dim, band.icon), (t.on_accent, t.on_accent, t.on_accent));
    // A band of another colour than the accent is written in the theme's own text colour.
    assert_eq!(t.on_band(t.surface).text, t.text);
}

#[test]
fn tokens_drawn_for_a_band_are_put_back_after_it() {
    let ctx = egui::Context::default();
    let before = Tokens::get(&ctx);
    let band = before.on_band(before.accent);
    let inside = Tokens::scoped(&ctx, band, || Tokens::get(&ctx));
    assert_eq!(inside.status_bar, before.accent);
    assert_eq!(Tokens::get(&ctx).text, before.text);
    crate::theme::apply(
        &ctx,
        egui::ThemePreference::Light,
        &crate::theme::ThemeChoice { appearance: Appearance::ClassicLight, ..Default::default() },
    );
    let installed = Tokens::get(&ctx);
    Tokens::scoped(&ctx, installed.on_band(installed.accent), || ());
    assert!(Tokens::get(&ctx).file_tab_fill);
    assert_eq!(Tokens::get(&ctx).text, installed.text);
}
