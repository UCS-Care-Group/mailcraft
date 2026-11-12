// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Outlook's Favorites for an account of an organisation, the whole way: the window's commands,
//! this host's job for Microsoft Graph, `mailcraft-net` and the engine, against a stand-in that
//! takes only what the documented service takes (`mailcraft_net::graph::standin`). Nothing here
//! leaves this machine, nobody is signed in, and every name and id is invented.
//!
//! The host's job runs on the test's own thread (the stand-in is spoken to in the service's
//! place on that thread only). Where the host would ask Microsoft for a token or put its page
//! in front of the owner, the test answers in its place.

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Directory, FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin as graph;
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::favorites_sync::{NEEDS_APPROVAL, REFUSED_FOR_ACCOUNT};
use mailcraft_ui_egui::{GraphRequest, MailApp, Services, SignInRequest};
use serde_json::{Value, json};

/// Wednesday 2026-10-14, 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SECOND: f64 = 1_000.0;
const MINUTE: f64 = 60.0 * SECOND;
/// The directory of the organisation the account belongs to, as its sign-in names it, and the
/// application this installation is known to Microsoft by: both invented.
const ORGANISATION: &str = "3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b";
const APPLICATION: &str = "0f6c8d2a-1b3e-4c5d-9e7f-a1b2c3d4e5f6";

/// What Microsoft's page comes to when the owner is sent there, which the test stands in for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    /// The owner agrees.
    Agrees,
    /// The organisation keeps the permission for its administrators to give.
    NeedsApproval,
    /// This kind of account has no such permission.
    NoSuchThing,
}

/// What the owner has agreed to, as Microsoft's token endpoint knows, and what its page does.
struct Microsoft {
    /// Graph's two permissions for the Favorites.
    graph: bool,
    page: Page,
    /// The sign-ins the window asked for: whether for Graph, and for the Favorites.
    sign_ins: Vec<(bool, bool)>,
}

/// The folders the mailbox has here, by what they are in the tests.
#[derive(Clone, Copy)]
struct Here {
    inbox: Id,
    projects: Id,
    launch: Id,
    receipts: Id,
}

struct Flow {
    app: MailApp,
    /// Microsoft Graph, where Outlook keeps its list.
    graph: graph::StandIn,
    account: Id,
    here: Here,
    microsoft: Arc<Mutex<Microsoft>>,
    at: f64,
    _route: graph::Routed,
}

fn fresh(access: &str, tenant: &str) -> Tokens {
    Tokens { access: access.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, tenant: tenant.into(), ..Default::default() }
}

impl Flow {
    /// An account of an organisation that signs in with Microsoft, signed in, with the same
    /// folders here as on the server; two favourites of the owner's own making; and Outlook's
    /// list on the server: Receipts, the Inbox, Launch. The owner hasn't agreed to its being
    /// read yet.
    fn start() -> Flow {
        Flow::start_with(Mailbox::with_local_account("", ""), ORGANISATION)
    }

    /// [`start`](Self::start) on `mailbox` (the account is added when it has none that signs
    /// in with Microsoft), with a sign-in that names `tenant`.
    fn start_with(mailbox: Mailbox, tenant: &str) -> Flow {
        let graph_server = graph::StandIn::start().unwrap();
        {
            let mut state = graph_server.state();
            state.owner = "avery@lumen.example".into();
            let inbox = state.well_known("inbox");
            let projects = state.add_folder("Projects", Some(&inbox));
            let apollo = state.add_folder("Apollo", Some(&projects));
            let launch = state.add_folder("Launch", Some(&apollo));
            let receipts = state.add_folder("Receipts", None);
            state.add_folder("Team Lumen", None);
            state.add_favorite(&receipts, 0);
            state.add_favorite(&inbox, 2);
            state.add_favorite(&launch, 4);
            // (Nobody has agreed to reading it yet.)
            state.configuration = graph::Configuration::Denied;
        }
        let route = graph_server.route();
        let microsoft = Arc::new(Mutex::new(Microsoft { graph: false, page: Page::Agrees, sign_ins: Vec::new() }));
        let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services { microsoft_client_id: APPLICATION.into(), ..Default::default() });
        app.autosave = false;
        let (asked, directory) = (microsoft.clone(), tenant.to_string());
        app.services.graph = Some(Box::new(move |request: GraphRequest| {
            let (tx, rx) = std::sync::mpsc::channel();
            // The host's own job, with the test in the token endpoint's place.
            let result = super::graph_with(request, "UTC", NOW, &|_, _, favorites| {
                let covers = favorites && asked.lock().unwrap().graph;
                Ok(GraphTokens { tokens: fresh(graph::TOKEN, &directory), extras_missing: false, covers_favorites: covers })
            });
            let _ = tx.send(result);
            rx
        }));
        let (asked, directory) = (microsoft.clone(), tenant.to_string());
        app.services.sign_in = Some(Box::new(move |request: SignInRequest| {
            let mut microsoft = asked.lock().unwrap();
            microsoft.sign_ins.push((request.graph, request.favorites));
            let answer = match microsoft.page {
                Page::Agrees => {
                    microsoft.graph |= request.favorites;
                    Ok(fresh(graph::TOKEN, &directory))
                }
                Page::NeedsApproval => Err(NEEDS_APPROVAL.to_string()),
                Page::NoSuchThing => Err(REFUSED_FOR_ACCOUNT.to_string()),
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(answer);
            rx
        }));
        // The host's own way of making the link an administrator is sent.
        app.services.admin_consent_link = Some(Box::new(|oauth, tenant| mailcraft_net::oauth::admin_consent_url(oauth, tenant).ok()));
        let microsoft_account = |app: &MailApp| app.session.mailbox.accounts.iter().find(|a| a.oauth.is_some()).map(|a| a.id);
        let account = match microsoft_account(&app) {
            Some(account) => account,
            None => {
                let added = app.execute(
                    "account.add",
                    json!({"email": "avery@lumen.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "imap.lumen.example"}}),
                );
                let account = added.unwrap()["id"].as_u64().unwrap();
                let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
                let mut folder = |name: &str, parent: Option<Id>| {
                    app.execute("folder.new", json!({"name": name, "parent": parent, "account": account})).unwrap()["id"].as_u64().unwrap()
                };
                let projects = folder("Projects", Some(inbox));
                let apollo = folder("Apollo", Some(projects));
                folder("Launch", Some(apollo));
                folder("Receipts", None);
                folder("Team Lumen", None);
                // The owner's own favourites of that account: its Inbox (as every account's is) and Projects.
                app.execute("folder.favorite", json!({"id": projects, "value": true})).unwrap();
                account
            }
        };
        // Signed in, as a Send/Receive leaves it: the sign-in said which directory it was.
        app.seed_graph_token(account, fresh(graph::TOKEN, tenant));
        let mailbox = &app.session.mailbox;
        let named = |path: &str| mailbox.folders.iter().find(|f| f.account == account && mailbox.folder_path(f.id) == path).unwrap().id;
        let here = Here {
            inbox: named("Inbox"),
            projects: named("Inbox/Projects"),
            launch: named("Inbox/Projects/Apollo/Launch"),
            receipts: named("Receipts"),
        };
        Flow { app, graph: graph_server, account, here, microsoft, at: mailcraft_ui_egui::now_ms(), _route: route }
    }

    /// Look at the account's servers `ms` on: the jobs that are due start, their answers are
    /// taken in, and a page the owner was sent to has come back.
    fn look(&mut self, ms: f64) {
        self.at += ms;
        for _ in 0..3 {
            self.app.calendar_tick_at(true, self.at);
            self.app.poll_calendar();
            self.app.poll_sign_in();
        }
    }

    /// The owner takes the offer (`ui.connectFavorites`), and what follows from it has happened.
    fn connect(&mut self) -> Value {
        let answer = self.app.execute("ui.connectFavorites", json!({"account": self.account})).unwrap();
        self.app.poll_sign_in();
        // (The page the owner agreed on stands for the server letting the list be read.)
        if self.microsoft.lock().unwrap().graph {
            self.graph.state().configuration = graph::Configuration::There;
        }
        self.look(SECOND);
        answer
    }

    /// The folders of the account that the pane lists under Favorites, in its order.
    fn listed(&self) -> Vec<Id> {
        let all = mailcraft_engine::cmd::folder::favorites(&self.app.session);
        all.into_iter().filter(|id| self.app.session.mailbox.folder(*id).is_some_and(|f| f.account == self.account)).collect()
    }

    /// What the window says of the account's Favorites (`ui.favorites`).
    fn state(&mut self) -> Value {
        self.app.execute("ui.favorites", json!({"account": self.account})).unwrap()
    }

    fn sign_ins(&self) -> Vec<(bool, bool)> {
        self.microsoft.lock().unwrap().sign_ins.clone()
    }

    /// The server was never asked to change anything.
    fn nothing_was_written(&self) -> bool {
        self.graph.state().writes().is_empty()
    }
}

#[test]
fn an_organisations_account_is_offered_outlooks_favorites_through_graph_like_any_other() {
    let mut flow = Flow::start();
    let here = flow.here;
    assert_eq!(flow.listed(), [here.inbox, here.projects]);
    flow.look(0.0);
    // An account of an organisation: its sign-in named a directory that is not the one of
    // personal accounts. There is nothing to choose: the offer to show them stands.
    let state = flow.state();
    assert_eq!((state["directory"].clone(), state["line"].clone()), (json!("organisation"), json!("offer")));
    assert!(state.get("choice").is_none() && state.get("offered").is_none() && state.get("source").is_none(), "{state}");
    assert!(flow.app.execute("ui.favoritesSource", json!({"account": flow.account, "source": "graph"})).is_err());
    assert!(flow.app.execute("ui.dialog", json!({"name": "favoritesSource", "account": flow.account})).is_err());
    // (The sign-in said which kind of account it is; that writes nothing.)
    let looked = String::from_utf8(mailcraft_engine::io::write_mailbox(&flow.app.session.mailbox).unwrap()).unwrap();
    assert!(!looked.contains("favoritesSource") && !looked.contains("directory"));

    // The offer: Microsoft's page, for the Favorites' two permissions; then the list is Outlook's.
    assert_eq!(flow.connect()["pending"], "signIn");
    assert_eq!(flow.sign_ins(), [(true, true)]);
    assert_eq!(flow.listed(), [here.receipts, here.inbox, here.launch]);
    let state = flow.state();
    assert_eq!((state["from"].clone(), state["line"].clone()), (json!("server"), json!(null)));
    // Nothing of a choice, no directory's id and no token is in the mailbox file.
    let file = String::from_utf8(mailcraft_engine::io::write_mailbox(&flow.app.session.mailbox).unwrap()).unwrap();
    assert!(!file.contains("favoritesSource") && !file.contains(ORGANISATION) && !file.contains(graph::TOKEN));
    assert!(flow.nothing_was_written() && flow.app.dialog.is_none());
}

#[test]
fn graph_that_an_administrator_has_to_approve_says_so_with_a_link_that_names_graph_alone() {
    let mut flow = Flow::start();
    let own = flow.listed();
    flow.look(0.0);
    // The organisation keeps Graph's permission for its administrators to give.
    flow.microsoft.lock().unwrap().page = Page::NeedsApproval;
    let main = flow.graph.state().main_calendar();
    flow.graph.state().add_event(&main, "Budget review", NOW + 3_600);
    flow.connect();
    let state = flow.state();
    assert_eq!(
        (state["line"].clone(), state["text"].clone(), state["steps"].clone(), state["from"].clone()),
        (json!("approval"), json!(NEEDS_APPROVAL), json!(["copyMessage", "help"]), json!("mailbox"))
    );
    // Nothing else is any different: the owner's favourites, the calendar, no dialog, no problem.
    assert_eq!(flow.listed(), own);
    flow.look(6.0 * MINUTE);
    assert!(flow.app.session.mailbox.events.values().any(|event| event.title == "Budget review"));
    assert!(flow.app.dialog.is_none() && !flow.app.has_sync_problems());

    // The link for the administrator: Microsoft's admin-consent page, for this application and
    // everything it asks an owner for (the two permissions among it, so that one approval
    // covers the rest as well), in the organisation's own directory, and nothing of Exchange
    // Web Services. Nothing is opened or sent.
    let copied = flow.app.execute("ui.copyAdminConsentLink", json!({"account": flow.account})).unwrap();
    let link = copied["url"].as_str().unwrap();
    assert!(link.starts_with(&format!("https://login.microsoftonline.com/{ORGANISATION}/v2.0/adminconsent?client_id={APPLICATION}&")), "{link}");
    assert!(link.contains("MailboxConfigItem.Read") && link.contains("Mail.ReadBasic") && link.contains("redirect_uri=http%3A%2F%2Flocalhost"));
    assert!(link.contains("IMAP.AccessAsUser.All") && link.contains("Calendars.ReadWrite"));
    assert!(!link.contains("EWS") && !link.contains("office365") && !link.contains("avery") && !link.contains(graph::TOKEN));
    assert!(copied.get("source").is_none());
    // The window's own link copies a message for the administrator with that address in it,
    // and nothing of the owner.
    let message = flow.app.execute("ui.copyAdminMessage", json!({"account": flow.account})).unwrap();
    let text = message["text"].as_str().unwrap();
    assert!(message["url"] == link && text.lines().any(|line| line == link) && !text.contains("avery") && !text.contains('@'));

    // Not asked again that day, however often the servers are looked at.
    let asked = flow.sign_ins().len();
    for _ in 0..40 {
        flow.look(31.0 * SECOND);
    }
    flow.look(6.0 * 60.0 * MINUTE);
    assert_eq!((flow.sign_ins().len(), flow.state()["line"].clone()), (asked, json!("approval")));

    // An administrator approves; "Try again" asks for the Favorites again, and the list comes.
    flow.microsoft.lock().unwrap().page = Page::Agrees;
    flow.app.execute("ui.signInAgain", json!({"account": flow.account})).unwrap();
    flow.app.poll_sign_in();
    flow.graph.state().configuration = graph::Configuration::There;
    flow.look(SECOND);
    assert_eq!(flow.sign_ins().last(), Some(&(true, true)));
    assert_eq!((flow.state()["line"].clone(), flow.listed()), (json!(null), vec![flow.here.receipts, flow.here.inbox, flow.here.launch]));
    assert!(flow.nothing_was_written());
}

#[test]
fn a_mailbox_whose_favorites_were_read_through_exchange_web_services_opens_and_reads_them_through_graph() {
    // A mailbox written by 0.2.0 or 0.2.1 whose owner had chosen Exchange Web Services.
    let flow = Flow::start();
    let mut file: Value = serde_json::from_slice(&mailcraft_engine::io::write_mailbox(&flow.app.session.mailbox).unwrap()).unwrap();
    let at = file["accounts"].as_array().unwrap().iter().position(|a| a["email"] == "avery@lumen.example").unwrap();
    file["accounts"][at]["favoritesSource"] = json!("ews");
    file["accounts"][at]["oauth"]["directory"] = json!("organisation");
    let old = file.to_string();
    let dir = std::env::temp_dir().join(format!("mailcraft-r2-{}-{}", std::process::id(), mailcraft_ui_egui::now_ms() as u64));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.mailcraft");
    std::fs::write(&path, &old).unwrap();

    // Opened: everything is there, the owner is told once, and the file is to be saved again.
    let mut flow = Flow::start_with(Mailbox::with_local_account("", ""), ORGANISATION);
    flow.app.run("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    let told = flow.app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(told.contains("Outlook's Favorites are now read through Microsoft Graph"), "{told}");
    assert!(flow.app.session.dirty);
    let account = flow.app.session.mailbox.accounts.iter().find(|a| a.email == "avery@lumen.example").unwrap();
    assert_eq!(account.oauth.as_ref().unwrap().directory, Directory::Organisation);
    let saved = String::from_utf8(mailcraft_engine::io::write_mailbox(&flow.app.session.mailbox).unwrap()).unwrap();
    assert!(!saved.contains("favoritesSource"), "{saved}");
    // Saved and opened again: nothing more to say.
    flow.app.run("file.save", json!({"path": path.to_string_lossy()})).unwrap();
    flow.app.status_msg = None;
    flow.app.run("file.open", json!({"path": path.to_string_lossy()})).unwrap();
    assert!(flow.app.status_msg.as_ref().is_none_or(|(text, _)| !text.contains("Microsoft Graph")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_account_that_has_no_such_permission_is_said_in_a_line() {
    // Its sign-in names the directory of personal accounts, and the page says this kind of
    // account has no such permission.
    let mut flow = Flow::start_with(Mailbox::with_local_account("", ""), Directory::PERSONAL_TENANT);
    flow.look(0.0);
    assert_eq!((flow.state()["directory"].clone(), flow.state()["line"].clone()), (json!("personal"), json!("offer")));
    flow.microsoft.lock().unwrap().page = Page::NoSuchThing;
    flow.connect();
    let state = flow.state();
    assert_eq!(
        (state["line"].clone(), state["text"].clone(), state["from"].clone()),
        (json!("refused"), json!(REFUSED_FOR_ACCOUNT), json!("mailbox"))
    );
    assert_eq!(flow.sign_ins(), [(true, true)]);
    assert!(flow.nothing_was_written());
}
