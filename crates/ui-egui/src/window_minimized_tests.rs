// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A window of the system an agent opens while the main window is minimized (#1224): the backend
//! can't make it then (it never runs the callback, and egui panics), so it waits, open, until the
//! main window is on screen.

use std::cell::Cell;
use std::rc::Rc;

use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

/// A backend that makes a window only when asked from a main window that is on screen, as the
/// real one does; asked at any other time it does nothing, and egui's `expect` panics.
fn backend(asked: &Rc<Cell<u32>>) -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let asked = asked.clone();
    let made = Cell::new(false);
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        asked.set(asked.get() + 1);
        // A window that exists is shown whatever the main one does; one that doesn't is made only
        // from a main window that is on screen.
        if !made.get() && ctx.input(|i| i.viewport().visible()) == Some(false) {
            return;
        }
        made.set(true);
        let mut input = egui::RawInput { viewport_id: viewport.ids.this, ..Default::default() };
        input.viewports.insert(viewport.ids.this, egui::ViewportInfo::default());
        ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui)).textures_delta.clear();
    });
    ctx
}

fn frame(ctx: &egui::Context, app: &mut MailApp, minimized: bool) {
    let mut input = egui::RawInput::default();
    let info = egui::ViewportInfo { minimized: Some(minimized), occluded: Some(false), ..Default::default() };
    input.viewports.insert(egui::ViewportId::ROOT, info);
    ctx.run_ui(input, |ui| super::show(app, ui.ctx())).textures_delta.clear();
}

#[test]
fn a_window_opened_while_the_main_window_is_minimized_waits_for_it() {
    let asked = Rc::new(Cell::new(0));
    let ctx = backend(&asked);
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    let message = app.session.mailbox.messages.values().find(|m| !m.draft).map(|m| m.id).unwrap();
    app.execute("ui.openWindow", json!({"message": message})).unwrap();
    app.set_monitors(Vec::new());
    for _ in 0..3 {
        frame(&ctx, &mut app, true);
    }
    assert_eq!(asked.get(), 0, "the backend is not asked for a window while the main one is minimized");
    assert!(!app.windows.is_empty(), "the window is still open");
    // Restored: it is made, once.
    for _ in 0..3 {
        frame(&ctx, &mut app, false);
    }
    assert!(asked.get() >= 1);
    // Made, it goes on being shown if the main window is minimized again.
    let before = asked.get();
    frame(&ctx, &mut app, true);
    assert!(asked.get() > before);
}
