// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Send/Receive whose token stops being taken part-way (#306: it ran out during a long run, or
//! while the computer slept), the whole way: the window, this host's Graph job and
//! `mailcraft-net`, against the strict stand-in. The token is renewed once, from the newest
//! refresh token, the walk goes on to its end, and the renewed sign-in is the one kept. Nothing
//! here leaves this machine, and nobody is signed in.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{FolderKind, Mailbox};
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::{GraphRequest, MailApp, Services};
use serde_json::json;

const NOW: i64 = 1_791_993_600;

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
fn a_token_refused_part_way_through_a_send_receive_is_renewed_once_and_kept() {
    let server = StandIn::start().unwrap();
    let inbox_there = server.state().well_known("inbox");
    let projects = server.state().add_folder("Projects", None);
    server.state().mailbox.generate(&inbox_there, 120, NOW);
    server.state().mailbox.generate(&projects, 3, NOW);
    let session = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut app = MailApp::new(session, Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    // Every refresh token a new access token was asked for with, in order; each answer comes
    // with the next refresh token, as Microsoft rotates them.
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let (route, said) = (server.url(), asked.clone());
    app.services.graph = Some(Box::new(move |request: GraphRequest| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (route, said) = (route.clone(), said.clone());
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&route);
            let result = super::graph_with(request, "UTC", &move |_, refresh, _| {
                let mut said = said.lock().unwrap();
                said.push(refresh.to_string());
                let refresh = format!("ref-{}", said.len() + 1);
                let tokens = Tokens { access: TOKEN.into(), refresh, expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
                Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: false })
            });
            let _ = tx.send(result);
        });
        rx
    }));
    app.services.sync = Some(Box::new(|_| std::sync::mpsc::channel().1));
    let account = app.execute("account.add", json!({"email": "avery@mail.example", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.seed_graph_token(account, Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: NOW, ..Default::default() });
    // Part-way through the walk, Graph stops taking the token for one request of the Inbox.
    server.state().fail_next.push((format!("GET /v1.0/me/mailFolders/{inbox_there}/messages/delta"), 401, "InvalidAuthenticationToken".into()));

    app.execute("ui.sync", json!({"account": account})).unwrap();
    finish(&mut app);
    // Renewed for the job, then once part-way with the refresh token the first renewal brought.
    assert_eq!(*asked.lock().unwrap(), ["ref", "ref-2"]);
    assert_eq!(server.state().requests.iter().filter(|r| r.status == 401).count(), 1);
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    assert_eq!(app.session.mailbox.messages.values().filter(|m| m.folder == inbox).count(), 120, "the walk went on to its end");
    assert!(app.session.sync_status.starts_with("Up to date"), "{}", app.session.sync_status);

    // The renewed sign-in is the one kept: the next run's renewal starts from its refresh token.
    app.execute("ui.sync", json!({"account": account})).unwrap();
    finish(&mut app);
    assert_eq!(asked.lock().unwrap().get(2).map(String::as_str), Some("ref-3"));
}
