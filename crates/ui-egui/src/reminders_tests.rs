// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Reminders window in a window of the system, while the main window has no passes (#771).

use egui::{RawInput, ViewportCommand, pos2, vec2};
use mailcraft_engine::Session;

use super::*;

// Wednesday 2026-10-14 16:00 UTC: the sample has reminders due then.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), crate::Services::default());
    app.autosave = false;
    app
}

fn screen() -> Option<egui::Rect> {
    Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0)))
}

/// A pass of the main window, logic and layout, as eframe runs it while the window is on screen;
/// answers the Reminders window's own pass, when it has one.
fn main_pass(ctx: &egui::Context, app: &mut MailApp, time: &mut f64) -> Option<std::sync::Arc<egui::DeferredViewportUiCallback>> {
    *time += 1.0 / 60.0;
    let output = ctx.run_ui(RawInput { screen_rect: screen(), time: Some(*time), ..Default::default() }, |ui| {
        let ctx = ui.ctx().clone();
        app.logic(&ctx);
        app.ui(ui);
    });
    let paint = output.viewport_output.get(&viewport()).and_then(|v| v.viewport_ui_cb.clone());
    output.drop_without_applying_deltas();
    paint
}

/// The main window's logic alone, as eframe runs it while the window is minimized and the
/// compositor gives it no frames; answers the commands it sent to the Reminders window.
fn logic_only(ctx: &egui::Context, app: &mut MailApp) -> Vec<ViewportCommand> {
    let mut output = ctx.run_logic(&RawInput::default(), |ctx| app.logic(ctx));
    output.viewport_commands.remove(&viewport()).unwrap_or_default()
}

/// A pass of the Reminders window of its own, with `events`.
fn reminders_pass(ctx: &egui::Context, paint: &egui::DeferredViewportUiCallback, time: &mut f64, events: Vec<egui::Event>) {
    *time += 1.0 / 60.0;
    let mut input = RawInput {
        viewport_id: viewport(),
        screen_rect: Some(egui::Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 300.0))),
        time: Some(*time),
        events,
        ..Default::default()
    };
    input.viewports.insert(viewport(), egui::ViewportInfo::default());
    ctx.run_ui(input, |ui| paint(ui)).drop_without_applying_deltas();
}

fn click(ctx: &egui::Context, paint: &egui::DeferredViewportUiCallback, time: &mut f64, pos: egui::Pos2) {
    reminders_pass(ctx, paint, time, vec![egui::Event::PointerMoved(pos)]);
    for pressed in [true, false] {
        let button = egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        reminders_pass(ctx, paint, time, vec![egui::Event::PointerMoved(pos), button]);
    }
}

/// Dismiss in the Reminders window is carried out by the main window's logic alone, with no
/// pass of the main window; and a window no longer wanted is put out of sight the same way.
#[test]
fn a_reminder_is_dismissed_while_the_main_window_has_no_passes() {
    let mut app = app();
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let mut time = 0.0;
    let mut paint = None;
    for _ in 0..6 {
        paint = main_pass(&ctx, &mut app, &mut time).or(paint);
    }
    let paint = paint.expect("the Reminders window has passes of its own");
    let before = app.reminders.showing.clone();
    assert!(before.len() >= 2, "the sample has reminders due: {before:?}");
    let first = before[0].clone();

    // The main window is minimized from here on: its logic runs, its layout doesn't.
    reminders_pass(&ctx, paint.as_ref(), &mut time, Vec::new());
    let row = lock(&app.reminders.shared).hits.iter().find(|(kind, id, _)| *kind == "reminder" && *id == first.event).map(|h| h.2);
    let row = row.expect("the reminder is laid out in its window");
    // Dismiss is at the row's right end.
    click(&ctx, paint.as_ref(), &mut time, pos2(row.max.x - 20.0, row.center().y));
    assert_eq!(lock(&app.reminders.shared).actions, [Action::Dismiss(first.event, first.start.iso())]);
    assert!(logic_only(&ctx, &mut app).is_empty());
    assert!(app.ui.dismissed_reminders.contains(&key_of(&first)));
    assert!(!app.reminders.showing.contains(&first));
    // What the window shows follows at once, for its next pass.
    assert_eq!(lock(&app.reminders.shared).rows.len(), before.len() - 1);
    assert!(lock(&app.reminders.shared).actions.is_empty(), "carried out once");

    // Dismiss All: nothing is left to show, and with no pass of the main window to take the
    // window away it is minimized, once.
    lock(&app.reminders.shared).actions.push(Action::DismissAll);
    assert!(logic_only(&ctx, &mut app).is_empty(), "the main window's next pass may yet take it away");
    assert!(app.reminders.showing.is_empty());
    assert_eq!(logic_only(&ctx, &mut app), [ViewportCommand::Minimized(true)]);
    assert!(logic_only(&ctx, &mut app).is_empty());

    // Back on screen, the main window lets it go.
    assert!(main_pass(&ctx, &mut app, &mut time).is_none());
    assert!(!app.reminders.minimized && !app.reminders.on_screen);
}

/// On screen, the main window's pass takes a window no longer wanted away itself: nothing is
/// minimized. And one that was minimized is made afresh when it is wanted again.
#[test]
fn a_window_on_screen_closes_without_being_minimized() {
    let mut app = app();
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let mut time = 0.0;
    for _ in 0..6 {
        main_pass(&ctx, &mut app, &mut time);
    }
    assert!(app.reminders.on_screen);
    lock(&app.reminders.shared).actions.push(Action::Close);
    // Closed with reminders showing: they are put off.
    assert!(main_pass(&ctx, &mut app, &mut time).is_none());
    assert!(app.reminders.showing.is_empty() && !app.reminders.snoozed.is_empty());
    for _ in 0..3 {
        assert!(logic_only(&ctx, &mut app).is_empty());
        main_pass(&ctx, &mut app, &mut time);
    }
    assert!(!app.reminders.minimized);

    // Asked for again after it was minimized: let go for a pass, then made afresh.
    app.reminders.minimized = true;
    app.reminders.asked_for = true;
    assert!(main_pass(&ctx, &mut app, &mut time).is_none());
    assert!(main_pass(&ctx, &mut app, &mut time).is_some());
    // Its Close with nothing due takes the request back.
    lock(&app.reminders.shared).actions.push(Action::Close);
    assert!(main_pass(&ctx, &mut app, &mut time).is_none());
    assert!(!app.reminders.asked_for);
}
