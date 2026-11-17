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
    Account { email: email.into(), name: "Example Owner".into(), provider_name: "Example provider".into(), provider, linked: None }
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
        link_error: None,
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
fn supported_generic_mail_accounts_can_also_be_set_up_separately() {
    let app = app(Services::default());
    let mut generic = linkable();
    generic.provider = Provider::Unsupported;
    let mut form = Import { rx: None, accounts: vec![generic], error: None, selected: Some(0), link_error: None };
    assert!(form.setup(&app).is_some());
    form.accounts[0].linked = None;
    assert!(form.setup(&app).is_none());
}

#[test]
fn a_failed_goa_graph_run_clears_the_sync_status_and_directs_repair_to_gnome() {
    for consent in [false, true] {
        let mut app = app(Services {
            gnome_accounts: Some(Box::new(|| channel().1)),
            sync: Some(Box::new(|_| panic!("Graph mail must not use IMAP"))),
            sign_in: Some(Box::new(|_| panic!("GOA repair must not open MailCraft sign-in"))),
            graph: Some(Box::new(move |request| {
                let (tx, rx) = channel();
                tx.send(crate::GraphResult { account: request.account.id, sign_in_refused: !consent, consent_needed: consent, ..Default::default() })
                    .unwrap();
                rx
            })),
            ..Default::default()
        });
        app.session.prefs.sync_minutes = 0;
        let form = Import { rx: None, accounts: vec![graph_linkable()], error: None, selected: Some(0), link_error: None };
        let id = form.link(&mut app).unwrap();
        app.execute("ui.sync", json!({"account": id})).unwrap();
        app.poll_graph();
        assert!(app.session.sync_status.is_empty());
        assert!(!app.syncing());
        assert!(app.status_msg.as_ref().is_some_and(|(text, _)| text.contains("GNOME Online Accounts")));
    }
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
    harness.get_by_label("Set up separately").click();
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
    app.dialog = Some(Dialog::Form(Form::GnomeAccounts(Box::new(Import {
        rx: Some(rx),
        accounts: Vec::new(),
        error: None,
        selected: None,
        link_error: None,
    }))));
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
                tx.send(Ok(vec![linkable(), graph_linkable(), account(Provider::Unsupported, "other@example.org")])).unwrap();
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
        harness.get_by_label("work@example.org — Microsoft 365").click();
        harness.run_steps(3);
        harness.render().unwrap().save(std::path::Path::new(&dir).join(format!("gnome-import-{theme}-selected.png"))).unwrap();
    }
}

fn linkable() -> Account {
    let mut account = account(Provider::Google, "linked@example.org");
    account.linked = Some(mailcraft_model::Account {
        email: account.email.clone(),
        display_name: account.name.clone(),
        kind: mailcraft_model::AccountKind::Imap,
        incoming: Some(mailcraft_model::Server { host: "imap.gmail.com".into(), port: 993, security: mailcraft_model::Security::Tls }),
        outgoing: Some(mailcraft_model::Server { host: "smtp.gmail.com".into(), port: 587, security: mailcraft_model::Security::StartTls }),
        goa: Some(mailcraft_model::GoaAccount { id: "account_1".into(), provider: "google".into() }),
        ..Default::default()
    });
    account
}

#[test]
fn explicit_link_persists_reference_and_syncs_without_mailcraft_credentials() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut app = app(Services {
        sign_in: Some(Box::new(|_| panic!("GOA must never start a MailCraft login"))),
        gnome_accounts: Some(Box::new(|| channel().1)),
        sync: Some(Box::new(move |request| {
            assert!(request.account.goa.is_some());
            assert!(request.account.oauth.is_none());
            assert!(request.password.is_empty() && request.access_token.is_empty());
            observed.fetch_add(1, Ordering::Relaxed);
            let (_tx, rx) = std::sync::mpsc::sync_channel(1);
            rx
        })),
        ..Default::default()
    });
    let form = Import { rx: None, accounts: vec![linkable()], error: None, selected: Some(0), link_error: None };
    form.link(&mut app).unwrap();
    let account = app.session.mailbox.accounts.iter().find(|a| a.goa.is_some()).unwrap().clone();
    let roundtrip: mailcraft_model::Account = serde_json::from_str(&serde_json::to_string(&account).unwrap()).unwrap();
    assert_eq!(account, roundtrip);
    assert_eq!(
        app.execute("account.list", json!({})).unwrap()["accounts"].as_array().unwrap().iter().find(|a| a["id"] == account.id).unwrap()["auth"],
        "goa"
    );
    let original = crate::unconfirmed::fingerprint(&app.session.mailbox);
    let mut changed = app.session.mailbox.clone();
    changed.accounts.iter_mut().find(|a| a.id == account.id).unwrap().goa.as_mut().unwrap().id = "different_account".into();
    assert_ne!(original, crate::unconfirmed::fingerprint(&changed), "a different GOA reference needs confirmation when opening a mailbox file");
    assert!(form.link(&mut app).is_err(), "no duplicate links");
    for field in [json!({"incoming": {"host": "evil.example"}}), json!({"auth": "oauth2"}), json!({"goa": {"id": "other", "provider": "google"}})] {
        let mut params = field;
        params["id"] = json!(account.id);
        assert!(app.execute("account.edit", params).is_err());
    }
    app.execute("ui.sync", json!({"account": account.id})).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(app.passwords.is_empty() && app.tokens.is_empty());
    assert!(app.can_sync_quietly(account.id), "GOA-linked accounts participate in scheduled Send/Receive");
}

#[test]
fn link_button_adds_only_the_selected_account_and_starts_its_first_sync() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut app = app(Services {
        gnome_accounts: Some(Box::new(|| {
            let (tx, rx) = channel();
            tx.send(Ok(vec![linkable(), account(Provider::Microsoft365, "other@example.org")])).unwrap();
            rx
        })),
        sign_in: Some(Box::new(|_| panic!("linking must not open MailCraft sign-in"))),
        sync: Some(Box::new(move |request| {
            assert_eq!(request.account.email, "linked@example.org");
            assert!(request.account.goa.is_some());
            observed.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = channel();
            tx.send(mailcraft_model::sync::SyncResult { account: request.account.id, email: request.account.email, ..Default::default() }).unwrap();
            rx
        })),
        ..Default::default()
    });
    // Isolate the button's requested run from the sample's independently due automatic round.
    app.session.prefs.sync_minutes = 0;
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
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    harness.get_by_label("linked@example.org — Example provider").click();
    harness.run_steps(2);
    harness.get_by_label("Link with GNOME").click();
    harness.run_steps(4);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(harness.state().session.mailbox.accounts.len(), before + 1);
    assert_eq!(harness.state().session.mailbox.accounts.iter().filter(|a| a.goa.is_some()).count(), 1);
    assert!(harness.state().dialog.is_none());
}

fn graph_linkable() -> Account {
    Account {
        email: "work@example.org".into(),
        name: "Example Owner".into(),
        provider_name: "Microsoft 365".into(),
        provider: Provider::Microsoft365,
        linked: Some(mailcraft_model::Account {
            email: "work@example.org".into(),
            kind: mailcraft_model::AccountKind::Microsoft,
            goa: Some(mailcraft_model::GoaAccount { id: "graph_1".into(), provider: "ms_graph".into() }),
            ..Default::default()
        }),
    }
}

#[test]
fn graph_link_uses_the_graph_host_without_local_sign_in_or_cached_tokens() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let mut app = app(Services {
        gnome_accounts: Some(Box::new(|| channel().1)),
        sync: Some(Box::new(|_| panic!("Graph-linked mail must never use IMAP"))),
        sign_in: Some(Box::new(|_| panic!("GOA must never fall back to MailCraft sign-in"))),
        graph: Some(Box::new(move |request| {
            assert_eq!(request.account.kind, mailcraft_model::AccountKind::Microsoft);
            assert!(request.account.goa.is_some() && request.account.oauth.is_none());
            assert!(request.oauth.client_id.is_empty() && request.refresh.is_empty() && request.access.is_empty());
            assert!(request.mail.is_some() && !request.directory && !request.settings && !request.favorites);
            observed.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = channel();
            // A faulty host returning tokens must not put GOA credentials in UI caches either.
            tx.send(crate::GraphResult {
                account: request.account.id,
                tokens: Some(mailcraft_model::sync::Tokens {
                    access: "must-not-cache".into(),
                    refresh: "must-not-store".into(),
                    ..Default::default()
                }),
                mail: Some(mailcraft_model::sync::GraphMailResult {
                    account: request.account.id,
                    email: request.account.email,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap();
            rx
        })),
        ..Default::default()
    });
    app.session.prefs.sync_minutes = 0;
    let form = Import { rx: None, accounts: vec![graph_linkable()], error: None, selected: Some(0), link_error: None };
    let id = form.link(&mut app).unwrap();
    app.execute("ui.sync", json!({"account": id})).unwrap();
    app.poll_graph();
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(app.graph_tokens.is_empty() && app.tokens.is_empty() && app.passwords.is_empty());
    assert!(app.session.mailbox.account(id).unwrap().oauth.is_none());
    assert!(app.can_sync_quietly(id));
    assert!(app.execute("account.edit", json!({"id": id, "auth": "oauth2"})).is_err());
}

#[test]
fn ordinary_graph_keeps_its_own_login_without_goa_and_beside_a_goa_account() {
    use mailcraft_model::sync::Tokens;

    for with_goa in [false, true] {
        let runs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = runs.clone();
        let mut app = app(Services {
            gnome_accounts: with_goa.then(|| {
                Box::new(|| -> std::sync::mpsc::Receiver<Discovery> { panic!("ordinary Graph and explicit links must not trigger discovery") })
                    as Discover
            }),
            microsoft_client_id: "independent-mailcraft-client".into(),
            sync: Some(Box::new(|_| panic!("both Microsoft accounts must use Graph"))),
            sign_in: Some(Box::new(|_| panic!("the independent account already has its refresh token"))),
            graph: Some(Box::new(move |request| {
                let managed = request.account.goa.is_some();
                observed.lock().unwrap().push(managed);
                if managed {
                    assert!(request.account.oauth.is_none());
                    assert!(request.oauth.client_id.is_empty() && request.refresh.is_empty());
                } else {
                    assert!(request.account.oauth.is_some());
                    assert_eq!(request.oauth.client_id, "independent-mailcraft-client");
                    assert_eq!(request.refresh, "independent-refresh");
                }
                let (tx, rx) = channel();
                tx.send(crate::GraphResult {
                    account: request.account.id,
                    tokens: Some(Tokens {
                        access: if managed { "external-access" } else { "independent-access" }.into(),
                        refresh: if managed { "external-refresh" } else { "independent-refresh" }.into(),
                        expires_at: mailcraft_time::now() + 3600,
                        ..Default::default()
                    }),
                    mail: Some(mailcraft_model::sync::GraphMailResult {
                        account: request.account.id,
                        email: request.account.email,
                        ..Default::default()
                    }),
                    ..Default::default()
                })
                .unwrap();
                rx
            })),
            ..Default::default()
        });
        app.session.prefs.sync_minutes = 0;
        let ordinary = app.execute("account.add", json!({"email": "independent@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        app.tokens.insert(ordinary, Tokens { refresh: "independent-refresh".into(), ..Default::default() });
        app.execute("ui.sync", json!({"account": ordinary})).unwrap();
        app.poll_graph();
        assert_eq!(app.graph_tokens[&ordinary].access, "independent-access");

        if with_goa {
            let form = Import { rx: None, accounts: vec![graph_linkable()], error: None, selected: Some(0), link_error: None };
            let linked = form.link(&mut app).unwrap();
            app.execute("ui.sync", json!({"account": linked})).unwrap();
            app.poll_graph();
            assert!(!app.graph_tokens.contains_key(&linked) && !app.tokens.contains_key(&linked));
            app.execute("ui.sync", json!({"account": ordinary})).unwrap();
            app.poll_graph();
        }
        assert_eq!(*runs.lock().unwrap(), if with_goa { vec![false, true, false] } else { vec![false] });
        assert!(app.session.mailbox.account(ordinary).unwrap().goa.is_none());
        assert_eq!(app.graph_tokens[&ordinary].refresh, "independent-refresh");
    }
}
