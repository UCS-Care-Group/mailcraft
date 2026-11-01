// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tighter spacing (`view.density`): the list's rows are shorter, and what is in them keeps its
//! place in them, half of what a row gave up above and half below.

use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn window(tight: bool, one_line: bool) -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app.overlay_scroll_bars = false;
    app.execute("view.density", json!({"value": tight})).unwrap();
    app.execute("view.oneLine", json!({"value": one_line})).unwrap();
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

/// The rows that were drawn, top to bottom, each with where it is.
fn drawn(harness: &mut Window) -> Vec<(Row, Rect)> {
    let rows = harness.state_mut().session.rows();
    rows.iter().filter_map(|row| harness.state().hit.rect("message", row.id).map(|rect| (row.clone(), rect))).collect()
}

#[test]
fn tighter_spacing_makes_every_row_shorter() {
    for one_line in [false, true] {
        let mut normal = window(false, one_line);
        let mut tight = window(true, one_line);
        let (normal, tight) = (drawn(&mut normal), drawn(&mut tight));
        assert!(tight.len() >= normal.len() && tight.len() >= 4, "more rows fit: {} {}", tight.len(), normal.len());
        let full = if one_line { ONE_LINE_H } else { row_height(1) };
        let want = full - tightening(true, one_line);
        assert!(want < full);
        for (_, rect) in &tight {
            assert_eq!(rect.height(), want, "single line {one_line}");
        }
        for (_, rect) in &normal {
            assert_eq!(rect.height(), full, "single line {one_line}");
        }
    }
}

#[test]
fn the_flag_stays_on_the_senders_line_in_a_tighter_row() {
    let mut harness = window(true, false);
    let rows = drawn(&mut harness);
    let Some((row, rect)) = rows.get(1).cloned() else { panic!("too few rows") };
    harness.event(egui::Event::PointerMoved(rect.center()));
    harness.run_steps(3);
    let flag = harness.state().hit.rect("messageFlag", row.id).unwrap_or_else(|| panic!("no flag on row {}", row.id));
    let lift = tightening(true, false) / 2.0;
    assert!((flag.center().y - (rect.min.y + SENDER_Y - lift)).abs() < 0.01, "{flag:?} {rect:?}");
    assert!(flag.min.y >= rect.min.y && flag.max.y <= rect.max.y, "inside its row: {flag:?} {rect:?}");
}
