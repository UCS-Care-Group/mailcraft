// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host supplies Google's Desktop registration secret; preferences and commands cannot.
//! Every registration value here is synthetic and opens nothing.

use std::sync::mpsc;

use mailcraft_engine::Session;
use mailcraft_model::Mailbox;
use serde_json::json;

use crate::{MailApp, Services, UiState};

const DESKTOP_ID: &str = "synthetic-client.apps.googleusercontent.com";
const HOST_SECRET: &str = "synthetic-host-desktop-registration";
const LEGACY_SECRET: &str = "synthetic-obsolete-preference-registration";

fn app() -> MailApp {
    let services = Services { sign_in: Some(Box::new(|_| mpsc::channel().1)), ..Default::default() };
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), 1_791_993_600, 0), services);
    (app.autosave, app.auto_sync) = (false, false);
    app
}

fn legacy_preferences() -> UiState {
    serde_json::from_value(json!({
        "googleClientId": DESKTOP_ID,
        "googleClientSecret": LEGACY_SECRET,
        "rememberSignIn": false,
        "folderPaneWidth": 275.0,
        "recent": ["synthetic.mailcraft"]
    }))
    .unwrap()
}

#[test]
fn legacy_google_registration_secret_is_ignored_and_not_saved_again() {
    let mut app = app();
    app.services.google_client_secret = HOST_SECRET.into();
    app.apply_prefs(legacy_preferences());
    assert_eq!(app.google_client_secret(), HOST_SECRET);

    // Exercise the same preference snapshot and serialization used by the host's ui.json save.
    let saved = serde_json::to_string_pretty(&app.prefs()).unwrap();
    assert!(!saved.contains("googleClientSecret"));
    assert!(!saved.contains(LEGACY_SECRET));
    assert!(!saved.contains(HOST_SECRET));

    let restored: UiState = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.google_client_id, DESKTOP_ID);
    assert!(!restored.remember_sign_in);
    assert_eq!(restored.folder_pane_width, 275.0);
    assert_eq!(restored.recent, ["synthetic.mailcraft"]);
}

#[test]
fn google_registration_requires_the_host_secret_even_with_legacy_preferences() {
    let mut app = app();
    app.apply_prefs(legacy_preferences());
    assert!(app.google_client_secret().is_empty());
    assert!(!app.google_registration_ready(), "a legacy preference must not configure the registration");

    app.services.google_client_secret = HOST_SECRET.into();
    assert!(app.google_registration_ready(), "the public ID override still works with the host's registration");

    app.execute("ui.googleClientId", json!({"value": ""})).unwrap();
    assert!(!app.google_registration_ready(), "the registration also needs its client ID");
    app.services.google_client_id = DESKTOP_ID.into();
    assert!(app.google_registration_ready(), "host configuration needs no preference override");
}

#[test]
fn retired_google_secret_command_cannot_configure_or_replace_the_host_registration() {
    for host_secret in ["", HOST_SECRET] {
        let mut app = app();
        app.services.google_client_id = DESKTOP_ID.into();
        app.services.google_client_secret = host_secret.into();
        let was_ready = app.google_registration_ready();
        let before = serde_json::to_value(app.prefs()).unwrap();
        assert!(!app.enabled("ui.googleClientSecret"));

        for value in [LEGACY_SECRET, ""] {
            let error = app.execute("ui.googleClientSecret", json!({"value": value})).unwrap_err();
            assert!(!error.contains(LEGACY_SECRET));
            assert_eq!(app.google_client_secret(), host_secret);
            assert_eq!(app.google_registration_ready(), was_ready);
            assert_eq!(serde_json::to_value(app.prefs()).unwrap(), before);
        }
    }
}
