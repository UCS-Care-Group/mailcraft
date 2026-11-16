// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The tray preference is opt-in, persistent and a command like the other Options switches.

use crate::{MailApp, Services, UiState};
use serde_json::json;

#[test]
fn the_choice_round_trips_and_old_settings_keep_closing_normally() {
    let old: UiState = serde_json::from_value(json!({"theme": "dark"})).unwrap();
    assert!(!old.close_to_tray);
    assert!(!old.close_to_tray_notice_seen);
    let saved = serde_json::to_value(UiState { close_to_tray: true, ..old }).unwrap();
    assert_eq!(saved["closeToTray"], true);
    assert!(serde_json::from_value::<UiState>(saved).unwrap().close_to_tray);
    let damaged: UiState = serde_json::from_value(json!({"closeToTray": "junk", "theme": "dark"})).unwrap();
    assert!(!damaged.close_to_tray);
    assert_eq!(damaged.theme, crate::theme::Appearance::Dark);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_first_close_explains_where_to_find_mailcraft_and_can_be_cancelled() {
    use egui_kittest::kittest::Queryable as _;
    for theme in ["light", "dark"] {
        let mut app = MailApp::new(mailcraft_engine::Session::new(mailcraft_model::Mailbox::default()), Services::default());
        app.session.driver.set_auto_sync(false);
        app.autosave = false;
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        app.execute("ui.closeToTray", json!({"value": true})).unwrap();
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    app.logic(ui.ctx());
                    app.ui(ui);
                },
                app,
            );
        let ctx = harness.ctx.clone();
        assert!(!crate::close_to_tray::confirm_hide(&ctx, harness.state()));
        harness.run_steps(4);
        harness.get_by_label("MailCraft will keep running");
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            harness.render().unwrap().save(std::path::Path::new(&dir).join(format!("tray-first-close-{theme}.png"))).unwrap();
        }
        harness.get_by_label("Cancel").click();
        harness.run_steps(4);
        assert!(!harness.state().ui.close_to_tray_notice_seen);
        assert!(!crate::close_to_tray::confirm_hide(&ctx, harness.state()));
        harness.run_steps(4);
        harness.get_by_label("Continue in tray").click();
        harness.run_steps(4);
        assert!(harness.state().ui.close_to_tray_notice_seen);
        assert!(crate::close_to_tray::confirm_hide(&ctx, harness.state()));
        let saved = serde_json::to_value(&harness.state().ui).unwrap();
        assert!(serde_json::from_value::<UiState>(saved).unwrap().close_to_tray_notice_seen);
    }
}

#[test]
fn the_command_sets_and_toggles_but_refuses_invalid_values() {
    let mut app = MailApp::new(mailcraft_engine::Session::new(mailcraft_model::Mailbox::default()), Services::default());
    assert!(!app.ui.close_to_tray);
    assert_eq!(app.execute("ui.closeToTray", json!({"value": true})).unwrap()["value"], true);
    for junk in [json!(null), json!(1), json!("false"), json!([]), json!({})] {
        assert!(app.execute("ui.closeToTray", json!({"value": junk})).is_err());
        assert!(app.ui.close_to_tray);
    }
    assert_eq!(app.execute("ui.closeToTray", json!({})).unwrap()["value"], false);
    assert!(!app.ui.close_to_tray);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_checkbox_works_in_general_in_light_and_dark() {
    use egui_kittest::kittest::Queryable as _;
    for theme in ["light", "dark"] {
        let mut app = MailApp::new(mailcraft_engine::Session::new(mailcraft_model::Mailbox::default()), Services::default());
        app.session.driver.set_auto_sync(false);
        app.autosave = false;
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        app.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        let mut harness = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1280.0, 800.0))
            .with_step_dt(1.0 / 60.0)
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    app.logic(ui.ctx());
                    app.ui(ui);
                },
                app,
            );
        harness.run_steps(4);
        assert!(!harness.state().ui.close_to_tray);
        harness.get_by_label("Close to system tray").click();
        harness.run_steps(4);
        assert!(harness.state().ui.close_to_tray);
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            harness.render().unwrap().save(std::path::Path::new(&dir).join(format!("tray-options-{theme}.png"))).unwrap();
        }
        harness.get_by_label("Close to system tray").click();
        harness.run_steps(4);
        assert!(!harness.state().ui.close_to_tray);
    }
}
