// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check Names in the window with the address list of an Exchange server account (#1262), the
//! host played by a stand-in that answers as told: what is asked, what is put in, what is offered.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::sync::{DirectoryKind, DirectoryPerson, EwsResolveResult, EwsResolved};
use mailcraft_model::{Id, Mailbox};
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn person(name: &str, email: &str, department: &str) -> DirectoryPerson {
    DirectoryPerson { name: name.into(), email: email.into(), kind: DirectoryKind::Mailbox, department: department.into(), ..Default::default() }
}

/// What the stand-in host answers every look-up with.
fn answer() -> Vec<EwsResolved> {
    vec![
        EwsResolved { text: "grace".into(), people: vec![person("Grace Hopper", "grace@example.org", "")], more: false },
        EwsResolved {
            text: "ada".into(),
            people: vec![person("Ada Byron", "ada.byron@example.org", "Engines"), person("Ada Lovelace", "ada.lovelace@example.org", "Accounts")],
            more: true,
        },
        EwsResolved { text: "nobody".into(), people: Vec::new(), more: false },
    ]
}

/// A window with one Exchange server account whose password was typed, a message being written to
/// `to`, and a host that writes down what it was asked and answers [`answer`] at once.
fn writing(to: &str) -> (MailApp, Id, Arc<Mutex<Vec<Vec<String>>>>) {
    let mut app = MailApp::new(Session::at(Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let added = app
        .execute(
            "account.add",
            json!({"email": "me@example.org", "kind": "exchange", "ews": {"url": "https://exchange.example.org/EWS/Exchange.asmx"}}),
        )
        .unwrap();
    let account = added["id"].as_u64().unwrap();
    app.passwords.insert(account, "an invented password".into());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let log = asked.clone();
    app.services.ews_resolve = Some(Box::new(move |job| {
        assert_eq!(job.secret, "an invented password");
        log.lock().unwrap().push(job.request.texts.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(EwsResolveResult { account: job.request.account, found: answer(), ..Default::default() }).unwrap();
        rx
    }));
    app.execute("mail.new", json!({"account": account, "to": to})).unwrap();
    (app, account, asked)
}

fn to_line(app: &MailApp) -> String {
    app.session.compose.as_ref().map(|c| c.to.clone()).unwrap_or_default()
}

fn status(app: &MailApp) -> String {
    app.status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default()
}

#[test]
fn the_names_no_contact_settles_are_looked_up_and_one_person_is_put_in() {
    let (mut app, _, asked) = writing("grace; ada; nobody; bob@example.org");
    app.run("compose.checkNames", json!({})).unwrap();
    // Only the names nobody matched were asked for, and nothing was put in yet.
    assert_eq!(*asked.lock().unwrap(), [vec!["grace".to_string(), "ada".into(), "nobody".into()]]);
    assert_eq!(to_line(&app), "grace; ada; nobody; bob@example.org");
    app.poll_resolving();
    assert_eq!(to_line(&app), "Grace Hopper <grace@example.org>; ada; nobody; bob@example.org");
    assert_eq!(status(&app), "Nobody matches: nobody");
    // The name with two people is asked about.
    let Some(Dialog::NameChoice(choice)) = &app.dialog else { panic!("{:?}", app.dialog.as_ref().map(Dialog::name)) };
    assert_eq!(choice.names.len(), 1);
    assert_eq!(choice.more, ["ada"]);
}

#[test]
fn the_owner_chooses_one_of_several_in_the_dialog() {
    let (mut app, _, _) = writing("ada");
    app.run("compose.checkNames", json!({})).unwrap();
    app.poll_resolving();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().map(Dialog::name), Some("checkNames"));
    // The second, chosen and taken with OK.
    if let Some(Dialog::NameChoice(choice)) = &mut h.state_mut().dialog {
        choice.selected = 1;
    }
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(to_line(h.state()), "Ada Lovelace <ada.lovelace@example.org>");
    assert!(h.state().dialog.is_none());
}

#[test]
fn a_script_or_an_agent_has_nothing_looked_up_and_nothing_asked() {
    let (mut app, _, asked) = writing("grace; ada");
    let v = app.execute("compose.checkNames", json!({})).unwrap();
    assert_eq!(v["unresolved"], json!(["grace", "ada"]));
    assert_eq!(v["directory"], json!(true));
    assert!(asked.lock().unwrap().is_empty());
    assert!(app.resolving.is_none() && app.dialog.is_none());
}

#[test]
fn an_answer_for_a_message_no_longer_written_is_let_go_of() {
    let (mut app, _, asked) = writing("grace");
    app.run("compose.checkNames", json!({})).unwrap();
    assert_eq!(asked.lock().unwrap().len(), 1);
    app.execute("compose.close", json!({"discard": true})).unwrap();
    app.execute("mail.new", json!({"to": "grace"})).unwrap();
    app.poll_resolving();
    assert!(app.resolving.is_none());
    assert_eq!(to_line(&app), "grace");
    assert!(app.dialog.is_none());
}

#[test]
fn without_the_password_or_a_host_only_the_contacts_are_matched() {
    let (mut app, account, asked) = writing("grace");
    app.passwords.remove(&account);
    app.run("compose.checkNames", json!({})).unwrap();
    assert!(asked.lock().unwrap().is_empty() && app.resolving.is_none());
    assert_eq!(status(&app), "Nobody matches: grace");

    let (mut app, _, _) = writing("grace");
    app.services.ews_resolve = None;
    app.run("compose.checkNames", json!({})).unwrap();
    assert!(app.resolving.is_none());
    assert_eq!(to_line(&app), "grace");
}
