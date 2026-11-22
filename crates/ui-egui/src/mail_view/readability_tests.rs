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
    let banded = rows.iter().filter(|(row, _)| harness.state().hit.rect("messageShade", row.id).is_some()).count();
    assert!(banded >= 2, "rows wear the band: {banded}");
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

/// Where an empty row's sender and the next row's sender are, from each row's top, and how tall
/// the rows are, with `preview_lines` lines of preview and Tighter Spacing on or off.
fn empty_and_full_row(preview_lines: u8, tight: bool) -> (f32, f32, f32) {
    let mut app = app();
    app.session.prefs.preview_lines = preview_lines;
    app.session.prefs.tight_spacing = tight;
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
    let sender_at = |id| {
        let y = harness.state().hit.rect("messageSender", id).unwrap_or_else(|| panic!("no sender on {id}")).center().y;
        y - rect_of(id).min.y
    };
    let (empty_rect, other_rect) = (rect_of(empty), rect_of(other));
    assert_eq!(empty_rect.height(), other_rect.height(), "every row stays as tall ({preview_lines} lines, tight {tight})");
    (sender_at(empty), sender_at(other), empty_rect.height())
}

#[test]
fn a_message_with_nothing_to_preview_sits_in_the_middle_of_its_row() {
    // One, two and three lines of preview, with Tighter Spacing off and on.
    for lines in 1..=3 {
        for tight in [false, true] {
            let (empty, full, height) = empty_and_full_row(lines, tight);
            let drop = empty_preview_drop(lines);
            assert!(drop > 0.0);
            // A row with a preview is where it was; the empty row's lines move down by the drop.
            let lift = tightening(tight, false) / 2.0;
            assert!((full - (SENDER_Y - lift)).abs() < 0.01, "{lines} lines, tight {tight}: a row with a preview is at {full}");
            assert!((empty - (full + drop)).abs() < 0.01, "{lines} lines, tight {tight}: the empty row's lines move down by {drop}");
            // The blank is now shared about evenly above the sender's line and below the subject's.
            let (above, below) = (empty, height - (empty + SUBJECT_Y - SENDER_Y));
            assert!((above - below).abs() < 3.0, "{lines} lines, tight {tight}: {above} above, {below} below");
        }
    }
}

#[test]
fn what_sits_at_the_end_of_an_empty_row_moves_down_with_its_text() {
    // Arranged by size, a message not downloaded yet shows its size at the end of its row; with
    // nothing to preview it goes on the subject's line, which has moved down, not where the
    // preview's first line would have been.
    for lines in 1..=3 {
        let mut app = app();
        app.session.prefs.preview_lines = lines;
        app.execute("view.sort", json!({"by": "size"})).unwrap();
        let rows = app.session.rows();
        let Some(empty) = rows.get(2).map(|r| r.id) else { panic!("too few rows") };
        let Some(m) = app.session.mailbox.messages.get_mut(&empty) else { panic!("no message {empty}") };
        let m = Arc::make_mut(m);
        m.body.html = None;
        m.body.text.clear();
        m.not_downloaded = 4096;
        let harness = window(app);
        let hit = |name| harness.state().hit.rect(name, empty).unwrap_or_else(|| panic!("no {name} on {empty}"));
        let (sender, size) = (hit("messageSender").center().y, hit("messageSize").center().y);
        let subject = sender + SUBJECT_Y - SENDER_Y;
        assert!((size - subject).abs() < 2.0, "{lines} lines: the size is at {size}, the subject at {subject}");
    }
}

/// Where an empty row's sender is from its row's top, two frames after its body arrives and
/// once it has settled, with `preview_lines` lines of preview and animations `motion`.
fn sender_as_body_arrives(preview_lines: u8, motion: crate::motion::Motion) -> (f32, f32, f32) {
    let mut app = app();
    app.ui.animations = motion;
    app.session.prefs.preview_lines = preview_lines;
    let rows = app.session.rows();
    let Some(id) = rows.get(2).map(|r| r.id) else { panic!("too few rows") };
    let Some(m) = app.session.mailbox.messages.get_mut(&id) else { panic!("no message {id}") };
    let m = Arc::make_mut(m);
    m.body.html = None;
    m.body.text.clear();
    let mut harness = window(app);
    // The test harness has egui's animations off; the window's own setting stops them or not.
    if motion != crate::motion::Motion::Off {
        harness.ctx.all_styles_mut(|s| s.animation_time = egui::Style::default().animation_time);
    }
    let sender_at = |harness: &Window| {
        let hit = &harness.state().hit;
        let (Some(sender), Some(row)) = (hit.rect("messageSender", id), hit.rect("message", id)) else { panic!("row {id} not drawn") };
        sender.center().y - row.min.y
    };
    let empty = sender_at(&harness);
    // The body arrives, as a new copy of the message the way a synchronisation brings it.
    let Some(old) = harness.state().session.mailbox.messages.get(&id).cloned() else { panic!("no message {id}") };
    let mut arrived = (*old).clone();
    arrived.body.text = "Here is what the message says, now that it has been downloaded.".into();
    harness.state_mut().session.mailbox.messages.insert(id, Arc::new(arrived));
    harness.run_steps(2);
    let easing = sender_at(&harness);
    harness.run_steps(30);
    (empty, easing, sender_at(&harness))
}

#[test]
fn an_empty_rows_lines_ease_up_when_its_body_arrives() {
    use crate::motion::Motion;
    for lines in 1..=3 {
        let drop = empty_preview_drop(lines);
        let (empty, easing, settled) = sender_as_body_arrives(lines, Motion::On);
        assert!((empty - settled - drop).abs() < 0.01, "{lines} lines: the lines end up {drop} higher: {empty} then {settled}");
        assert!(easing < empty && easing > settled, "{lines} lines: on their way, not there at once: {empty}, {easing}, {settled}");
        // With Animations off they are there at once.
        let (empty, easing, settled) = sender_as_body_arrives(lines, Motion::Off);
        assert!(
            (empty - settled - drop).abs() < 0.01 && (easing - settled).abs() < 0.01,
            "{lines} lines, no animations: {empty}, {easing}, {settled}"
        );
    }
}

#[test]
fn results_on_the_server_alternate_too() {
    use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult, GraphMessage, GraphSearchResult, ServerMessage};
    const OWNER: &str = "owner@example.org";
    let mut app = app();
    app.execute("view.shadeRows", json!({"value": true})).unwrap();
    // A Microsoft account with its Inbox, searched everywhere for what nothing here says.
    let s = &mut app.session;
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let inbox = GraphFolder { id: "in".into(), name: "Inbox".into(), kind: mailcraft_model::FolderKind::Inbox, ..Default::default() };
    let listing = FolderListing { folders: vec![inbox], complete: true };
    let result = GraphMailResult { account: owner, email: OWNER.into(), listing: Some(listing), ..Default::default() };
    s.run("sync.applyGraph", &json!({"result": result})).unwrap();
    s.run("view.search", &json!({"query": "quokka", "scope": "everywhere"})).unwrap();
    let asked = s.run("sync.graphSearchRequest", &json!({})).unwrap()["requests"][0].clone();
    let messages: Vec<GraphMessage> = (0..4)
        .map(|i| GraphMessage {
            server: ServerMessage { id: format!("AAkA-{i}"), folder: "in".into(), ..Default::default() },
            subject: format!("Quokka {i}"),
            received: NOW - 60 * (i + 1),
            ..Default::default()
        })
        .collect();
    let found = GraphSearchResult { account: owner, email: OWNER.into(), ticket: asked["ticket"].as_u64().unwrap(), messages, ..Default::default() };
    s.run("sync.applyGraphSearch", &json!({"result": found})).unwrap();
    let harness = window(app);
    let hit = &harness.state().hit;
    assert!(hit.rect("onTheServer", 0).is_some(), "the server's heading is drawn");
    for i in 0..4 {
        assert!(hit.rect("serverResult", i).is_some(), "result {i} is drawn");
        assert_eq!(hit.rect("serverResultShade", i).is_some(), i % 2 == 1, "result {i}, number {i} under its heading");
    }
}
