// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A window of the system that was minimized while the main window was minimized too, and is
//! given back by the system (#1314). eframe runs a pass of the main window, which draws the
//! window, only while it thinks one of them is on screen, and learns that the window is on screen
//! only in such a pass: the window stays as eframe last saw it, minimized, and its close button and
//! everything in it do nothing. The main window's logic, which still runs, asks for that pass.

use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

/// A backend that makes and draws the windows it is asked for.
fn backend() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let mut input = egui::RawInput { viewport_id: viewport.ids.this, ..Default::default() };
        input.viewports.insert(viewport.ids.this, egui::ViewportInfo::default());
        ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui)).textures_delta.clear();
    });
    ctx
}

/// The app with a message open in a window of the system, made while the main window was on
/// screen.
fn with_window(ctx: &egui::Context) -> (MailApp, egui::ViewportId) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let message = app.session.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    app.execute("ui.openWindow", json!({"message": message})).unwrap();
    app.set_monitors(Vec::new());
    for _ in 0..3 {
        let mut input = egui::RawInput::default();
        input.viewports.insert(egui::ViewportId::ROOT, info(false, vec![]));
        ctx.run_ui(input, |ui| super::show(&mut app, ui.ctx())).textures_delta.clear();
    }
    let viewport = super::viewports(&app).into_iter().next().expect("the window is open");
    (app, viewport)
}

/// A window as eframe on Windows sees it: minimized or not, never told whether it is covered.
fn info(minimized: bool, events: Vec<egui::ViewportEvent>) -> egui::ViewportInfo {
    egui::ViewportInfo { minimized: Some(minimized), events, ..Default::default() }
}

/// The main window's logic alone, as eframe runs it while nothing it knows of is on screen: the
/// main window `main_minimized`, the window as eframe last saw it (`window`). Whether it asked
/// for a pass of the main window, painted unseen.
fn logic(ctx: &egui::Context, app: &mut MailApp, viewport: egui::ViewportId, main_minimized: bool, window: egui::ViewportInfo) -> bool {
    let mut input = egui::RawInput::default();
    input.viewports.insert(egui::ViewportId::ROOT, info(main_minimized, vec![]));
    input.viewports.insert(viewport, window);
    let mut output = ctx.run_logic(&input, |ctx| app.logic(ctx));
    let commands = output.viewport_commands.remove(&egui::ViewportId::ROOT).unwrap_or_default();
    commands.contains(&egui::ViewportCommand::RequestPaintWhileHidden)
}

#[test]
fn a_window_minimized_with_the_main_window_has_the_main_window_painted_unseen() {
    let ctx = backend();
    let (mut app, viewport) = with_window(&ctx);
    assert!(logic(&ctx, &mut app, viewport, true, info(true, vec![])), "the window may be back on screen: a pass is asked for");
    assert!(
        logic(&ctx, &mut app, viewport, true, info(true, vec![egui::ViewportEvent::Close])),
        "its close button was pressed: a pass is asked for at once, however recent the last"
    );
}

#[test]
fn nothing_is_painted_unseen_when_eframe_runs_the_pass_itself() {
    let ctx = backend();
    let (mut app, viewport) = with_window(&ctx);
    assert!(!logic(&ctx, &mut app, viewport, false, info(true, vec![])), "the main window is on screen");
    assert!(!logic(&ctx, &mut app, viewport, true, info(false, vec![])), "the window is on screen, as eframe knows");
}
