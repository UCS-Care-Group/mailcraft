// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Start when you sign in: off unless turned on, a command like the other Options switches, and
//! changed only when the host has changed the system's list.

use std::cell::RefCell;
use std::rc::Rc;

use crate::{MailApp, Services, UiState};
use serde_json::json;

/// A host whose system list is a flag, refusing when `refuse` is set; every call is noted.
fn host(calls: Rc<RefCell<Vec<bool>>>, refuse: Rc<RefCell<bool>>) -> Services {
    Services {
        start_at_login: Some(Box::new(move |on| {
            calls.borrow_mut().push(on);
            if *refuse.borrow() { Err("the list couldn't be written".into()) } else { Ok(()) }
        })),
        ..Services::default()
    }
}

fn app_with(services: Services) -> MailApp {
    let mut app = MailApp::new(mailcraft_engine::Session::new(mailcraft_model::Mailbox::default()), services);
    app.session.driver.set_auto_sync(false);
    app.autosave = false;
    app
}

#[test]
fn the_choice_round_trips_and_old_or_damaged_settings_leave_it_off() {
    let old: UiState = serde_json::from_value(json!({"theme": "dark"})).unwrap();
    assert!(!old.start_at_login);
    let saved = serde_json::to_value(UiState { start_at_login: true, ..old }).unwrap();
    assert_eq!(saved["startAtLogin"], true);
    assert!(serde_json::from_value::<UiState>(saved).unwrap().start_at_login);
    let damaged: UiState = serde_json::from_value(json!({"startAtLogin": "junk", "theme": "dark"})).unwrap();
    assert!(!damaged.start_at_login);
    assert_eq!(damaged.theme, crate::theme::Appearance::Dark);
}

#[test]
fn the_command_asks_the_host_and_keeps_what_the_host_did() {
    let (calls, refuse) = (Rc::new(RefCell::new(Vec::new())), Rc::new(RefCell::new(false)));
    let mut app = app_with(host(calls.clone(), refuse.clone()));
    assert!(!app.ui.start_at_login);
    assert_eq!(app.execute("ui.startAtLogin", json!({"value": true})).unwrap()["value"], true);
    assert!(app.ui.start_at_login);
    for junk in [json!(null), json!(1), json!("false"), json!([]), json!({})] {
        assert!(app.execute("ui.startAtLogin", json!({"value": junk})).is_err());
        assert!(app.ui.start_at_login);
    }
    // The host couldn't change the list: the switch stays as the list is.
    *refuse.borrow_mut() = true;
    assert!(app.execute("ui.startAtLogin", json!({})).is_err());
    assert!(app.ui.start_at_login);
    *refuse.borrow_mut() = false;
    assert_eq!(app.execute("ui.startAtLogin", json!({})).unwrap()["value"], false);
    assert!(!app.ui.start_at_login);
    // Junk never reached the host.
    assert_eq!(*calls.borrow(), [true, false, false]);
}

#[test]
fn without_a_host_nothing_changes() {
    let mut app = app_with(Services::default());
    assert!(app.execute("ui.startAtLogin", json!({"value": true})).is_err());
    assert!(!app.ui.start_at_login);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_checkbox_is_in_general_only_where_the_host_offers_it_in_light_and_dark() {
    use egui_kittest::kittest::Queryable as _;
    let build = |app: MailApp| {
        egui_kittest::Harness::builder()
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
            )
    };
    for theme in ["light", "dark"] {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut app = app_with(host(calls.clone(), Rc::new(RefCell::new(false))));
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        app.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
        let mut harness = build(app);
        harness.run_steps(4);
        harness.get_by_label("Start MailCraft when you sign in").click();
        harness.run_steps(4);
        assert!(harness.state().ui.start_at_login);
        if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
            harness.render().unwrap().save(std::path::Path::new(&dir).join(format!("start-at-login-options-{theme}.png"))).unwrap();
        }
        harness.get_by_label("Start MailCraft when you sign in").click();
        harness.run_steps(4);
        assert!(!harness.state().ui.start_at_login);
        assert_eq!(*calls.borrow(), [true, false]);
    }
    let mut app = app_with(Services::default());
    app.execute("ui.backstage", json!({"value": true, "page": "options"})).unwrap();
    let mut harness = build(app);
    harness.run_steps(4);
    assert!(harness.query_by_label("Start MailCraft when you sign in").is_none());
}
