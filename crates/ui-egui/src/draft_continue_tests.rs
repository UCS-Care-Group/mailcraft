// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #651 in the window: a draft shown in the reading pane says it hasn't been sent and has a
//! Continue editing button; Enter on a selected draft opens it in the composer; other mail has
//! neither.

use egui::Key;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The sample mailbox, in Drafts (`drafts`) or the Inbox, with the first message selected.
fn window(drafts: bool) -> Harness<'static, MailApp> {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    app.run("folder.select", json!({"kind": if drafts { "drafts" } else { "inbox" }})).unwrap();
    app.run("mail.first", json!({})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 700.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn enter(h: &mut Harness<'static, MailApp>) {
    let m = egui::Modifiers::NONE;
    h.event(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: m });
    h.step();
    h.event(egui::Event::Key { key: Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: m });
    h.run_steps(3);
}

#[test]
fn a_draft_in_the_reading_pane_has_the_bar_and_its_button_opens_it() {
    let mut h = window(true);
    assert!(h.state().hit.rect("draftBar", 0).is_some(), "the bar is drawn over a draft");
    assert!(h.state().session.compose.is_none());
    h.get_by_label("Continue editing").click();
    h.run_steps(4);
    assert!(h.state().session.compose.is_some(), "Continue editing opens the draft in the composer");
}

#[test]
fn enter_on_a_selected_draft_opens_it() {
    let mut h = window(true);
    assert!(h.state().session.compose.is_none());
    enter(&mut h);
    assert!(h.state().session.compose.is_some());
}

#[test]
fn received_mail_has_no_bar_and_enter_does_not_compose() {
    let mut h = window(false);
    assert!(h.state().hit.rect("draftBar", 0).is_none());
    enter(&mut h);
    assert!(h.state().session.compose.is_none());
}
