// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list's height with the reading pane at the bottom (#1372): kept as the owner
//! dragged the splitter under it, and only then, through the saved settings to the next start.

use egui::{Event, Modifiers, PointerButton, Pos2, Vec2};
use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::{Services, UiState};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

/// The main window `height` tall, started from the settings `saved` (as `ui.json` holds them), the
/// reading pane at the bottom.
fn window(height: f32, saved: Option<&str>) -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.overlay_scroll_bars = false;
    if let Some(saved) = saved {
        app.apply_prefs(serde_json::from_str::<UiState>(saved).unwrap());
    }
    app.execute("view.readingPane", json!({"value": "bottom"})).unwrap();
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(vec2(1280.0, height))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(4);
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    harness
}

/// The settings the window would write to `ui.json` when it closes.
fn saved(harness: &Window) -> String {
    serde_json::to_string(&harness.state().prefs()).unwrap()
}

fn list(harness: &Window) -> Rect {
    harness.state().hit.rect("messageList", 0).unwrap_or_else(|| panic!("no message list"))
}

/// Where the splitter under the list is grabbed.
fn splitter(harness: &Window) -> Pos2 {
    let list = list(harness);
    pos2(list.center().x, list.max.y)
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

fn press(harness: &mut Window, at: Pos2) {
    harness.event(Event::PointerMoved(at));
    harness.step();
    harness.event(button(at, true));
    harness.step();
}

/// A drag of the splitter by `by` in `steps` moves, one a frame, let go in a frame of its own.
fn drag(harness: &mut Window, by: Vec2, steps: u16) {
    let from = splitter(harness);
    press(harness, from);
    for n in 1..=steps {
        harness.event(Event::PointerMoved(from + by * (f32::from(n) / f32::from(steps))));
        harness.step();
    }
    harness.event(button(from + by, false));
    harness.run_steps(3);
}

#[test]
fn a_new_install_keeps_the_share_of_the_window_until_the_owner_drags() {
    let harness = window(900.0, None);
    assert!(list(&harness).height() > 140.0, "{:?}", list(&harness));
    assert_eq!(harness.state().ui.list_height, 0.0);
    assert!(saved(&harness).contains("\"listHeight\":0.0"), "{}", saved(&harness));
}

#[test]
fn the_dragged_height_is_saved_and_comes_back_at_the_next_start() {
    let mut harness = window(900.0, None);
    let before = list(&harness);
    drag(&mut harness, vec2(0.0, 120.0), 4);
    let after = list(&harness);
    assert!((after.height() - (before.height() + 120.0)).abs() < 4.0, "{before:?} {after:?}");
    assert!((harness.state().ui.list_height - after.height()).abs() < 0.5, "{} {after:?}", harness.state().ui.list_height);

    let again = window(900.0, Some(&saved(&harness)));
    assert!((list(&again).height() - after.height()).abs() < 0.5, "{:?} {after:?}", list(&again));
}

#[test]
fn a_height_saved_in_a_short_window_comes_back_in_a_taller_one() {
    let mut tall = window(1000.0, None);
    let grown = 610.0 - list(&tall).height();
    drag(&mut tall, vec2(0.0, grown), 4);
    let kept = tall.state().ui.list_height;
    assert!((kept - 610.0).abs() < 4.0, "{kept}");

    // Too short for it: the list is laid out shorter, and the height kept stays.
    let short = window(500.0, Some(&saved(&tall)));
    assert!(list(&short).height() < kept - 100.0, "{:?} {kept}", list(&short));
    assert_eq!(short.state().ui.list_height, kept);

    let tall_again = window(1000.0, Some(&saved(&short)));
    assert!((list(&tall_again).height() - kept).abs() < 0.5, "{:?} {kept}", list(&tall_again));
}

#[test]
fn the_last_move_is_kept_when_the_button_is_let_go_in_the_same_frame() {
    let mut harness = window(900.0, None);
    let from = splitter(&harness);
    let start = list(&harness).height();
    press(&mut harness, from);
    harness.event(Event::PointerMoved(from + vec2(0.0, 40.0)));
    harness.step();
    // The last move and the release come in one frame (`event` would give each a frame of its own).
    let to = from + vec2(0.0, 120.0);
    harness.input_mut().events.extend([Event::PointerMoved(to), button(to, false)]);
    harness.step();
    harness.run_steps(3);
    let shown = list(&harness).height();
    assert!((shown - (start + 120.0)).abs() < 4.0, "{start} {shown}");
    assert!((harness.state().ui.list_height - shown).abs() < 0.5, "{} {shown}", harness.state().ui.list_height);
}

#[test]
fn a_slow_drag_of_half_a_point_a_frame_is_kept() {
    let mut harness = window(900.0, None);
    let start = list(&harness).height();
    drag(&mut harness, vec2(0.0, 20.0), 40);
    let shown = list(&harness).height();
    assert!((shown - (start + 20.0)).abs() < 2.0, "{start} {shown}");
    assert!((harness.state().ui.list_height - shown).abs() < 0.5, "{} {shown}", harness.state().ui.list_height);
}
