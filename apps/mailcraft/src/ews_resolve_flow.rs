// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check Names with an Exchange server account's address list (#1262), the whole way: the window,
//! the host's own runner (`ews_runner::run_resolve`) and `mailcraft_net::https`, to a loopback
//! server that answers with the strict stand-in. Nothing here leaves the machine; every name and
//! address is invented.

use std::time::Duration;

use mailcraft_engine::Session;
use mailcraft_ews::standin::StandIn;
use mailcraft_ews::standin_directory::Person;
use mailcraft_model::{Id, Mailbox};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;

use crate::ews_mail_flow::{ME, NOW, PASSWORD, USER, send_receive, serve};

/// A window with one Exchange server account on `url`, the host's own look-up, and a message being
/// written to `to`.
fn window(url: &str, to: &str) -> (MailApp, Id) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let (ctx, run_ctx) = (egui::Context::default(), egui::Context::default());
    app.services.ews = Some(Box::new(move |job| crate::ews_runner::run_mail(job, &run_ctx)));
    app.services.ews_resolve = Some(Box::new(move |job| crate::ews_runner::run_resolve(job, &ctx)));
    let added = app.execute("account.add", json!({"email": ME, "kind": "exchange", "ews": {"url": url, "username": USER}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    app.execute("mail.new", json!({"account": account, "to": to})).unwrap();
    (app, account)
}

/// A Send/Receive the password was typed for; what the server was asked in it is forgotten.
fn signed_in(app: &mut MailApp, account: Id, standin: &StandIn) {
    send_receive(app, json!({"account": account, "password": PASSWORD}));
    standin.state().requests.clear();
}

/// Check Names pressed by the owner, to the end of its look-up.
fn check_names(app: &mut MailApp) {
    app.run("compose.checkNames", json!({})).unwrap();
    for _ in 0..5_000 {
        app.poll_resolving();
        if !app.resolving_names() {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the look-up didn't end");
}

fn to_line(app: &MailApp) -> String {
    app.session.compose.as_ref().map(|c| c.to.clone()).unwrap_or_default()
}

#[test]
fn names_are_looked_up_in_the_address_list_and_nothing_else_is_asked() {
    let server = serve();
    server.standin.state().directory = vec![
        Person { department: "Engines".into(), ..Person::new("Grace Hopper", "grace@example.org") },
        Person::new("Ada Byron", "ada.byron@example.org"),
        Person::new("Ada Lovelace", "ada.lovelace@example.org"),
    ];
    let (mut app, account) = window(&server.url, "grace; ada; nobody");
    signed_in(&mut app, account, &server.standin);
    check_names(&mut app);
    assert_eq!(to_line(&app), "Grace Hopper <grace@example.org>; ada; nobody");
    assert_eq!(app.dialog.as_ref().map(|d| d.name()), Some("checkNames"));
    // One ResolveNames for each name as typed, and nothing that writes.
    let state = server.standin.state();
    assert!(state.writes.is_empty());
    let asked: Vec<&str> = state.requests.iter().filter(|r| r.operation == "ResolveNames").map(|r| r.body.as_str()).collect();
    assert_eq!(asked.len(), 3, "{:?}", state.requests.iter().map(|r| &r.operation).collect::<Vec<_>>());
    assert!(asked[0].contains("<m:UnresolvedEntry>grace</m:UnresolvedEntry>"));
    // (Besides them only the first request, with no body and no credential, which asks how to sign in.)
    assert!(
        state.requests.iter().all(|r| r.operation == "ResolveNames" || r.body.is_empty()),
        "{:?}",
        state.requests.iter().map(|r| &r.operation).collect::<Vec<_>>()
    );
}

#[test]
fn a_password_the_server_turns_down_is_said_and_nothing_is_put_in() {
    let server = serve();
    server.standin.state().directory = vec![Person::new("Grace Hopper", "grace@example.org")];
    let (mut app, account) = window(&server.url, "grace");
    signed_in(&mut app, account, &server.standin);
    // The password is changed on the server meanwhile.
    server.standin.state().authorization = mailcraft_ews::Secret::basic(USER, "a new password").unwrap().expose().to_owned();
    check_names(&mut app);
    assert_eq!(to_line(&app), "grace");
    let said = app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(said.contains("turned the password down"), "{said}");
    assert!(!said.contains(&server.url) && !said.contains(USER));
}

/// People's Address Book: what is typed in the search box is looked up in the address list, and who
/// is found is shown with how they are reached; the home number the server also holds is not kept.
#[test]
fn the_address_book_searches_the_address_list_for_what_is_typed() {
    let server = serve();
    server.standin.state().directory = vec![
        Person {
            title: "Controller".into(),
            phone: "+1 555 0100".into(),
            mobile: "+1 555 0199".into(),
            office: "Building 2".into(),
            home: "+1 555 0123".into(),
            ..Person::new("Grace Hopper", "grace@example.org")
        },
        Person { kind: "PublicDL".into(), ..Person::new("Graceful Team", "team@example.org") },
    ];
    let (mut app, account) = window(&server.url, "");
    signed_in(&mut app, account, &server.standin);
    app.run("people.addressBook", json!({"account": account})).unwrap();
    app.type_people_search("grace");
    let ctx = egui::Context::default();
    for _ in 0..5_000 {
        app.poll_address_book(&ctx);
        if !app.address_book_found().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let found = app.address_book_found().to_vec();
    let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Grace Hopper", "Graceful Team"]);
    assert_eq!((found[0].phone.as_str(), found[0].mobile.as_str(), found[0].office.as_str()), ("+1 555 0100", "+1 555 0199", "Building 2"));
    assert!(!format!("{:?}", serde_json::to_value(&found).unwrap()).contains("0123"));
    // One look-up for what was typed, nothing that writes, and nothing kept in the mailbox.
    let state = server.standin.state();
    let asked: Vec<&str> = state.requests.iter().filter(|r| r.operation == "ResolveNames").map(|r| r.body.as_str()).collect();
    assert_eq!(asked.len(), 1);
    assert!(asked[0].contains("<m:UnresolvedEntry>grace</m:UnresolvedEntry>"));
    assert!(state.writes.is_empty());
    assert!(app.session.mailbox.contacts.is_empty());
}
