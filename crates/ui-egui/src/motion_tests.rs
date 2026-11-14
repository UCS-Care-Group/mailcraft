// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Options › Animations (#1183): the command, `ui.json`, what egui is told, and the
//! system's setting.

use std::cell::Cell;
use std::rc::Rc;

use mailcraft_engine::Session;
use serde_json::{Value, json};

use crate::motion::{Motion, Tracker};
use crate::{MailApp, Services, UiState};

const NOW: i64 = 1_791_993_600;

fn app(system: Option<bool>) -> MailApp {
    let mut services = Services::default();
    if let Some(on) = system {
        services.animations_enabled = Some(Box::new(move || on));
    }
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    a.autosave = false;
    a
}

/// Whether egui animates after a frame of `a`'s logic.
fn animates(a: &mut MailApp, ctx: &egui::Context) -> bool {
    a.logic(ctx);
    let style = ctx.global_style();
    let time = style.animation_time;
    assert_eq!(time > 0.0, style.scroll_animation != egui::style::ScrollAnimation::none(), "scrolling follows the same choice");
    time > 0.0
}

#[test]
fn the_command_sets_the_choice_and_refuses_anything_else() {
    let mut a = app(None);
    assert_eq!(a.ui.animations, Motion::System);
    for (value, motion) in [("off", Motion::Off), ("on", Motion::On), (" system ", Motion::System), ("off", Motion::Off)] {
        assert_eq!(a.execute("ui.animations", json!({"value": value})).unwrap()["value"], motion.code());
        assert_eq!(a.ui.animations, motion);
    }
    for junk in
        [json!({}), json!(null), json!([]), json!({"value": "fast"}), json!({"value": false}), json!({"value": null}), json!({"value": ["on"]})]
    {
        assert!(a.execute("ui.animations", junk).is_err());
    }
    assert_eq!(a.ui.animations, Motion::Off);
}

#[test]
fn the_choice_is_saved_and_a_missing_or_unreadable_one_follows_the_system() {
    let ui = UiState { animations: Motion::Off, ..UiState::default() };
    let saved: Value = serde_json::to_value(&ui).unwrap();
    assert_eq!(saved["animations"], "off");
    let back: UiState = serde_json::from_value(saved).unwrap();
    assert_eq!(back.animations, Motion::Off);
    // A file from before there was a choice, and one with a value that isn't one: the rest is kept.
    let old: UiState = serde_json::from_value(json!({"theme": "dark"})).unwrap();
    assert_eq!((old.animations, old.theme), (Motion::System, crate::theme::Appearance::Dark));
    for junk in [json!("slow"), json!(5), json!(null), json!([1]), json!({"a": 1}), json!(true)] {
        let bad: UiState = serde_json::from_value(json!({"theme": "dark", "animations": junk})).unwrap();
        assert_eq!((bad.animations, bad.theme), (Motion::System, crate::theme::Appearance::Dark));
    }
}

/// Off stops egui's animations whatever the system says, On plays them whatever it says, and Use
/// system setting follows it; where it says nothing they play.
#[test]
fn the_window_follows_the_choice_and_the_system() {
    for (system, follows) in [(Some(false), false), (Some(true), true), (None, true)] {
        let mut a = app(system);
        let ctx = egui::Context::default();
        assert_eq!(animates(&mut a, &ctx), follows, "system {system:?}");
        a.execute("ui.animations", json!({"value": "off"})).unwrap();
        assert!(!animates(&mut a, &ctx));
        a.execute("ui.animations", json!({"value": "on"})).unwrap();
        assert!(animates(&mut a, &ctx));
        assert_eq!(ctx.global_style().animation_time, egui::Style::default().animation_time);
        a.execute("ui.animations", json!({"value": "system"})).unwrap();
        assert_eq!(animates(&mut a, &ctx), follows);
    }
}

/// A change of theme doesn't bring animations back.
#[test]
fn a_new_theme_keeps_animations_off() {
    let mut a = app(None);
    let ctx = egui::Context::default();
    a.execute("ui.animations", json!({"value": "off"})).unwrap();
    assert!(!animates(&mut a, &ctx));
    for theme in ["dark", "light"] {
        a.execute("ui.theme", json!({"value": theme})).unwrap();
        a.execute("ui.highContrast", json!({"on": theme == "dark"})).unwrap();
        assert!(!animates(&mut a, &ctx), "{theme}");
    }
}

/// The system is asked once every few seconds at most, not every frame.
#[test]
fn the_system_is_asked_again_only_after_a_while() {
    let asked = Rc::new(Cell::new(0));
    let counted = asked.clone();
    let ask = move || {
        counted.set(counted.get() + 1);
        false
    };
    let mut tracker = Tracker::default();
    assert_eq!(tracker.system(0.0, Some(&ask)), Some(false));
    assert_eq!(tracker.system(1.0, Some(&ask)), Some(false));
    assert_eq!(tracker.system(4.9, Some(&ask)), Some(false));
    assert_eq!(asked.get(), 1);
    assert_eq!(tracker.system(5.0, Some(&ask)), Some(false));
    assert_eq!(asked.get(), 2);
    // A clock that went back asks again rather than trusting an answer from the future.
    assert_eq!(tracker.system(1.0, Some(&ask)), Some(false));
    assert_eq!(asked.get(), 3);
    assert_eq!(tracker.system(9.0, None), None);
}

fn click(harness: &mut egui_kittest::Harness<'static, MailApp>, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

/// File › Options › General has the choice, in both themes.
#[test]
fn the_choice_is_in_options() {
    use egui_kittest::kittest::Queryable as _;
    for theme in ["light", "dark"] {
        let mut a = app(None);
        a.session.driver.set_auto_sync(false);
        a.execute("ui.theme", json!({"value": theme})).unwrap();
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1280.0, 800.0))
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
        let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
        harness.state_mut().execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        harness.run_steps(4);
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            harness.render().unwrap().save(std::path::Path::new(&dir).join(format!("animations-{theme}.png"))).unwrap();
        }
        // Chosen there, it is the window's at once.
        let combo = harness.state().hit.rect("optionAnimations", 0).expect("the choice in File › Options");
        click(&mut harness, combo.center());
        harness.query_by_label("Never play").expect("its items").click();
        harness.run_steps(3);
        assert_eq!(harness.state().ui.animations, Motion::Off, "{theme}");
        assert_eq!(harness.ctx.global_style().animation_time, 0.0);
    }
}

/// The system is asked only while the choice is to follow it.
#[test]
fn the_system_is_asked_only_to_follow_it() {
    let asked = Rc::new(Cell::new(0));
    let counted = asked.clone();
    let ask = move || {
        counted.set(counted.get() + 1);
        false
    };
    let mut tracker = Tracker::default();
    assert!(tracker.plays(Motion::On, 0.0, Some(&ask)));
    assert!(!tracker.plays(Motion::Off, 10.0, Some(&ask)));
    assert_eq!(asked.get(), 0);
    assert!(!tracker.plays(Motion::System, 20.0, Some(&ask)));
    assert_eq!(asked.get(), 1);
}

/// While animations play the styles are left as they are, and stopping them and playing them
/// again gives back what the styles had.
#[test]
fn playing_again_gives_back_what_the_styles_had() {
    let ctx = egui::Context::default();
    ctx.all_styles_mut(|s| s.animation_time = 0.2);
    let mut tracker = Tracker::default();
    tracker.apply(&ctx, true);
    assert_eq!(ctx.global_style().animation_time, 0.2);
    tracker.apply(&ctx, false);
    assert_eq!(ctx.global_style().animation_time, 0.0);
    tracker.apply(&ctx, true);
    assert_eq!(ctx.global_style().animation_time, 0.2);
}

/// Animations stopped while the mailbox is read stay stoppable and come back once it is open:
/// the window that takes over keeps what the styles had before (#1183, CodeRabbit on the PR).
#[test]
fn the_window_that_takes_over_from_the_opening_one_can_play_them_again() {
    use crate::opening::{Arrival, Build, Opening};
    let off = UiState { animations: Motion::Off, ..UiState::default() };
    let (tx, rx) = std::sync::mpsc::channel();
    let kept = off.clone();
    let build: Build = Box::new(move |arrival: Arrival| {
        let mut app = MailApp::new(Session::at(arrival.mailbox, NOW, 0), Services::default());
        app.autosave = false;
        app.ui = kept;
        app
    });
    let progress = std::sync::Arc::new(mailcraft_engine::io::OpenProgress::default());
    let mut a = MailApp::opening(Opening::new(rx, progress, build), Some(off), None);
    let ctx = egui::Context::default();
    assert!(!animates(&mut a, &ctx), "off while the mailbox is read");
    tx.send(Arrival::opened(mailcraft_engine::sample::mailbox(NOW, 0), None)).unwrap();
    a.logic(&ctx);
    assert!(a.opening.is_none(), "the mailbox is open");
    assert!(!animates(&mut a, &ctx), "still off once it is open");
    a.execute("ui.animations", json!({"value": "on"})).unwrap();
    assert!(animates(&mut a, &ctx));
    assert_eq!(ctx.global_style().animation_time, egui::Style::default().animation_time);
}
