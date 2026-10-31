// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Insert › Item and Insert › Contact Card in the window: the picker opens, and only its choice
//! is attached.

use egui_kittest::Harness;
use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::pick_item::{PickKind, rows};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn writing() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.run("mail.new", json!({})).unwrap();
    app
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
fn the_buttons_open_the_picker_and_attach_nothing_by_themselves() {
    for (command, name) in [("compose.attachItem", "attachItem"), ("insert.businessCard", "businessCard")] {
        let mut h = window(writing());
        h.state_mut().run(command, json!({})).unwrap_or_default();
        h.run_steps(3);
        assert_eq!(h.state().dialog.as_ref().map(Dialog::name), Some(name), "{command}");
        assert!(h.state().session.compose.as_ref().unwrap().attachments.is_empty());
    }
}

#[test]
fn the_picker_finds_by_what_is_typed() {
    let app = writing();
    let all = rows(&app, PickKind::Message, "");
    assert!(!all.is_empty());
    let some = rows(&app, PickKind::Message, "lisbon");
    assert!(!some.is_empty() && some.len() < all.len());
    assert!(rows(&app, PickKind::Message, "nothing-has-this-zzz").is_empty());
    assert!(!rows(&app, PickKind::Contact, "").is_empty());
}
