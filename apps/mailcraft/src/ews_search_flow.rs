// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! More results on the server for an Exchange server account (#1725), the whole way: the window,
//! the host's own runners (`ews_runner::run_search`, `run_fetch`) and `mailcraft_net::https`, to a
//! loopback server that answers with the strict stand-in. Nothing here leaves the machine; every
//! message is invented.

use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_ews::standin::StandIn;
use mailcraft_model::{Id, Mailbox};
use mailcraft_ui_egui::server_search::Link;
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;

use crate::ews_mail_flow::{ME, NOW, PASSWORD, USER, send_receive, serve};

/// A window with one Exchange server account on `url` and the host's own runners.
fn window(url: &str) -> (MailApp, Id) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let (run_ctx, search_ctx, fetch_ctx) = (egui::Context::default(), egui::Context::default(), egui::Context::default());
    app.services.ews = Some(Box::new(move |job| crate::ews_runner::run_mail(job, &run_ctx)));
    app.services.ews_search = Some(Box::new(move |job| crate::ews_runner::run_search(job, &search_ctx)));
    app.services.ews_fetch = Some(Box::new(move |job| crate::ews_runner::run_fetch(job, &fetch_ctx)));
    let added = app.execute("account.add", json!({"email": ME, "kind": "exchange", "ews": {"url": url, "username": USER}})).unwrap();
    (app, added["id"].as_u64().unwrap())
}

/// A Send/Receive the password was typed for; what the server was asked in it is forgotten.
fn signed_in(app: &mut MailApp, account: Id, standin: &StandIn) {
    send_receive(app, json!({"account": account, "password": PASSWORD}));
    standin.state().requests.clear();
}

/// The search typed and More results on the server clicked, to the end of what the server gives.
fn search_on_server(app: &mut MailApp, query: &str) {
    app.run("view.search", json!({"query": query, "scope": "everywhere"})).unwrap();
    app.run("ui.moreResultsOnServer", json!({})).unwrap();
    for _ in 0..5_000 {
        app.drive_server_search();
        if app.server_link() != Link::Searching {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the search didn't end");
}

#[test]
fn mail_that_is_only_on_the_server_is_found_there_opened_and_fetched_whole() {
    let server = serve();
    let (mut app, account) = window(&server.url);
    signed_in(&mut app, account, &server.standin);
    // Mail that came since: on the server, not on this computer.
    {
        let mut state = server.standin.state();
        let projects = state.add_folder("Projects");
        state.deliver("inbox", "Budget for the spring", false);
        state.deliver("inbox", "Lunch on Friday", false);
        state.deliver(&projects, "The budget of the old project", true);
    }
    search_on_server(&mut app, "budget");
    assert_eq!(app.server_link(), Link::End);
    assert!(app.session.server_search.errors.is_empty(), "{:?}", app.session.server_search.errors);
    // The Inbox's is found; the new folder isn't one this computer has yet, so it wasn't asked.
    let subjects: Vec<&str> = app.session.server_search.results.iter().map(|r| r.found.subject.as_str()).collect();
    assert_eq!(subjects, ["Budget for the spring"]);
    {
        let state = server.standin.state();
        assert!(state.writes.is_empty(), "{:?}", state.writes);
        let finds = state.requests.iter().filter(|r| r.operation == "FindItem").count();
        assert!(finds >= 1 && state.requests.iter().all(|r| r.operation == "FindItem" || r.body.is_empty()), "{:?}", state.requests);
        // (Each a query string: the search, and once the word nobody wrote that checks the server looks at it.)
        let asked: Vec<&str> = state.requests.iter().filter(|r| r.operation == "FindItem").map(|r| r.body.as_str()).collect();
        assert!(asked.iter().all(|body| body.contains("<m:QueryString>budget</m:QueryString>") || body.contains("<m:QueryString>zq")));
        assert!(asked.first().is_some_and(|body| body.contains("<m:QueryString>budget</m:QueryString>")));
    }

    // Opened: in the Inbox, and fetched whole by the host as any message not here yet.
    app.run("search.openServerResult", json!({"index": 0})).unwrap();
    let id = app.session.sel.messages.first().copied().unwrap();
    assert!(app.session.mailbox.message(id).is_some_and(|m| m.unfetched));
    for _ in 0..5_000 {
        app.drive_fetch();
        if app.session.mailbox.message(id).is_some_and(|m| !m.unfetched) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let m = app.session.mailbox.message(id).unwrap();
    assert!(!m.unfetched && m.subject == "Budget for the spring", "the message came whole");
    assert!(server.standin.state().writes.is_empty());
}

#[test]
fn a_password_the_server_turns_down_is_said_without_the_server_or_the_owner() {
    let server = serve();
    let (mut app, account) = window(&server.url);
    signed_in(&mut app, account, &server.standin);
    server.standin.state().deliver("inbox", "Budget for the spring", false);
    // The password is changed on the server meanwhile.
    server.standin.state().authorization = mailcraft_ews::Secret::basic(USER, "a new password").unwrap().expose().to_owned();
    search_on_server(&mut app, "budget");
    let said = app.session.server_search.errors.join(" ");
    assert!(app.session.server_search.results.is_empty());
    assert!(said.contains("turned down the sign-in"), "{said}");
    assert!(!said.contains(&server.url) && !said.contains(USER) && !said.contains("budget"), "{said}");
}
