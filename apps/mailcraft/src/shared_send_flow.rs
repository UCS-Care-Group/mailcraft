//! Sending from a shared mailbox (#70), the whole way: the window's Send/Receive, this host's
//! Graph job and `mailcraft-net`, against the strict stand-in. The first send finds that sending
//! from shared mailboxes isn't allowed yet: nothing is sent, the window asks the owner, and once
//! allowed it goes through the shared mailbox's own address and never the owner's. Nothing here
//! leaves this machine, and nobody is signed in; every address is invented.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{FolderKind, Mailbox};
use mailcraft_net::NetError;
use mailcraft_net::graph::standin::{StandIn, TOKEN};
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::{GraphRequest, MailApp, Services};
use serde_json::json;

const NOW: i64 = 1_791_993_600;
const OWNER: &str = "noor@mail.example";
const SHARED: &str = "frontdesk@mail.example";

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

fn renewed() -> Result<GraphTokens, NetError> {
    let tokens = Tokens { access: TOKEN.into(), refresh: "ref-2".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: false })
}

#[test]
fn the_first_send_from_a_shared_mailbox_waits_for_its_permission_and_then_goes_through_its_own_address() {
    let server = StandIn::start().unwrap();
    server.state().add_other(SHARED, true);
    let session = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut app = MailApp::new(session, Services::default());
    (app.autosave, app.auto_sync) = (false, false);
    app.session.shared_mailboxes = true;
    // Whether the owner has allowed sending from shared mailboxes yet, as the provider says when
    // a token for it is asked for; and how often one was.
    let allowed: Arc<Mutex<(bool, usize)>> = Arc::default();
    let (route, said) = (server.url(), allowed.clone());
    app.services.graph = Some(Box::new(move |request: GraphRequest| {
        let (tx, rx) = std::sync::mpsc::channel();
        let (route, said) = (route.clone(), said.clone());
        std::thread::spawn(move || {
            let _route = mailcraft_net::graph::standin::route(&route);
            let send = move |_: &mailcraft_model::OAuth, _: &str| {
                let mut said = said.lock().unwrap();
                said.1 += 1;
                if said.0 { renewed() } else { Err(NetError::ConsentNeeded("consent_required".into())) }
            };
            let _ = tx.send(super::graph_sending_with(request, "UTC", &|_, _, _| renewed(), &send));
        });
        rx
    }));
    app.services.sync = Some(Box::new(|_| std::sync::mpsc::channel().1));
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.seed_graph_token(
        owner,
        Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() },
    );
    let shared = app.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    finish(&mut app);
    assert_eq!(allowed.lock().unwrap().1, 0, "nothing to send: sending isn't asked about");

    // Written in it, from it.
    app.session.view.folder = app.session.mailbox.folder_of_kind(shared, FolderKind::Inbox).unwrap();
    app.execute("mail.new", json!({"to": "kit@example.net", "subject": "Rota", "body": "Hello."})).unwrap();
    let id = app.execute("compose.send", json!({})).unwrap()["id"].as_u64().unwrap();
    server.state().requests.clear();
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    finish(&mut app);
    // Not allowed yet: nothing sent, it waits, and the window asks the owner.
    assert_eq!(allowed.lock().unwrap().1, 1);
    assert!(server.state().sent.is_empty());
    assert!(!server.state().requests.iter().any(|r| r.target.ends_with("/sendMail")));
    assert!(server.state().requests.iter().all(|r| !r.path.join("/").starts_with("v1.0/me")), "nothing of it at /me");
    assert!(app.session.mailbox.message(id).is_some_and(|m| m.held.is_empty() && m.account == shared));
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("confirm"));

    // Allowed: it goes through the shared mailbox's own address.
    allowed.lock().unwrap().0 = true;
    app.execute("ui.closeDialog", json!({})).unwrap();
    app.execute("ui.sync", json!({"account": shared})).unwrap();
    finish(&mut app);
    let sent: Vec<(String, String)> = server.state().sent.iter().map(|s| (s.subject.clone(), s.mailbox.clone())).collect();
    assert_eq!(sent, [("Rota".to_string(), format!("users/{SHARED}"))]);
    assert!(app.session.mailbox.message(id).is_none(), "gone from its Outbox");
    assert!(server.state().mailbox.messages.is_empty(), "the owner's mailbox has nothing of it");
}
