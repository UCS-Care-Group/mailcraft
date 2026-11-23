// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mouse wheel's notch moves the message list about three lines, as other desktop programs
//! do; a touchpad's points move it exactly as far as they say (#1513).

use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

/// The sample mailbox with 60 more messages in the Inbox, so that the list scrolls.
fn window() -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let inbox = app.session.view.folder;
    for i in 0..60u32 {
        let from = mailcraft_model::Address::new(format!("Sender {i}"), format!("s{i}@example.org"));
        let m = Message {
            folder: inbox,
            subject: format!("Message {i}"),
            from,
            date: NOW - 3600,
            received: NOW - 3600 - i64::from(i),
            ..Default::default()
        };
        app.session.mailbox.add_message(m).unwrap();
    }
    app.session.mailbox_changed();
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(vec2(1280.0, 800.0))
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

/// How far the first row moved up after one wheel event of `delta` in `unit` over the list.
fn moved(unit: egui::MouseWheelUnit, delta: f32) -> f32 {
    let mut harness = window();
    let first = harness.state_mut().session.rows().first().map(|r| r.id).unwrap();
    let before = harness.state().hit.rect("message", first).unwrap();
    harness.event(egui::Event::PointerMoved(before.center()));
    harness.step();
    harness.event(egui::Event::MouseWheel {
        unit,
        delta: vec2(0.0, delta),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    // egui eases a wheel's step over a few frames.
    harness.run_steps(120);
    let after = harness.state().hit.rect("message", first).unwrap();
    before.min.y - after.min.y
}

#[test]
fn a_wheel_notch_scrolls_about_three_lines() {
    let step = moved(egui::MouseWheelUnit::Line, -1.0);
    assert!(step >= 50.0, "one notch moved the list {step} points");
}

#[test]
fn a_touchpad_scrolls_exactly_its_points() {
    let step = moved(egui::MouseWheelUnit::Point, -30.0);
    assert!((step - 30.0).abs() < 0.5, "30 points moved the list {step}");
}
