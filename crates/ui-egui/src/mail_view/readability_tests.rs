// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where one message ends and the next begins (#1657): with View › Shade Alternate Rows every
//! other row wears a band, starting afresh under each heading, and a message with nothing to
//! preview has its two lines in the middle of its row rather than a blank under them.

use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Window = egui_kittest::Harness<'static, MailApp>;

fn window(app: MailApp) -> Window {
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(vec2(1280.0, 1400.0))
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

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    app.overlay_scroll_bars = false;
    app
}

/// The rows that were drawn, top to bottom, each with where it is.
fn drawn(harness: &mut Window) -> Vec<(Row, Rect)> {
    let rows = harness.state_mut().session.rows();
    rows.iter().filter_map(|row| harness.state().hit.rect("message", row.id).map(|rect| (row.clone(), rect))).collect()
}

#[test]
fn shaded_rows_alternate_under_each_heading() {
    let mut app = app();
    app.execute("view.shadeRows", json!({"value": true})).unwrap();
    let mut harness = window(app);
    let rows = drawn(&mut harness);
    assert!(rows.len() >= 6, "enough rows to see the pattern: {}", rows.len());
    assert!(rows.iter().filter(|(row, _)| row.group.is_some()).count() >= 2, "more than one heading");
    let mut nth = 0;
    for (row, _) in &rows {
        if row.group.is_some() {
            nth = 0;
        }
        let shaded = harness.state().hit.rect("messageShade", row.id).is_some();
        assert_eq!(shaded, nth % 2 == 1, "row {} is number {nth} under its heading", row.id);
        nth += 1;
    }
}

#[test]
fn rows_are_plain_until_shading_is_turned_on() {
    let mut harness = window(app());
    let rows = drawn(&mut harness);
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|(row, _)| harness.state().hit.rect("messageShade", row.id).is_none()));
}

#[test]
fn a_message_with_nothing_to_preview_sits_in_the_middle_of_its_row() {
    let mut app = app();
    let rows = app.session.rows();
    // The third row and the one after it: neither pinned, both under a date's heading.
    let (Some(empty), Some(other)) = (rows.get(2).map(|r| r.id), rows.get(3).map(|r| r.id)) else { panic!("too few rows") };
    let Some(m) = app.session.mailbox.messages.get_mut(&empty) else { panic!("no message {empty}") };
    let m = Arc::make_mut(m);
    m.body.html = None;
    m.body.text.clear();
    let mut harness = window(app);
    let rows = drawn(&mut harness);
    let rect_of = |id| rows.iter().find(|(row, _)| row.id == id).map(|(_, rect)| *rect).unwrap_or_else(|| panic!("row {id} not drawn"));
    let sender_y = |harness: &Window, id| harness.state().hit.rect("messageSender", id).unwrap_or_else(|| panic!("no sender on {id}")).center().y;
    let (empty_rect, other_rect) = (rect_of(empty), rect_of(other));
    assert_eq!(empty_rect.height(), other_rect.height(), "every row stays as tall");
    assert!((sender_y(&harness, other) - (other_rect.min.y + SENDER_Y)).abs() < 0.01, "a row with a preview is as it was");
    let drop = empty_preview_drop(1);
    assert!(drop > 0.0);
    assert!((sender_y(&harness, empty) - (empty_rect.min.y + SENDER_Y + drop)).abs() < 0.01, "the empty row's lines move down");
    // The blank is now shared about evenly above the sender's line and below the subject's.
    let (above, below) = (SENDER_Y + drop, empty_rect.height() - (SUBJECT_Y + drop));
    assert!((above - below).abs() < 3.0, "{above} above, {below} below");
}
