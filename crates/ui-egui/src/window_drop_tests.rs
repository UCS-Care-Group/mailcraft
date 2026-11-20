// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A file let go over a window: a New Email in a window of its own takes what is let go over it,
//! though the keyboard is still the desktop's; a picture goes into the text of a message in HTML,
//! anything else (and a picture in plain text) is attached. A stand-in for the backend opens the
//! windows and hands each its own input, as the desktop's does.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use mailcraft_engine::Session;
use mailcraft_model::rich::OBJECT;
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A file as the desktop hands it over: its path.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

fn handed(paths: Vec<PathBuf>) -> Vec<egui::DroppedFileHandle> {
    paths.into_iter().map(|p| Arc::new(Dropped(p)) as egui::DroppedFileHandle).collect()
}

/// The backend: every window of the system it opens is given what waits in `drops` (let go over
/// it), and never the keyboard.
fn backend() -> (egui::Context, Rc<RefCell<Vec<PathBuf>>>) {
    let ctx = egui::Context::default();
    ctx.set_embed_viewports(false);
    let drops = Rc::new(RefCell::new(Vec::new()));
    let waiting = drops.clone();
    egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
        let info = egui::ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(false), ..Default::default() };
        let mut input = egui::RawInput { viewport_id: viewport.ids.this, ..Default::default() };
        input.viewports.insert(viewport.ids.this, info);
        input.dropped_files = handed(std::mem::take(&mut *waiting.borrow_mut()));
        let mut out = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
        out.textures_delta.clear();
    });
    (ctx, drops)
}

/// One frame of the main window, which shows the windows of their own; `drops` let go over it.
fn frame(ctx: &egui::Context, app: &mut MailApp, drops: Vec<PathBuf>) {
    let mut input = egui::RawInput { dropped_files: handed(drops), ..Default::default() };
    let info = egui::ViewportInfo { native_pixels_per_point: Some(1.0), focused: Some(false), ..Default::default() };
    input.viewports.insert(egui::ViewportId::ROOT, info);
    let mut out = ctx.run_ui(input, |ui| {
        let ctx = ui.ctx().clone();
        app.logic(&ctx);
        app.ui(ui);
    });
    out.textures_delta.clear();
}

/// The window, past the frames its fonts take to settle.
fn app(ctx: &egui::Context) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    settle(ctx, &mut app);
    app
}

fn settle(ctx: &egui::Context, app: &mut MailApp) {
    for _ in 0..3 {
        frame(ctx, app, vec![]);
    }
}

/// A New Email in a window of its own, as File › Options › Mail › Open new messages in a new
/// window opens one, shown: its message's id.
fn new_window(ctx: &egui::Context, app: &mut MailApp) -> u64 {
    app.ui.replies_in_window = true;
    let compose = app.run("mail.new", json!({"to": "maya@lumen.example"})).unwrap()["compose"].as_u64().unwrap();
    app.ui.replies_in_window = false;
    assert!(app.windows.of_compose(compose).is_some(), "message {compose} has no window");
    app.set_monitors(vec![crate::window_places::monitor([0, 0], [1920, 1080], 1.0, false)]);
    settle(ctx, app);
    compose
}

/// A folder of its own for the files of `test`, with a picture and a text file in it.
fn files(test: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("mailcraft-drop-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut png = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    png.resize(64, 7);
    let (picture, text) = (dir.join("Screenshot.PNG"), dir.join("notes.txt"));
    std::fs::write(&picture, png).unwrap();
    std::fs::write(&text, "a few notes").unwrap();
    (dir, picture, text)
}

/// The pictures in the text and the attachments of the message being written `compose`.
fn taken(app: &MailApp, compose: u64) -> (usize, usize) {
    let c = app.session.composing_by_id(compose).unwrap();
    (c.pictures.len(), c.attachments.len())
}

#[test]
fn a_picture_let_go_over_a_new_emails_window_goes_into_its_text() {
    let (ctx, drops) = backend();
    let mut app = app(&ctx);
    let (dir, picture, _) = files("window");
    let compose = new_window(&ctx, &mut app);
    // Another message, in the reading pane, is current meanwhile: the window the file was let
    // go over takes it.
    let other = app.run("mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    assert_ne!(app.session.compose_id(), Some(compose));
    drops.borrow_mut().push(picture);
    frame(&ctx, &mut app, vec![]);
    assert!(drops.borrow().is_empty(), "the window was shown");
    assert_eq!(taken(&app, compose), (1, 0), "{:?}", app.status_msg);
    assert!(app.session.composing_by_id(compose).unwrap().body.plain().contains(OBJECT));
    assert_eq!(taken(&app, other), (0, 0), "the other message is left as it was");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_picture_in_plain_text_and_any_other_file_are_attached() {
    let (ctx, drops) = backend();
    let mut app = app(&ctx);
    let (dir, picture, text) = files("attached");
    let compose = new_window(&ctx, &mut app);
    drops.borrow_mut().push(text);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(taken(&app, compose), (0, 1), "{:?}", app.status_msg);
    app.run("compose.formatPlain", json!({"compose": compose})).unwrap();
    drops.borrow_mut().push(picture);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(taken(&app, compose), (0, 2), "{:?}", app.status_msg);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_picture_let_go_over_the_main_window_goes_into_the_reading_panes_message() {
    let (ctx, _) = backend();
    let mut app = app(&ctx);
    let (dir, picture, _) = files("pane");
    let compose = app.run("mail.new", json!({"to": "maya@lumen.example"})).unwrap()["compose"].as_u64().unwrap();
    assert!(app.windows.of_compose(compose).is_none(), "in the reading pane");
    settle(&ctx, &mut app);
    frame(&ctx, &mut app, vec![picture.clone()]);
    assert_eq!(taken(&app, compose), (1, 0), "{:?}", app.status_msg);
    // Handed over by the system (Open With), not let go over the text: attached, as before.
    app.open_from_system(&picture.to_string_lossy());
    assert_eq!(taken(&app, compose), (1, 1), "{:?}", app.status_msg);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_picture_dropped_on_the_page_goes_into_the_text() {
    let (ctx, _) = backend();
    let mut app = app(&ctx);
    let inbox = crate::Inbox::default();
    app.services.inbox = Some(inbox.clone());
    let compose = app.run("mail.new", json!({})).unwrap()["compose"].as_u64().unwrap();
    let mut png = b"\x89PNG\r\n\x1a\nIHDR....".to_vec();
    png.resize(64, 7);
    inbox.lock().unwrap().push(crate::Delivered::dropped("photo.png".into(), png));
    crate::files::drain_inbox(&mut app);
    assert_eq!(taken(&app, compose), (1, 0), "{:?}", app.status_msg);
}
