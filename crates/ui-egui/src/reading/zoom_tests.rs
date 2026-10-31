// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The zoom of the message being read (`view.zoom`): its text is laid out again larger or
//! smaller, in the reading pane, and Ctrl with the wheel over it steps it.

use egui::vec2;
use mailcraft_engine::Session;
use serde_json::json;

use super::kept;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn window() -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(vec2(1280.0, 900.0))
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

/// The message in the reading pane, and how tall and wide its text was laid out, with the size
/// of its first piece of text.
fn laid(harness: &mut Window) -> (f32, f32) {
    harness.run_steps(6);
    let Some(id) = harness.state().session.sel.focus() else { panic!("nothing selected") };
    let first = |laid: &super::Laid| {
        laid.items.iter().find_map(|i| match &i.kind {
            super::ItemKind::Text(t) => t.job.sections.first().map(|s| s.format.font_id.size),
            _ => None,
        })
    };
    kept(&harness.ctx, id, |laid, _| (laid.size.y, first(laid).unwrap_or(0.0))).unwrap_or_else(|| panic!("message {id} not laid out"))
}

#[test]
fn zoom_lays_the_message_out_larger_and_back() {
    let mut harness = window();
    harness.state_mut().execute("mail.first", json!({})).unwrap();
    let (height, size) = laid(&mut harness);
    assert!(height > 0.0 && size > 0.0);
    harness.state_mut().execute("view.zoom", json!({"value": 200})).unwrap();
    let (big_height, big_size) = laid(&mut harness);
    assert!((big_size - 2.0 * size).abs() < 0.01, "{size} → {big_size}");
    assert!(big_height > height, "{height} → {big_height}");
    harness.state_mut().execute("view.zoomReset", json!({})).unwrap();
    assert_eq!(laid(&mut harness), (height, size));
}

#[test]
fn ctrl_and_the_wheel_over_the_message_zoom_it() {
    let mut harness = window();
    harness.state_mut().execute("mail.first", json!({})).unwrap();
    let _ = laid(&mut harness);
    let Some(id) = harness.state().session.sel.focus() else { panic!("nothing selected") };
    let Some(origin) = kept(&harness.ctx, id, |_, origin| origin) else { panic!("not laid out") };
    harness.event(egui::Event::PointerMoved(origin + vec2(40.0, 20.0)));
    harness.step();
    harness.event(egui::Event::Zoom(1.25));
    harness.run_steps(2);
    assert_eq!(harness.state().session.prefs.zoom, 110, "one step");
    harness.event(egui::Event::Zoom(0.8));
    harness.run_steps(2);
    assert_eq!(harness.state().session.prefs.zoom, 100, "and back");
}
