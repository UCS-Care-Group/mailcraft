// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc::channel,
};

use egui_kittest::{Harness, kittest::Queryable as _};
use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::{Services, dialogs::Dialog, forms::Form};

const NOW: i64 = 1_791_993_600;

fn account(provider: Provider, email: &str) -> Account {
    Account { email: email.into(), name: "Example Owner".into(), provider_name: "Example provider".into(), provider }
}

fn app(services: Services) -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), services);
    app.autosave = false;
    app
}

#[test]
fn discovery_is_explicit_and_normal_add_account_does_not_query_gnome() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let mut app = app(Services {
        gnome_accounts: Some(Box::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = channel();
            tx.send(Ok(Vec::new())).unwrap();
            rx
        })),
        ..Default::default()
    });
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    app.execute("ui.dialog", json!({"name": "account"})).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let before = app.session.mailbox.accounts.len();
    app.execute("ui.dialog", json!({"name": "gnomeAccounts"})).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    app.execute("ui.closeDialog", json!({})).unwrap();
    assert_eq!(app.session.mailbox.accounts.len(), before);
}

#[test]
fn no_service_and_a_disconnected_worker_leave_existing_accounts_untouched() {
    let mut app = app(Services::default());
    assert!(app.execute("ui.dialog", json!({"name": "gnomeAccounts"})).is_err());
    app.services.gnome_accounts = Some(Box::new(|| channel().1));
    let before = app.session.mailbox.accounts.len();
    let mut form = Import::open(&app).unwrap();
    form.poll();
    assert_eq!(form.error, Some(DiscoveryError::Failed));
    assert_eq!(app.session.mailbox.accounts.len(), before);
}

#[test]
fn only_a_supported_selected_account_can_be_set_up_and_duplicates_are_refused() {
    let mut app = app(Services::default());
    app.execute("account.add", json!({"email": "existing@example.org"})).unwrap();
    let mut form = Import {
        rx: None,
        accounts: vec![
            account(Provider::Google, "owner@example.org"),
            account(Provider::Unsupported, "other@example.org"),
            account(Provider::Microsoft365, "EXISTING@example.org"),
        ],
        error: None,
        selected: None,
    };
    let before = app.session.mailbox.accounts.len();
    assert!(form.setup(&app).is_none());
    for selected in [1, 2, 999] {
        form.selected = Some(selected);
        assert!(form.setup(&app).is_none());
    }
    form.selected = Some(0);
    assert!(form.setup(&app).is_some());
    assert_eq!(app.session.mailbox.accounts.len(), before);
}

#[test]
fn selecting_an_account_prefills_setup_without_contacting_a_provider() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let services = Services {
        gnome_accounts: Some(Box::new(|| {
            let (tx, rx) = channel();
            tx.send(Ok(vec![account(Provider::Microsoft365, "owner@example.org"), account(Provider::Google, "second@example.org")])).unwrap();
            rx
        })),
        microsoft_client_id: "mailcraft-test-client".into(),
        sign_in: Some(Box::new(move |request| {
            assert_eq!(request.email, "owner@example.org");
            assert_eq!(request.oauth.provider, mailcraft_model::OAuthProvider::Microsoft);
            assert_eq!(request.oauth.client_id, "mailcraft-test-client");
            assert!(request.refresh.is_empty());
            count.fetch_add(1, Ordering::Relaxed);
            channel().1
        })),
        ..Default::default()
    };
    let mut app = app(services);
    let before = app.session.mailbox.accounts.len();
    app.execute("ui.dialog", json!({"name": "gnomeAccounts"})).unwrap();
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1000.0, 760.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(3);
    harness.get_by_label("owner@example.org — Example provider").click();
    harness.run_steps(2);
    harness.get_by_label("Continue").click();
    harness.run_steps(3);
    assert!(matches!(harness.state().dialog, Some(Dialog::Form(Form::Account(_)))));
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert_eq!(harness.state().session.mailbox.accounts.len(), before);
    harness.get_by_label("Connect").click();
    harness.run_steps(3);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(harness.state().session.mailbox.accounts.len(), before);
}

#[test]
fn changing_mailbox_discards_the_pending_discovery() {
    let (tx, rx) = channel();
    let app = &mut app(Services::default());
    app.dialog = Some(Dialog::Form(Form::GnomeAccounts(Box::new(Import { rx: Some(rx), accounts: Vec::new(), error: None, selected: None }))));
    app.clear_jobs_on_replace();
    assert!(app.dialog.is_none());
    assert!(tx.send(Ok(vec![account(Provider::Google, "old@example.org")])).is_err());
}

/// Optional headless visual check, using only invented accounts.
#[test]
#[ignore = "writes screenshots for visual inspection"]
fn import_dialog_light_and_dark() {
    let Ok(dir) = std::env::var("MAILCRAFT_GOA_SHOTS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    for theme in ["light", "dark"] {
        let mut app = app(Services {
            gnome_accounts: Some(Box::new(|| {
                let (tx, rx) = channel();
                tx.send(Ok(vec![
                    account(Provider::Google, "personal@example.org"),
                    account(Provider::Microsoft365, "work@example.org"),
                    account(Provider::Unsupported, "other@example.org"),
                ]))
                .unwrap();
                rx
            })),
            ..Default::default()
        });
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        app.execute("ui.dialog", json!({"name": "gnomeAccounts"})).unwrap();
        let mut harness =
            Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).wgpu().build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        harness.input_mut().max_texture_side = Some(8192);
        harness.run_steps(6);
        harness.render().unwrap().save(std::path::Path::new(&dir).join(format!("gnome-import-{theme}.png"))).unwrap();
    }
}
