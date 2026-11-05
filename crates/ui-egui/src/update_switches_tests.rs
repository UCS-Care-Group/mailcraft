// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The two update switches sit in File › About under Check for Updates, and File › Options has
//! none (#488).

use egui_kittest::Harness;
use mailcraft_engine::Session;
use serde_json::json;

use crate::{MailApp, Services};

/// A window on a build that can check for new versions (the answer never comes).
fn window() -> Harness<'static, MailApp> {
    let services = Services {
        update_check: Some(Box::new(|_| {
            let (_tx, rx) = std::sync::mpsc::channel();
            rx
        })),
        update_download: Some(Box::new(|_| {
            let (_tx, rx) = std::sync::mpsc::channel();
            rx
        })),
        ..Default::default()
    };
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), 1_791_993_600, 0), services);
    app.autosave = false;
    let mut window = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    window.run_steps(3);
    window
}

fn show(window: &mut Harness<'static, MailApp>, page: &str) {
    window.state_mut().run("ui.backstage", json!({"value": true, "page": page})).unwrap();
    window.run_steps(4);
}

#[test]
fn about_holds_both_switches_below_check_for_updates() {
    let mut w = window();
    show(&mut w, "about");
    let check = w.state().hit.rect("checkForUpdates", 0).expect("Check for Updates");
    let option = w.state().hit.rect("updateOption", 0).expect("the check switch");
    let install = w.state().hit.rect("updateInstallOption", 0).expect("the install switch");
    assert!(option.top() >= check.bottom(), "the check switch is under the button");
    assert!(install.top() >= option.bottom(), "the install switch is under the check switch");
}

#[test]
fn options_has_no_update_switch() {
    let mut w = window();
    show(&mut w, "options");
    assert!(w.state().hit.rect("updateOption", 0).is_none());
    assert!(w.state().hit.rect("updateInstallOption", 0).is_none());
}

#[test]
fn the_switch_in_about_turns_the_daily_check_on() {
    let mut w = window();
    show(&mut w, "about");
    let at = w.state().hit.rect("updateOption", 0).expect("the check switch").center();
    crate::people_view::testing::click(&mut w, at, egui::PointerButton::Primary, 1);
    w.run_steps(3);
    assert_eq!(w.state().ui.update_check, crate::updates::UpdateCheck::On);
}
