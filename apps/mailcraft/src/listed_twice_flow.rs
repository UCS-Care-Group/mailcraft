// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox's calendar that the server also lists among the owner's own (#70: a sharing
//! invitation accepted in Outlook), the whole way: the window's calendar timer, this host's
//! Microsoft Graph job and `mailcraft-net`, against the stand-in (`mailcraft_net::graph::standin`).
//! The owner is offered once to hide that copy; a hidden copy is neither read nor shown, and comes
//! back when shown again. Nothing here leaves this machine and nobody is signed in.

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, Mailbox};
use mailcraft_net::graph::standin::{Routed, StandIn, TOKEN};
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::{GraphRequest, MailApp, Services};
use serde_json::json;

/// Wednesday 2026-10-14, 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SECOND: f64 = 1_000.0;
const OWNER: &str = "avery@mail.example";
const OPS: &str = "operations@example.org";

struct Flow {
    app: MailApp,
    server: StandIn,
    account: Id,
    /// The copy's id on the stand-in.
    copy: String,
    at: f64,
    _route: Routed,
}

fn granted(_: &mailcraft_model::OAuth, _: &str) -> Result<GraphTokens, mailcraft_net::NetError> {
    let tokens = Tokens { access: TOKEN.into(), refresh: "ref-2".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false })
}

impl Flow {
    /// The owner, with the shared mailbox `operations@` open under the account, and the server
    /// listing that mailbox's calendar among the owner's own as well, with an event in it. The
    /// account's first job has been taken in.
    fn start() -> Flow {
        let server = StandIn::start().unwrap();
        let copy = {
            let mut state = server.state();
            state.owner = OWNER.into();
            state.add_other(OPS, true);
            state.add_other_calendar(OPS, "Calendar", true);
            let copy = state.add_shared_by("Example Operations", "Operations@example.org", "Example Operations", true);
            state.add_event(&copy, "Ops stand-up", NOW + 3_600);
            copy
        };
        let route = server.route();
        let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
        app.autosave = false;
        app.services.graph = Some(Box::new(move |request: GraphRequest| {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(super::graph_in_with(request, "UTC", &granted));
            rx
        }));
        let account = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
        app.execute("account.addSharedMailbox", json!({"account": account, "address": OPS})).unwrap();
        let fresh = Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
        app.seed_graph_token(account, fresh);
        app.execute("view.calendar", json!({})).unwrap();
        let mut flow = Flow { app, server, account, copy, at: mailcraft_ui_egui::now_ms(), _route: route };
        flow.job();
        flow
    }

    /// A job starts when one is due, and its answer is taken in.
    fn job(&mut self) {
        // (Twenty minutes on: the account's list of calendars is due again.)
        self.at += 20.0 * 60.0 * SECOND;
        assert!(self.app.calendar_tick_at(true, self.at), "a calendar job was started");
        self.app.poll_calendar();
    }

    fn copy_here(&self) -> bool {
        self.app.session.mailbox.calendars.iter().any(|c| c.origin.account == self.account && c.origin.id == self.copy)
    }

    fn copy_events(&self) -> Vec<String> {
        self.app.session.mailbox.events.values().filter(|e| e.title == "Ops stand-up").map(|e| e.title.clone()).collect()
    }

    /// Whether anything of the copy was asked of the stand-in since `requests` was last cleared.
    fn copy_asked(&self) -> bool {
        self.server.state().requests.iter().any(|r| r.path.contains(&self.copy))
    }
}

#[test]
fn a_shared_mailboxs_calendar_listed_twice_is_offered_once_and_a_hidden_copy_is_never_read() {
    let mut flow = Flow::start();
    // Listed, with its event, and the owner is asked once whether to hide it.
    assert!(flow.copy_here());
    assert_eq!(flow.copy_events(), ["Ops stand-up"]);
    let Some(mailcraft_ui_egui::dialogs::Dialog::Confirm { command, params, .. }) = &flow.app.dialog else { panic!("offered") };
    assert_eq!((command.as_str(), params["address"].as_str()), ("calendar.hideListedCopy", Some(OPS)));

    // Hidden: gone with its event, and the next job asks nothing about it.
    flow.app.dialog = None;
    flow.app.execute("calendar.hideListedCopy", json!({"address": OPS, "hide": true})).unwrap();
    assert!(!flow.copy_here() && flow.copy_events().is_empty());
    flow.server.state().requests.clear();
    flow.job();
    assert!(!flow.copy_asked(), "a hidden copy is not read from the server");
    assert!(!flow.copy_here() && flow.copy_events().is_empty());
    assert!(flow.app.dialog.is_none(), "not asked again");

    // Shown again: back at once, with its event, and nothing is asked.
    flow.app.execute("calendar.hideListedCopy", json!({"address": OPS, "hide": false})).unwrap();
    flow.job();
    assert!(flow.copy_here() && flow.copy_asked());
    assert_eq!(flow.copy_events(), ["Ops stand-up"]);
    assert!(flow.app.dialog.is_none());
}
