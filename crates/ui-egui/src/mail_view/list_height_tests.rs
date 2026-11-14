// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The message list's height with the reading pane at the bottom (#1372): kept as the owner
//! dragged it, and only then.

use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

/// The main window `height` tall, the reading pane at the bottom, the list's height kept as `kept`.
fn window(height: f32, kept: f32) -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.overlay_scroll_bars = false;
    app.execute("view.readingPane", json!({"value": "bottom"})).unwrap();
    app.ui.list_height = kept;
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

fn list(harness: &Window) -> Rect {
    harness.state().hit.rect("messageList", 0).unwrap_or_else(|| panic!("no message list"))
}

fn drag(harness: &mut Window, from: egui::Pos2, by: egui::Vec2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    harness.event(egui::Event::PointerMoved(from));
    harness.step();
    harness.event(button(from, true));
    harness.step();
    for n in 1..=4 {
        harness.event(egui::Event::PointerMoved(from + by * (n as f32 / 4.0)));
        harness.step();
    }
    harness.event(button(from + by, false));
    harness.run_steps(3);
}

#[test]
fn a_new_install_keeps_the_share_of_the_window_until_the_owner_drags() {
    let harness = window(900.0, 0.0);
    assert!(list(&harness).height() > 140.0, "{:?}", list(&harness));
    assert_eq!(harness.state().ui.list_height, 0.0);
}

#[test]
fn the_dragged_height_is_kept_and_comes_back() {
    let mut harness = window(900.0, 0.0);
    let before = list(&harness);
    drag(&mut harness, pos2(before.center().x, before.max.y), vec2(0.0, 120.0));
    let after = list(&harness);
    assert!((after.height() - (before.height() + 120.0)).abs() < 4.0, "{before:?} {after:?}");
    let kept = harness.state().ui.list_height;
    assert!((kept - after.height()).abs() < 0.5, "{kept} {after:?}");

    let again = window(900.0, kept);
    assert!((list(&again).height() - kept).abs() < 0.5, "{:?} {kept}", list(&again));
}

#[test]
fn a_short_window_lays_the_list_out_shorter_without_forgetting_its_height() {
    let short = window(500.0, 610.0);
    assert!(list(&short).height() < 610.0, "{:?}", list(&short));
    assert_eq!(short.state().ui.list_height, 610.0);

    let tall = window(1000.0, short.state().ui.list_height);
    assert!((list(&tall).height() - 610.0).abs() < 0.5, "{:?}", list(&tall));
}

#[test]
fn only_a_drag_that_changed_the_height_is_kept() {
    assert_eq!(dragged_height(610.0, Some(360.0), 360.0, true), 610.0);
    assert_eq!(dragged_height(610.0, Some(400.0), 360.0, false), 610.0);
    assert_eq!(dragged_height(0.0, None, 360.0, true), 0.0);
    assert_eq!(dragged_height(0.0, Some(360.0), 480.0, true), 480.0);
}
