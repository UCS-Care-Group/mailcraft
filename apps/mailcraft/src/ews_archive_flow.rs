// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The online archive of an Exchange server account (#1818), read by the window the whole way:
//! a Send/Receive of the mailbox asks the server whether there is an archive, the archive appears
//! as an account of its own, and its run goes to the owner's server with the owner's password,
//! reading its folders and none of the mailbox's. Nothing of it is written to the mailbox file.
//! Everything goes to a loopback stand-in; every name, address and id is invented.

use mailcraft_engine::Session;
use mailcraft_model::{Id, Mailbox};
use mailcraft_ui_egui::{MailApp, Services};
use serde_json::json;

use crate::ews_mail_flow::{ME, NOW, PASSWORD, USER, send_receive, serve};

fn window(url: &str) -> (MailApp, Id) {
    let session = Session::at(Mailbox::with_local_account("", ""), NOW, 0);
    let mut app = MailApp::new(session, Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let ctx = egui::Context::default();
    let (run_ctx, fetch_ctx) = (ctx.clone(), ctx);
    app.services.ews = Some(Box::new(move |job| crate::ews_runner::run_mail(job, &run_ctx)));
    app.services.ews_fetch = Some(Box::new(move |job| crate::ews_runner::run_fetch(job, &fetch_ctx)));
    let added = app.execute("account.add", json!({"email": ME, "kind": "exchange", "ews": {"url": url, "username": USER}})).unwrap();
    (app, added["id"].as_u64().unwrap())
}

#[test]
fn the_archive_appears_and_is_read_with_the_owners_sign_in() {
    let server = serve();
    {
        let mut state = server.standin.state();
        state.deliver("inbox", "Kickoff", false);
        state.add_archive(true);
        let old = state.add_own_folder("", "2019", "archivemsgfolderroot");
        state.deliver(&old, "Budget 2019", true);
    }
    let (mut app, account) = window(&server.url);
    assert_eq!(send_receive(&mut app, json!({"account": account, "password": PASSWORD}))["ews"], true);
    let archive = app.session.mailbox.archive_of(account).map(|a| a.id).expect("the server said there is one");
    assert!(app.session.mailbox.folders.iter().all(|f| f.account != archive), "nothing of it read yet");

    // Its own run: no password asked, the owner's goes; its folders, and none of the mailbox's.
    let started = send_receive(&mut app, json!({"account": archive}));
    assert_eq!(started["ews"], true, "{started}");
    assert!(app.dialog.is_none(), "no password asked for the archive");
    let names: Vec<&str> = app.session.mailbox.folders.iter().filter(|f| f.account == archive).map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"2019") && names.contains(&"Inbox") && names.contains(&"Deleted Items"), "{names:?}");
    assert!(!names.contains(&"Sent Items"));
    assert!(app.session.mailbox.messages.values().all(|m| m.account != archive || m.subject != "Kickoff"));
    // The folder on screen is read when it is one of the archive's.
    let year = app.session.mailbox.folders.iter().find(|f| f.account == archive && f.name == "2019").map(|f| f.id).unwrap();
    app.session.view.folder = year;
    send_receive(&mut app, json!({"account": archive}));
    assert!(app.session.mailbox.messages.values().any(|m| m.account == archive && m.subject == "Budget 2019"));
    assert!(server.standin.state().writes.is_empty(), "reading writes nothing");

    // Nothing of it is written to the file.
    let written = String::from_utf8(mailcraft_engine::io::write_mailbox(&app.session.mailbox).unwrap()).unwrap();
    assert!(!written.contains("Budget 2019") && !written.contains("\"2019\""));
}

#[test]
fn a_server_without_an_archive_shows_none() {
    let server = serve();
    let (mut app, account) = window(&server.url);
    send_receive(&mut app, json!({"account": account, "password": PASSWORD}));
    assert!(app.session.mailbox.archive_of(account).is_none());
    assert!(app.session.sync_status.starts_with("Up to date"), "the run went well: {}", app.session.sync_status);
}
