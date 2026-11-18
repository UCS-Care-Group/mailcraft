// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tighter spacing (`view.density`): the list's rows are shorter, and what is in them keeps its
//! place in them, half of what a row gave up above and half below. Classic Light's rows (#1203)
//! are shorter again, on top of it, in the same way.

use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn window(tight: bool, one_line: bool) -> Window {
    window_in("light", tight, one_line)
}

/// The window in the interface theme `theme` (`ui.theme`).
fn window_in(theme: &str, tight: bool, one_line: bool) -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.overlay_scroll_bars = false;
    app.execute("ui.theme", json!({"value": theme})).unwrap();
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

/// Classic Light (#1203) gives up a little more of every row, with tighter spacing on and off, and
/// Tighter Spacing still does what it says there.
#[test]
fn classic_light_rows_are_shorter_on_top_of_tighter_spacing() {
    for one_line in [false, true] {
        let full = if one_line { ONE_LINE_H } else { row_height(1) };
        let mut heights = Vec::new();
        for tight in [false, true] {
            let mut light = window_in("light", tight, one_line);
            let mut classic = window_in("classicLight", tight, one_line);
            let (light, classic) = (drawn(&mut light), drawn(&mut classic));
            assert!(classic.len() >= light.len() && classic.len() >= 4, "more rows fit: {} {}", classic.len(), light.len());
            let want = full - row_tightening(tight, true, one_line);
            assert!(want < full - tightening(tight, one_line), "single line {one_line}, tight {tight}");
            for (_, rect) in &classic {
                assert_eq!(rect.height(), want, "single line {one_line}, tight {tight}");
            }
            for (_, rect) in &light {
                assert_eq!(rect.height(), full - tightening(tight, one_line), "single line {one_line}, tight {tight}");
            }
            heights.push(want);
        }
        assert!(heights[1] < heights[0], "tighter spacing still tightens: {heights:?}");
    }
}

/// In a Classic Light row the flag keeps to the sender's line, and the row's marks stay inside it.
#[test]
fn the_flag_stays_on_the_senders_line_in_a_classic_light_row() {
    for tight in [false, true] {
        let mut harness = window_in("classicLight", tight, false);
        let rows = drawn(&mut harness);
        let Some((row, rect)) = rows.get(1).cloned() else { panic!("too few rows") };
        harness.event(egui::Event::PointerMoved(rect.center()));
        harness.run_steps(3);
        let flag = harness.state().hit.rect("messageFlag", row.id).unwrap_or_else(|| panic!("no flag on row {}", row.id));
        let lift = row_tightening(tight, true, false) / 2.0;
        assert!((flag.center().y - (rect.min.y + SENDER_Y - lift)).abs() < 0.01, "{flag:?} {rect:?}");
        assert!(flag.min.y >= rect.min.y && flag.max.y <= rect.max.y, "inside its row: {flag:?} {rect:?}");
    }
}

/// The previews drawn, each with the bottom of its lines and how far down it may be drawn.
fn previews(harness: &Window) -> Vec<(egui::Pos2, f32, f32)> {
    fn find(shape: &egui::Shape, clip: Rect, out: &mut Vec<(egui::Pos2, f32, f32)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| find(s, clip, out)),
            // (A preview is the one text laid out with the preview's line pitch.)
            egui::Shape::Text(t) if t.galley.job.sections.first().is_some_and(|s| s.format.line_height == Some(PREVIEW_PITCH)) => {
                out.push((t.pos, t.pos.y + t.galley.rect.max.y, clip.max.y));
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    harness.output().shapes.iter().for_each(|c| find(&c.shape, c.clip_rect, &mut out));
    out
}

/// A row short enough (Classic Light and Tighter Spacing both, #1203) still shows the whole of
/// its preview's line: it is cut off at the line under the row, not above it.
#[test]
fn the_preview_is_whole_in_the_shortest_rows() {
    for theme in ["light", "classicLight"] {
        for tight in [false, true] {
            let mut harness = window_in(theme, tight, false);
            let rows = drawn(&mut harness);
            let previews = previews(&harness);
            let mut seen = 0;
            for (_, rect) in rows.iter().take(4) {
                for (at, bottom, clip) in previews.iter().filter(|(at, _, _)| rect.contains(*at)) {
                    assert!(*bottom <= *clip, "{theme}, tight {tight}: the preview at {at:?} ends at {bottom}, cut at {clip} ({rect:?})");
                    assert!(*bottom <= rect.max.y - 1.0, "{theme}, tight {tight}: and inside its row ({rect:?})");
                    seen += 1;
                }
            }
            assert!(seen >= 4, "{theme}, tight {tight}: previews drawn: {seen} of {}", previews.len());
        }
    }
}
