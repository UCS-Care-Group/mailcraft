// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The whole way of the Favorites of the folder pane, from Outlook's list on the server to the
//! rows under the Favorites heading: the window's timer, this host's Microsoft Graph job,
//! `mailcraft-net` and the engine, against a stand-in that takes only what the documented
//! service takes (`mailcraft_net::graph::standin`). Nothing here leaves this machine, nobody is
//! signed in, and every name and id is invented.
//!
//! The host's job runs on the test's own thread (the stand-in is spoken to in Graph's place on
//! that thread only). Where the host would ask Microsoft for a token, the test answers in its
//! place: with one that is good for the Favorites, or with one that isn't, as an account has it
//! whose owner was never asked for that.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{FolderKind, Id, Mailbox};
use mailcraft_net::graph::standin::{CONFIGURATION_CANARY, Configuration, Favorite, Routed, StandIn, State, TOKEN};
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::{GraphRequest, MailApp, Services, SignInRequest};
use serde_json::{Value, json};

/// Wednesday 2026-10-14, 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SECOND: f64 = 1_000.0;
const MINUTE: f64 = 60.0 * SECOND;

/// The folders the mailbox has here, by what they are in the tests.
#[derive(Clone, Copy)]
struct Here {
    inbox: Id,
    sent: Id,
    projects: Id,
    launch: Id,
    receipts: Id,
    team: Id,
}

/// The same folders on the server, by what the server calls them.
struct There {
    inbox: String,
    sent: String,
    projects: String,
    team: String,
}

struct Flow {
    app: MailApp,
    server: StandIn,
    account: Id,
    here: Here,
    there: There,
    /// Whether the owner has agreed to the Favorites' permissions, as the token endpoint knows.
    consented: Arc<AtomicBool>,
    /// The tokens the host asked for: whether each was to be good for the Favorites.
    renewals: Arc<Mutex<Vec<bool>>>,
    /// The sign-ins the window asked for: whether for Graph, and for the Favorites.
    sign_ins: Arc<Mutex<Vec<(bool, bool)>>>,
    at: f64,
    _route: Routed,
}

impl Flow {
    /// An account that signs in with Microsoft, signed in, with the same folders here as on the
    /// server (as a Send/Receive leaves them), two favourites of the owner's own making, and
    /// Outlook's list on the server: Receipts, the Inbox, Launch (three levels down), Sent
    /// Items and Team Lumen, among entries for folders that are long gone.
    fn start() -> Flow {
        let server = StandIn::start().unwrap();
        let there = {
            let mut state = server.state();
            state.owner = "avery@mail.example".into();
            let (inbox, sent) = (state.well_known("inbox"), state.well_known("sentitems"));
            let projects = state.add_folder("Projects", Some(&inbox));
            let apollo = state.add_folder("Apollo", Some(&projects));
            let launch = state.add_folder("Launch", Some(&apollo));
            let receipts = state.add_folder("Receipts", None);
            let team = state.add_folder("Team Lumen", None);
            state.add_favorite(&receipts, 0);
            state.add_favorite(&inbox, 2);
            state.add_favorite(&launch, 4);
            state.add_favorite(&sent, 7);
            state.add_favorite(&team, 9);
            let listed = state.favorites.get_or_insert_with(Vec::new);
            for n in 0..14i64 {
                let gone = format!("AAMkAGFub3RoZXItbWFpbGJveC1leGFtcGxlLm9yZwAuAAAAAAB7Q+x/Lm9SnT4qB3L0d9EBAFh2c/Kp+0aTmWvN4uYx1RoAAAIB{n:04}AAAA=");
                listed.push(Favorite { name: "Inbox".into(), index: Some(n), kind: "Folder".into(), folder_id: gone });
            }
            There { inbox, sent, projects, team }
        };
        let route = server.route();
        let consented = Arc::new(AtomicBool::new(true));
        let renewals: Arc<Mutex<Vec<bool>>> = Default::default();
        let sign_ins: Arc<Mutex<Vec<(bool, bool)>>> = Default::default();
        let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
        app.autosave = false;
        let (agreed, asked) = (consented.clone(), renewals.clone());
        app.services.graph = Some(Box::new(move |request: GraphRequest| {
            let (tx, rx) = std::sync::mpsc::channel();
            // The host's own job, with the test in the token endpoint's place.
            let result = super::graph_with(request, "UTC", NOW, &|_, _, favorites| {
                asked.lock().unwrap().push(favorites);
                let tokens =
                    Tokens { access: TOKEN.into(), refresh: "ref-2".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
                Ok(GraphTokens { tokens, extras_missing: false, covers_favorites: favorites && agreed.load(Ordering::Relaxed) })
            });
            let _ = tx.send(result);
            rx
        }));
        let (agreed, asked) = (consented.clone(), sign_ins.clone());
        app.services.sign_in = Some(Box::new(move |request: SignInRequest| {
            asked.lock().unwrap().push((request.graph, request.favorites));
            // The owner agrees on the provider's page, which the test stands in for.
            agreed.store(true, Ordering::Relaxed);
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(Ok(Tokens {
                access: TOKEN.into(),
                refresh: "ref-3".into(),
                expires_at: mailcraft_time::now() + 3_600,
                ..Default::default()
            }));
            rx
        }));
        let added = app.execute(
            "account.add",
            json!({"email": "avery@mail.example", "auth": "oauth2", "provider": "microsoft", "incoming": {"host": "imap.mail.example"}}),
        );
        let account = added.unwrap()["id"].as_u64().unwrap();
        let fresh = Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
        app.seed_graph_token(account, fresh);
        let kind = |app: &MailApp, kind: FolderKind| app.session.mailbox.folder_of_kind(account, kind).unwrap();
        let (inbox, sent) = (kind(&app, FolderKind::Inbox), kind(&app, FolderKind::Sent));
        let mut folder = |name: &str, parent: Option<Id>| {
            app.execute("folder.new", json!({"name": name, "parent": parent, "account": account})).unwrap()["id"].as_u64().unwrap()
        };
        let projects = folder("Projects", Some(inbox));
        let apollo = folder("Apollo", Some(projects));
        let launch = folder("Launch", Some(apollo));
        let receipts = folder("Receipts", None);
        let team = folder("Team Lumen", None);
        // The owner's own favourites of that account: its Inbox (as every account's is) and Projects.
        app.execute("folder.favorite", json!({"id": projects, "value": true})).unwrap();
        let here = Here { inbox, sent, projects, launch, receipts, team };
        Flow { app, server, account, here, there, consented, renewals, sign_ins, at: mailcraft_ui_egui::now_ms(), _route: route }
    }

    /// Look at the account's server `ms` on: a job starts when one is due, and its answer is
    /// taken in. Whether one did.
    fn look(&mut self, ms: f64) -> bool {
        self.at += ms;
        let started = self.app.calendar_tick_at(true, self.at);
        for _ in 0..4 {
            self.app.poll_calendar();
        }
        started
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

    /// How often the server was asked for the configuration item.
    fn reads(&self) -> usize {
        self.server.state().requests.iter().filter(|asked| asked.target.contains("userConfigurations")).count()
    }

    /// What the server answered with anything but a yes, so far: the statuses.
    fn refusals(&self) -> Vec<u16> {
        self.server.state().requests.iter().map(|asked| asked.status).filter(|status| *status >= 400).collect()
    }

    fn marked(&self) -> Vec<Id> {
        self.app.session.mailbox.folders.iter().filter(|f| f.favorite).map(|f| f.id).collect()
    }
}

#[test]
fn outlooks_favorites_arrive_in_outlooks_order_with_the_stale_ones_left_out() {
    let mut flow = Flow::start();
    let here = flow.here;
    let own = flow.marked();
    assert_eq!(flow.listed(), [here.inbox, here.projects], "the owner's own, to begin with");
    // The first look at the account's server: Outlook's five, in Outlook's order, and none of
    // the fourteen entries for folders that are gone.
    assert!(flow.look(0.0));
    assert_eq!(flow.listed(), [here.receipts, here.inbox, here.launch, here.sent, here.team]);
    let state = flow.state();
    assert_eq!((state["from"].clone(), state["line"].clone()), (json!("server"), json!(null)));
    // The owner's own marks are kept aside as they were.
    assert_eq!(flow.marked(), own);
    // The token at hand was used; nothing was written; and the only answers that were not a
    // yes are the fourteen "no such folder".
    assert!(flow.renewals.lock().unwrap().is_empty());
    assert_eq!(flow.refusals(), [404; 14]);
    {
        let state = flow.server.state();
        assert!(state.writes().is_empty());
        let first = state.requests.iter().find(|asked| asked.target.contains("userConfigurations")).unwrap();
        assert_eq!(
            (first.method.as_str(), first.target.as_str()),
            ("GET", "/beta/me/mailFolders/root/userConfigurations/Aggregated.OwaUserConfiguration")
        );
    }
    // Nothing of the rest of the configuration, and nothing the server calls a folder by, is in
    // the mailbox or in what the window answers.
    let file = String::from_utf8(mailcraft_engine::io::write_mailbox(&flow.app.session.mailbox).unwrap()).unwrap();
    let shown = format!("{file} {state}");
    assert!(!shown.contains(CONFIGURATION_CANARY) && !shown.contains("AQMk") && !shown.contains("AAMk"));
    assert!(file.contains(r#""favoritesFromServer""#));
    // The list comes from Outlook and is changed there.
    let refused = flow.app.execute("folder.favorite", json!({"id": here.receipts, "value": false})).unwrap_err();
    assert!(refused.contains("Outlook"), "{refused}");

    // Not more often than every quarter of an hour, however often the calendar is looked at.
    for _ in 0..20 {
        flow.look(31.0 * SECOND);
    }
    assert_eq!(flow.reads(), 1);
    flow.look(5.0 * MINUTE);
    assert_eq!(flow.reads(), 2);
    assert_eq!(flow.listed(), [here.receipts, here.inbox, here.launch, here.sent, here.team]);
    assert!(flow.app.dialog.is_none() && !flow.app.has_sync_problems());
}

#[test]
fn a_favourite_removed_in_outlook_goes_at_the_next_look_and_a_renamed_inbox_is_still_the_inbox() {
    let mut flow = Flow::start();
    let here = flow.here;
    flow.look(0.0);
    assert_eq!(flow.listed().len(), 5);
    {
        // In Outlook: Team Lumen is taken off, Projects is put first, and the Inbox is called
        // something else (as it is in a mailbox in another language).
        let mut state = flow.server.state();
        let team = State::configuration_id(&flow.there.team);
        state.favorites.as_mut().unwrap().retain(|favorite| favorite.folder_id != team);
        let projects = flow.there.projects.clone();
        state.add_favorite(&projects, -1);
        let inbox = flow.there.inbox.clone();
        state.folders.iter_mut().find(|folder| folder.id == inbox).unwrap().name = "Posteingang".into();
    }
    // Not before the next look that reads them; then it is so here.
    flow.look(31.0 * SECOND);
    assert_eq!(flow.listed().len(), 5);
    flow.look(15.0 * MINUTE);
    assert_eq!(flow.listed(), [here.projects, here.receipts, here.inbox, here.launch, here.sent]);
    // A folder the mailbox here doesn't have (yet) is left out, and comes once it is here.
    let archive = {
        let mut state = flow.server.state();
        let archive = state.add_folder("Archive 2025", None);
        state.add_favorite(&archive, 1);
        archive
    };
    flow.look(15.0 * MINUTE);
    assert_eq!(flow.listed().len(), 5);
    let made =
        flow.app.execute("folder.new", json!({"name": "archive 2025", "parent": null, "account": flow.account})).unwrap()["id"].as_u64().unwrap();
    flow.look(15.0 * MINUTE);
    assert_eq!(flow.listed(), [here.projects, here.receipts, made, here.inbox, here.launch, here.sent]);
    assert!(!archive.is_empty() && flow.server.state().writes().is_empty());
}

#[test]
fn a_sign_in_without_the_permissions_gets_the_offer_and_nothing_else_changes() {
    let mut flow = Flow::start();
    let here = flow.here;
    let own = flow.listed();
    let marks = flow.marked();
    // The owner signed in before the Favorites were ever asked for: the server refuses the read.
    flow.server.state().configuration = Configuration::Denied;
    flow.consented.store(false, Ordering::Relaxed);
    let main = flow.server.state().main_calendar();
    flow.server.state().add_event(&main, "Budget review", NOW + 3_600);
    assert!(flow.look(0.0));
    let state = flow.state();
    assert_eq!(
        (state["from"].clone(), state["line"].clone(), state["text"].clone()),
        (json!("mailbox"), json!("offer"), json!("Show Outlook's Favorites"))
    );
    // Nothing else is any different: the owner's favourites, the calendar, no dialog, no problem.
    assert_eq!((flow.listed(), flow.marked()), (own.clone(), marks.clone()));
    assert!(flow.app.session.mailbox.events.values().any(|event| event.title == "Budget review"));
    assert!(flow.app.dialog.is_none() && !flow.app.has_sync_problems() && flow.app.status_msg.is_none());
    assert!(flow.app.session.mailbox.favorites_from_server.is_empty());
    flow.app.execute("folder.favorite", json!({"id": here.receipts, "value": true})).unwrap();
    flow.app.execute("folder.favorite", json!({"id": here.receipts, "value": false})).unwrap();
    assert_eq!(flow.refusals(), [403]);

    // Not again that day, however often the server is looked at.
    for _ in 0..40 {
        flow.look(31.0 * SECOND);
    }
    flow.look(6.0 * 60.0 * MINUTE);
    assert_eq!((flow.reads(), flow.renewals.lock().unwrap().len()), (1, 0));
    // The next day, once: a token is asked for that would be good for them, it isn't, and the
    // server is not asked what it would only refuse.
    flow.look(19.0 * 60.0 * MINUTE);
    assert_eq!((flow.reads(), flow.renewals.lock().unwrap().clone()), (1, vec![true]));
    flow.look(31.0 * SECOND);
    assert_eq!((flow.reads(), flow.renewals.lock().unwrap().len()), (1, 1));
    assert_eq!(flow.state()["line"], "offer");

    // The owner takes the offer and agrees on the provider's page: the list is there at the
    // next look, and the marks made here are kept aside as they were.
    flow.server.state().configuration = Configuration::There;
    assert_eq!(flow.app.execute("ui.connectFavorites", json!({"account": flow.account})).unwrap()["pending"], "signIn");
    flow.app.poll_sign_in();
    assert_eq!(*flow.sign_ins.lock().unwrap(), [(true, true)]);
    assert!(flow.look(SECOND));
    assert_eq!(flow.listed(), [here.receipts, here.inbox, here.launch, here.sent, here.team]);
    assert_eq!((flow.state()["line"].clone(), flow.marked()), (json!(null), marks));
    assert_eq!(flow.reads(), 2);
}

#[test]
fn a_mailbox_without_the_item_keeps_the_favourites_made_here() {
    let mut flow = Flow::start();
    let here = flow.here;
    let own = flow.listed();
    // Nobody has opened Outlook on the web for this mailbox: it has no such item.
    flow.server.state().configuration = Configuration::Missing;
    assert!(flow.look(0.0));
    let state = flow.state();
    assert_eq!((state["from"].clone(), state["line"].clone()), (json!("mailbox"), json!(null)));
    assert_eq!((flow.listed(), flow.refusals()), (own.clone(), vec![404]));
    // They work as they always did, and nothing new is in the mailbox file.
    flow.app.execute("folder.favorite", json!({"id": here.team, "value": true})).unwrap();
    assert_eq!(flow.listed(), [here.inbox, here.projects, here.team]);
    let file = String::from_utf8(mailcraft_engine::io::write_mailbox(&flow.app.session.mailbox).unwrap()).unwrap();
    assert!(!file.contains("favoritesFromServer"));
    // The item has no list in it: the same.
    flow.server.state().configuration = Configuration::There;
    flow.server.state().favorites = None;
    flow.look(16.0 * MINUTE);
    assert_eq!((flow.state()["from"].clone(), flow.listed().len()), (json!("mailbox"), 3));
    // Outlook's list appears (the owner made a favourite there): it is the one shown from then
    // on, and the three made here are kept; it goes again, and they are back exactly.
    flow.server.state().add_favorite(&flow.there.sent.clone(), 0);
    flow.look(16.0 * MINUTE);
    assert_eq!(flow.listed(), [here.sent]);
    flow.server.state().configuration = Configuration::Missing;
    flow.look(16.0 * MINUTE);
    assert_eq!(flow.listed(), [here.inbox, here.projects, here.team]);
    assert!(flow.app.dialog.is_none() && !flow.app.has_sync_problems());
}

#[test]
fn switching_outlooks_list_off_restores_the_favourites_made_here_exactly() {
    let mut flow = Flow::start();
    let here = flow.here;
    let own = flow.listed();
    let marks = flow.marked();
    flow.look(0.0);
    assert_ne!(flow.listed(), own);
    // File › Options: the ones kept in MailCraft instead. They are back exactly, and can be changed.
    let shown = flow.app.execute("view.favorites", json!({"own": true})).unwrap();
    assert_eq!(shown["own"], true);
    assert_eq!((flow.listed(), flow.marked()), (own.clone(), marks.clone()));
    flow.app.execute("folder.favorite", json!({"id": here.launch, "value": true})).unwrap();
    assert_eq!(flow.listed(), [here.inbox, here.projects, here.launch]);
    // The server is not asked for Outlook's list while it isn't wanted.
    let reads = flow.reads();
    for _ in 0..3 {
        flow.look(16.0 * MINUTE);
    }
    assert_eq!(flow.reads(), reads);
    // Switched on again: read at the next look, and the owner's own are kept aside once more.
    flow.app.execute("view.favorites", json!({"own": false})).unwrap();
    assert!(flow.look(SECOND));
    assert_eq!(flow.reads(), reads + 1);
    assert_eq!(flow.listed(), [here.receipts, here.inbox, here.launch, here.sent, here.team]);
    assert_eq!(flow.marked(), [here.inbox, here.projects, here.launch]);
}

#[test]
fn a_read_that_fails_is_a_quiet_line_and_trying_again_puts_it_right() {
    let mut flow = Flow::start();
    let here = flow.here;
    flow.look(0.0);
    let listed = flow.listed();
    // The document on the server is cut short the next time it is read.
    let whole = flow.server.state().configuration_xml();
    flow.server.state().configuration_document = Some(whole[..whole.len() / 2].to_string());
    flow.look(16.0 * MINUTE);
    let state = flow.state();
    assert_eq!(
        (state["line"].clone(), state["text"].clone(), state["from"].clone()),
        (json!("failed"), json!("Couldn't read the list"), json!("server"))
    );
    // What was listed stays; no dialog, nothing in the problems report, nothing in the status bar.
    assert_eq!(flow.listed(), listed);
    assert!(flow.app.dialog.is_none() && !flow.app.has_sync_problems() && flow.app.status_msg.is_none());
    // "Try again": asked at the next look, and the line goes when the read goes through.
    flow.server.state().configuration_document = None;
    let reads = flow.reads();
    assert_eq!(flow.app.execute("ui.favorites", json!({"account": flow.account, "refresh": true})).unwrap()["line"], json!(null));
    assert!(flow.look(SECOND));
    assert_eq!((flow.reads(), flow.state()["line"].clone(), flow.listed()), (reads + 1, json!(null), listed.clone()));

    // Two questions about folders are told to come back later: they are asked once more after
    // as long as the server says, and the list is whole and in order.
    flow.server.state().throttle_folders = 2;
    flow.look(16.0 * MINUTE);
    assert_eq!(flow.listed(), [here.receipts, here.inbox, here.launch, here.sent, here.team]);
    assert_eq!(flow.state()["line"], json!(null));
    assert_eq!(flow.refusals().iter().filter(|status| **status == 429).count(), 2);
    // The sign-in's token is no longer taken: nothing is said of the Favorites for it (the
    // calendar's own job notices, and asks for another).
    flow.server.state().token = "another".into();
    flow.look(16.0 * MINUTE);
    assert_eq!((flow.state()["line"].clone(), flow.listed().len()), (json!(null), 5));
}
