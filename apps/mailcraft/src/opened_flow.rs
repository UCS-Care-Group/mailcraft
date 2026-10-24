// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Calendars opened by address (#70, Open Shared Calendar), the whole way: the command, the
//! window's calendar timer, this host's Microsoft Graph job and `mailcraft-net`, against a
//! stand-in that holds other people's calendars at `/users/{address}/calendar`
//! (`mailcraft_net::graph::standin`). Nothing here leaves this machine and nobody is signed in:
//! the token for other people's calendars is a fake's.

use mailcraft_engine::Session;
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Id, Mailbox, Rights};
use mailcraft_net::graph::standin::{Access, Routed, StandIn, TOKEN};
use mailcraft_net::oauth::GraphTokens;
use mailcraft_ui_egui::{GraphRequest, MailApp, Services};
use serde_json::json;

/// Wednesday 2026-10-14, 16:00 UTC.
const NOW: i64 = 1_791_993_600;
const SECOND: f64 = 1_000.0;
const OWNER: &str = "avery@mail.example";

struct Flow {
    app: MailApp,
    server: StandIn,
    account: Id,
    at: f64,
    _route: Routed,
}

/// How a token for other people's calendars is had: from nobody, or not at all.
type Renew = fn(&mailcraft_model::OAuth, &str) -> Result<GraphTokens, mailcraft_net::NetError>;

fn granted(_: &mailcraft_model::OAuth, _: &str) -> Result<GraphTokens, mailcraft_net::NetError> {
    let tokens = Tokens { access: TOKEN.into(), refresh: "ref-2".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
    Ok(GraphTokens { tokens, extras_missing: true, covers_favorites: false })
}

fn not_yet(_: &mailcraft_model::OAuth, _: &str) -> Result<GraphTokens, mailcraft_net::NetError> {
    Err(mailcraft_net::NetError::ConsentNeeded("the owner hasn't agreed to it yet".into()))
}

impl Flow {
    fn start(renew: Renew) -> Flow {
        let server = StandIn::start().unwrap();
        let main = server.state().main_calendar();
        server.state().owner = OWNER.into();
        server.state().add_event(&main, "Own lunch", NOW + 3_600);
        let route = server.route();
        let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
        app.autosave = false;
        app.services.graph = Some(Box::new(move |request: GraphRequest| {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(super::graph_in_with(request, "UTC", &renew));
            rx
        }));
        let added = app.execute(
            "account.add",
            json!({"email": OWNER, "auth": "oauth2", "provider": "microsoft", "tenant": "organizations", "incoming": {"host": "imap.mail.example"}}),
        );
        let account = added.unwrap()["id"].as_u64().unwrap();
        let fresh = Tokens { access: TOKEN.into(), refresh: "ref".into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() };
        app.seed_graph_token(account, fresh);
        app.execute("view.calendar", json!({})).unwrap();
        let mut flow = Flow { app, server, account, at: mailcraft_ui_egui::now_ms(), _route: route };
        flow.job();
        flow
    }

    /// A job starts when one is due, and its answer is taken in.
    fn job(&mut self) {
        // (A job that brought nothing makes the next wait; twenty minutes is longer than any wait.)
        self.at += 20.0 * 60.0 * SECOND;
        assert!(self.app.calendar_tick_at(true, self.at), "a calendar job was started");
        self.app.poll_calendar();
    }

    fn calendar(&self, name: &str) -> Option<mailcraft_model::Calendar> {
        self.app.session.mailbox.calendars.iter().find(|c| c.name == name).cloned()
    }

    fn titles_in(&self, calendar: Id) -> Vec<String> {
        let mut all: Vec<String> = self.app.session.mailbox.events.values().filter(|e| e.calendar == calendar).map(|e| e.title.clone()).collect();
        all.sort();
        all
    }

    /// The requests of the stand-in that reached the owner's own `/me`, as `METHOD /path`.
    fn on_me(&self) -> Vec<String> {
        let seen = self.server.state().requests.clone();
        seen.iter().filter(|r| r.path.get(1).is_some_and(|p| p == "me")).map(|r| format!("{} /{}", r.method, r.path.join("/"))).collect()
    }
}

#[test]
fn a_calendar_opened_by_address_is_read_and_listed_and_none_of_it_goes_through_me() {
    let mut flow = Flow::start(granted);
    flow.server.state().add_person("cy@example.org", "Cy Colleague", Access::Read);
    flow.server.state().add_person_event("cy@example.org", "Cy's dentist", NOW + 7_200);
    flow.app.execute("calendar.openShared", json!({"address": "cy@example.org"})).unwrap();
    flow.job();
    let cy = flow.calendar("Cy Colleague").expect("listed");
    assert_eq!(
        (cy.rights, cy.origin.id.as_str(), cy.owner.as_ref().map(|o| o.address.as_str())),
        (Rights::Read, "user:cy@example.org", Some("cy@example.org"))
    );
    // Its events came with it, in its own calendar; the owner's lunch is the owner's.
    assert_eq!(flow.titles_in(cy.id), ["Cy's dentist"]);
    let own = flow.app.session.mailbox.calendars.iter().find(|c| c.origin.id.is_empty() && c.origin.account == flow.account).map(|c| c.id).unwrap();
    assert_eq!(flow.titles_in(own), ["Own lunch"]);
    // It is listed under Shared Calendars, and the owner's list keeps it.
    let groups = mailcraft_engine::cmd::calendar_settings::calendar_groups(&flow.app.session);
    assert!(groups.iter().any(|g| g.title == "Shared Calendars" && g.calendars == [cy.id]));
    assert_eq!(flow.app.session.mailbox.account(flow.account).unwrap().opened_calendars[0].name, "Cy Colleague");
    // Nothing for it was asked of `/me`: not its list, not its view, not its events.
    let me = flow.on_me();
    assert!(!me.is_empty(), "the owner's own calendar was asked for");
    assert!(me.iter().all(|r| !r.contains("cy") && !r.contains("/users/")), "{me:?}");
    let seen = flow.server.state().requests.clone();
    assert!(seen.iter().any(|r| r.path.get(1).is_some_and(|p| p == "users") && r.path.last().is_some_and(|l| l == "calendarView")));
    // The next job keeps it up to date, and an event that went from there goes here too.
    flow.server.state().person("cy@example.org").unwrap();
    flow.job();
    assert_eq!(flow.titles_in(cy.id), ["Cy's dentist"]);
}

#[test]
fn a_refusal_adds_nothing_and_says_why() {
    let mut flow = Flow::start(granted);
    flow.server.state().add_person("gus@example.org", "Gus", Access::None);
    flow.app.execute("calendar.openShared", json!({"address": "gus@example.org"})).unwrap();
    flow.job();
    assert!(flow.calendar("gus@example.org").is_none() && flow.calendar("Gus").is_none());
    assert!(flow.app.session.mailbox.account(flow.account).unwrap().opened_calendars.is_empty());
    assert!(flow.on_me().iter().all(|r| !r.contains("gus")));
    // Nobody at all at the address.
    flow.app.execute("calendar.openShared", json!({"address": "nobody@example.org"})).unwrap();
    flow.job();
    assert!(flow.app.session.mailbox.account(flow.account).unwrap().opened_calendars.is_empty());
    // The owner's own calendar was not harmed by either.
    assert!(flow.app.session.mailbox.events.values().any(|e| e.title == "Own lunch"));
}

#[test]
fn a_person_who_shares_only_free_and_busy_is_shown_as_blocks_that_cannot_be_changed() {
    let mut flow = Flow::start(granted);
    flow.server.state().add_person("fay@example.org", "Fay", Access::FreeBusy);
    flow.server.state().person("fay@example.org").unwrap().busy = vec![(NOW + 3_600, NOW + 7_200, "busy".into())];
    flow.app.execute("calendar.openShared", json!({"address": "fay@example.org"})).unwrap();
    flow.job();
    let fay = flow.calendar("fay@example.org").expect("listed by its address: nothing says a name");
    assert_eq!(fay.rights, Rights::FreeBusy);
    let seen: Vec<String> = flow.server.state().requests.iter().map(|r| format!("{} /{} -> {}", r.method, r.path.join("/"), r.status)).collect();
    // (Blocks say no more than how the time shows: they have no title.)
    assert_eq!(flow.titles_in(fay.id), [""], "{seen:#?}");
    // Nothing can be made in it, or changed.
    let made = flow.app.execute("calendar.new", json!({"title": "No", "start": "2026-10-14T18:00", "calendar": fay.id}));
    assert!(made.is_err(), "{made:?}");
    assert!(flow.on_me().iter().all(|r| !r.contains("fay")));
}

#[test]
fn what_is_made_changed_and_deleted_in_a_calendar_that_can_be_edited_goes_to_its_owner_alone() {
    let mut flow = Flow::start(granted);
    flow.server.state().add_person("dee@example.org", "Dee", Access::Edit);
    flow.app.execute("calendar.openShared", json!({"address": "dee@example.org"})).unwrap();
    flow.job();
    let dee = flow.calendar("Dee").unwrap();
    assert_eq!(dee.rights, Rights::Edit);
    let made = flow
        .app
        .execute("calendar.new", json!({"title": "Review", "start": "2026-10-14T18:00", "end": "2026-10-14T19:00", "calendar": dee.id}))
        .unwrap();
    let event = made["id"].as_u64().unwrap();
    flow.job();
    flow.job();
    assert_eq!(
        flow.titles_in(dee.id),
        ["Review"],
        "{:?}",
        flow.app.session.mailbox.events.values().map(|e| (e.title.clone(), e.calendar)).collect::<Vec<_>>()
    );
    // It is on Dee's server calendar, not on the owner's.
    let there: Vec<String> = flow.server.state().events.iter().map(|e| e.subject.clone()).collect();
    assert_eq!(there, ["Own lunch"], "the owner's own calendar holds none of it");
    flow.app.execute("calendar.edit", json!({"id": event, "title": "Review (moved)"})).unwrap();
    flow.job();
    flow.job();
    assert_eq!(flow.titles_in(dee.id), ["Review (moved)"]);
    flow.app.execute("calendar.delete", json!({"id": event})).unwrap();
    flow.job();
    flow.job();
    assert!(flow.titles_in(dee.id).is_empty());
    assert!(flow.on_me().iter().all(|r| !r.contains("dee") && !r.contains("/users/")), "{:?}", flow.on_me());
}

#[test]
fn without_the_permission_the_owners_own_calendar_still_comes_and_the_window_is_told_to_ask() {
    let mut flow = Flow::start(not_yet);
    flow.server.state().add_person("cy@example.org", "Cy", Access::Read);
    flow.app.execute("calendar.openShared", json!({"address": "cy@example.org"})).unwrap();
    flow.job();
    // Nothing of Cy's was asked: the token can't carry it, and a refusal would read as "no rights".
    let seen = flow.server.state().requests.clone();
    assert!(seen.iter().all(|r| r.path.get(1).is_none_or(|p| p != "users")), "{seen:?}");
    assert!(flow.app.session.mailbox.events.values().any(|e| e.title == "Own lunch"));
    assert_eq!(flow.app.session.mailbox.account(flow.account).unwrap().opened_calendars.len(), 1, "kept for when it is allowed");
    assert!(flow.calendar("Cy").is_none());
}

#[test]
fn what_is_read_by_address_never_passes_the_accounts_cap_with_the_calendars_the_server_lists() {
    let mut flow = Flow::start(granted);
    // Fifteen calendars the server lists, and ten people's.
    for n in 0..15 {
        flow.server.state().add_calendar(&format!("Server {n}"), true, false);
    }
    for n in 0..10 {
        let address = format!("p{n}@example.org");
        flow.server.state().add_person(&address, &format!("Person {n}"), Access::Read);
        flow.app.execute("calendar.openShared", json!({"address": address})).unwrap();
    }
    flow.server.state().requests.clear();
    flow.job();
    let views = flow.server.state().requests.iter().filter(|r| r.path.last().is_some_and(|l| l == "calendarView")).count();
    // The main calendar, and at most twenty others: those opened by address first.
    assert!(views <= 21, "{views}");
    for n in 0..10 {
        assert!(flow.calendar(&format!("Person {n}")).is_some(), "Person {n} was read before the others");
    }
}
