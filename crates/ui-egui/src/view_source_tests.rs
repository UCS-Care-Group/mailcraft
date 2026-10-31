// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! View Source and Save All Attachments in the window.

use std::sync::mpsc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::files::FileDialogRequest;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

fn first(app: &mut MailApp) -> u64 {
    app.session.rows()[0].id
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

#[test]
fn view_source_opens_its_window_for_the_owner_only() {
    let mut a = app();
    let id = first(&mut a);
    // A script or an agent gets the text, and no window.
    let r = a.execute("mail.viewSource", json!({"id": id})).unwrap();
    assert!(r["source"].as_str().unwrap().contains("Subject: "));
    assert!(a.dialog.is_none());
    let mut h = window(a);
    h.state_mut().run("mail.viewSource", json!({"id": id})).unwrap();
    h.run_steps(3);
    let Some(Dialog::ViewSource(source)) = h.state().dialog.as_ref() else { panic!("View Source isn't open") };
    assert!(source.text.contains("From: "));
    assert!(h.query_by_label("Copy").is_some());
    h.query_all_by_label("Close").last().unwrap().click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
}

#[test]
fn save_all_asks_for_a_folder() {
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = asked.clone();
    let mut a = app();
    a.services.file_dialog = Some(Box::new(move |req: FileDialogRequest| {
        seen.lock().unwrap().push(req);
        let (_tx, rx) = mpsc::channel();
        rx
    }));
    let with_files = a.session.mailbox.messages.values().find(|m| m.listed_attachments().next().is_some()).unwrap().id;
    // (The dialog here is closed at once, as if cancelled.)
    a.run("mail.saveAllAttachments", json!({"id": with_files})).unwrap();
    assert_eq!(*asked.lock().unwrap(), [FileDialogRequest::Folder]);
}
