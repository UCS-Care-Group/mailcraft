// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A dialog a window of the system opens for its message being written comes up over that window
//! (#1712), as [`super::show`] draws it: a stand-in for the backend runs each window and hands it
//! the pointer events queued for it.

use std::cell::RefCell;
use std::rc::Rc;

use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The events for the windows of the system, one pass's worth at a time.
type Queue = Rc<RefCell<Vec<Vec<egui::Event>>>>;

fn backend() -> (egui::Context, Queue) {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let queue: Queue = Rc::default();
    let events = queue.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let mut input = egui::RawInput { viewport_id: viewport.ids.this, ..Default::default() };
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 60.0), egui::vec2(980.0, 720.0));
        let info = egui::ViewportInfo {
            native_pixels_per_point: Some(1.0),
            inner_rect: Some(rect),
            outer_rect: Some(rect),
            focused: Some(true),
            ..Default::default()
        };
        input.viewports.insert(viewport.ids.this, info);
        let mut queued = events.borrow_mut();
        if !queued.is_empty() {
            input.events = queued.remove(0);
        }
        drop(queued);
        let mut out = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
        out.textures_delta.clear();
    });
    (ctx, queue)
}

/// One frame of the main window, which shows the windows of their own.
fn frame(ctx: &egui::Context, app: &mut MailApp) {
    frame_then(ctx, app, |_| {});
}

/// One frame of the main window, which shows the windows of their own and then does `after`, as
/// its own ribbon and keys do later in the frame.
fn frame_then(ctx: &egui::Context, app: &mut MailApp, after: impl FnOnce(&mut MailApp)) {
    let mut input = egui::RawInput::default();
    let info = egui::ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(false), ..Default::default() };
    input.viewports.insert(egui::ViewportId::ROOT, info);
    let mut after = Some(after);
    let mut out = ctx.run_ui(input, |ui| {
        super::show(app, ui.ctx());
        if let Some(after) = after.take() {
            after(app);
        }
    });
    out.textures_delta.clear();
}

/// One frame in which the main window, not the window that opened it, draws the dialog (as it does
/// when that window is inside the main one, or gone), with `events` in the main window.
fn main_window_draws(ctx: &egui::Context, app: &mut MailApp, events: Vec<egui::Event>) {
    let mut input = egui::RawInput { events, ..Default::default() };
    let info = egui::ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(true), ..Default::default() };
    input.viewports.insert(egui::ViewportId::ROOT, info);
    let mut out = ctx.run_ui(input, |ui| {
        crate::keys::gate_main(app, ui.ctx());
        crate::dialogs::show(app, ui.ctx());
    });
    out.textures_delta.clear();
}

fn escape() -> Vec<egui::Event> {
    [true, false]
        .map(|pressed| egui::Event::Key { key: egui::Key::Escape, physical_key: None, pressed, repeat: false, modifiers: egui::Modifiers::NONE })
        .to_vec()
}

/// A new message popped out, with Insert Link opened from its ribbon.
fn link_over_a_pop_out() -> (egui::Context, MailApp, u64, Queue) {
    let (ctx, queue) = backend();
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let compose = app.execute("mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    app.execute("compose.popOut", json!({"compose": compose})).unwrap();
    // It waits a frame for the host's list of monitors (none here: the system places it), then opens.
    for _ in 0..3 {
        frame(&ctx, &mut app);
    }
    let window = app.windows.of_compose(compose).expect("a window of its own");
    let link = app.hit.rect_in(window, "ribbon", crate::ribbon::hit_id("compose.link")).expect("Link in its ribbon").center();
    // Link pressed in the window.
    queue.borrow_mut().extend([
        vec![egui::Event::PointerMoved(link)],
        vec![egui::Event::PointerButton { pos: link, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE }],
        vec![egui::Event::PointerButton { pos: link, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE }],
    ]);
    for _ in 0..4 {
        frame(&ctx, &mut app);
    }
    (ctx, app, compose, queue)
}

#[test]
fn insert_link_pressed_in_a_pop_out_comes_up_over_it() {
    let (_ctx, app, compose, _queue) = link_over_a_pop_out();
    assert_eq!(app.dialog.as_ref().map(crate::dialogs::Dialog::name), Some("link"));
    // The window drew it over itself: the main window, behind it, doesn't.
    assert!(app.windows.dialog_shown, "Insert Link wasn't drawn over the window it was pressed in");
    assert_eq!(app.session.compose_id(), Some(compose));
}

#[test]
fn a_main_window_dialog_replaces_the_pop_outs() {
    let (ctx, mut app, _compose, _queue) = link_over_a_pop_out();
    assert!(app.windows.dialog_shown);
    // All Categories from the main window takes Insert Link's place. The next frame draws it in
    // the main window: the pop-out, which drew the one it opened, does not draw this one.
    app.windows.dialog_shown = false;
    app.run("category.list", json!({})).unwrap();
    frame(&ctx, &mut app);
    assert_eq!(app.dialog.as_ref().map(crate::dialogs::Dialog::name), Some("categories"));
    assert!(!app.windows.dialog_shown, "All Categories was drawn over the pop-out");
    assert_eq!(super::dialog_compose(&app), None);
}

#[test]
fn a_dialog_the_main_window_opens_in_the_frame_the_pop_outs_closes_is_the_main_windows() {
    let (ctx, mut app, _compose, queue) = link_over_a_pop_out();
    assert_eq!(app.dialog.as_ref().map(crate::dialogs::Dialog::name), Some("link"));
    // One frame: Escape in the pop-out answers Insert Link, then the main window, after the
    // windows, opens Folder Properties.
    queue.borrow_mut().push(escape());
    let mut closed = (false, Some(0));
    frame_then(&ctx, &mut app, |app| {
        closed = (app.dialog.is_none(), app.windows.dialog_for);
        app.run("folder.properties", json!({})).unwrap();
    });
    assert!(closed.0, "Escape didn't answer Insert Link in the pop-out");
    // Answering it let go of its owner there and then, before anything else could open.
    assert_eq!(closed.1, None, "Insert Link's owner outlived it");
    assert_eq!(app.dialog.as_ref().map(crate::dialogs::Dialog::name), Some("folderProperties"));
    assert_eq!((app.windows.dialog_for, super::dialog_compose(&app)), (None, None), "Folder Properties took Insert Link's owner");
    // The next frame draws it in the main window, not over the pop-out.
    app.windows.dialog_shown = false;
    frame(&ctx, &mut app);
    assert_eq!(app.dialog.as_ref().map(crate::dialogs::Dialog::name), Some("folderProperties"));
    assert!(!app.windows.dialog_shown, "Folder Properties was drawn over the pop-out");
}

#[test]
fn a_dialog_the_main_window_answers_lets_go_of_the_window_that_opened_it() {
    let (ctx, mut app, compose, _queue) = link_over_a_pop_out();
    assert_eq!(app.windows.dialog_for, Some(compose));
    // Escape in the main window, once it is drawn there: Insert Link is answered, and its owner
    // goes with it there and then, not at the start of the next frame.
    main_window_draws(&ctx, &mut app, Vec::new());
    assert_eq!(app.windows.dialog_for, Some(compose));
    main_window_draws(&ctx, &mut app, escape());
    assert!(app.dialog.is_none(), "Escape didn't answer Insert Link in the main window");
    assert_eq!(app.windows.dialog_for, None, "Insert Link's owner outlived it");
}
