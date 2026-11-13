// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account kept in step through Microsoft Graph (R3.1), the whole way: the window's
//! Send/Receive, this host's Graph job and `mailcraft-net`, against the strict stand-in. Nothing
//! here leaves this machine, and nobody is signed in.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, Tokens};
use mailcraft_model::{FolderKind, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::{GraphRequest, MailApp, Services};
use serde_json::json;

const NOW: i64 = 1_791_993_600;

/// Take in the Graph job's pieces and its answer, as the window does frame by frame.
fn finish(app: &mut MailApp) {
    for _ in 0..2_000 {
        app.poll_graph();
        if !app.graph_running() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the Graph job didn't end");
}

#[test]
fn a_send_receive_keeps_the_folders_in_step_through_graph_and_never_imap() {
    let server = StandIn::start().unwrap();
    {
        let mut state = server.state();
        let inbox = state.well_known("inbox");
        state.add_folder("Sales/Marketing", Some(&inbox));
    }
    let _route = server.route();
    let session = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    // The host's own Graph job, with the test in the token endpoint's place.
    let renewed: Arc<Mutex<usize>> = Arc::default();
    let route = server.url();
    let counted = renewed.clone();
    app.services.graph = Some(Box::new(move |request: GraphRequest| {
        let (tx, rx) = std::sync::mpsc::channel();
        let counted = counted.clone();
        let route = route.clone();
        // On a thread of its own, as the host runs it: the mail comes a piece at a time.
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&route);
            let result = super::graph_with(request, "UTC", &move |_, _, _| {
                *counted.lock().unwrap() += 1;
                let tokens =
                    Tokens { access: TOKEN.into(), refresh: "ref-2".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
                Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: false })
            });
            let _ = tx.send(result);
        });
        rx
    }));
    // IMAP is there to be asked, and never is.
    let imap: Arc<Mutex<Vec<String>>> = Arc::default();
    let asked = imap.clone();
    app.services.sync = Some(Box::new(move |request: SyncRequest| {
        asked.lock().unwrap().push(request.account.email.clone());
        std::sync::mpsc::channel().1
    }));
    let account = app.execute("account.add", json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.seed_graph_token(
        account,
        Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() },
    );

    app.execute("ui.sync", json!({"account": account})).unwrap();
    // A Graph run can be stopped like any Send/Receive (R3.2).
    assert!(app.syncing());
    app.execute("ui.cancelSync", json!({})).unwrap();
    finish(&mut app);
    assert!(!app.syncing());
    app.execute("ui.sync", json!({"account": account})).unwrap();
    finish(&mut app);
    let mb = &app.session.mailbox;
    let inbox = mb.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let sales = mb.folders.iter().find(|f| f.name == "Sales/Marketing").unwrap();
    assert_eq!((sales.account, sales.parent), (account, Some(inbox)));
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);

    // A folder made here is there after the next Send/Receive.
    app.execute("folder.new", json!({"name": "A\\B", "parent": sales.id})).unwrap();
    app.execute("ui.sync", json!({"account": account})).unwrap();
    finish(&mut app);
    {
        let state = server.state();
        let there = state.folder_named("A\\B").unwrap();
        assert_eq!(there.parent, state.folder_named("Sales/Marketing").unwrap().id);
    }
    // Each job had its token renewed for it (with its mail: `refresh_graph_mail`), and nothing
    // went to IMAP.
    assert_eq!(*renewed.lock().unwrap(), 3);
    assert!(imap.lock().unwrap().is_empty());
    assert!(
        server
            .state()
            .requests
            .iter()
            .all(|asked| asked.header("prefer").contains("IdType=\"ImmutableId\"") || !asked.target.contains("mailFolders"))
    );
}

/// After the switch, Microsoft wanting the owner's permission for the mail, or refusing the
/// sign-in, ends this host's Graph job with nothing but that said; the window asks the owner to
/// sign in for the mail, and never leaves "Sending and receiving…" behind (#124).
#[test]
fn a_token_refused_for_the_mail_has_the_owner_asked_to_sign_in() {
    use mailcraft_net::NetError;
    use mailcraft_ui_egui::SignInRequest;
    let answers: [fn() -> NetError; 2] = [
        || NetError::ConsentNeeded("AADSTS65001: consent required".into()),
        || NetError::Proto(mailcraft_proto::ProtoError::Auth("invalid_grant".into())),
    ];
    for answer in answers {
        let session = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
        let mut app = MailApp::new(session, Services::default());
        app.autosave = false;
        app.session.driver.set_auto_sync(false);
        app.services.graph = Some(Box::new(move |request: GraphRequest| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(super::graph_with(request, "UTC", &move |_, _, _| Err(answer())));
            });
            rx
        }));
        app.services.sync = Some(Box::new(|_| std::sync::mpsc::channel().1));
        let asked: Arc<Mutex<Vec<SignInRequest>>> = Arc::default();
        let kept = asked.clone();
        let browser = Mutex::new(Vec::new());
        app.services.sign_in = Some(Box::new(move |request: SignInRequest| {
            kept.lock().unwrap().push(request);
            let (tx, rx) = std::sync::mpsc::channel();
            browser.lock().unwrap().push(tx);
            rx
        }));
        let account = app.execute("account.add", json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        app.seed_graph_token(
            account,
            Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() },
        );
        app.execute("ui.sync", json!({"account": account})).unwrap();
        finish(&mut app);
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1, "{}", answer());
        assert!(asked[0].graph && asked[0].mail_write && asked[0].account == account);
        assert!(app.signing_in());
        assert_ne!(app.session.sync_status, "Sending and receiving…");
    }
}
