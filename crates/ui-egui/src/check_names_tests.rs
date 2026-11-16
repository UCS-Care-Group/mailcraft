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

/// The host answers every look-up with `found`, or turns the password down (`refused`).
fn answers(app: &mut MailApp, found: Vec<EwsResolved>, refused: bool) {
    app.services.ews_resolve = Some(Box::new(move |job| {
        let (tx, rx) = std::sync::mpsc::channel();
        let error = refused.then(|| "The server turned the password down".to_string());
        tx.send(EwsResolveResult { account: job.request.account, found: found.clone(), error, auth_refused: refused }).unwrap();
        rx
    }));
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    )
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
    let mut h = window(app);
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().map(Dialog::name), Some("checkNames"));
    // The look-up is over: the status line doesn't say it is still under way.
    assert_eq!(status(h.state()), "");
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
    assert_eq!(status(&app), "Looking the names up in the organisation's address list…");
    app.execute("compose.close", json!({"discard": true})).unwrap();
    app.execute("mail.new", json!({"to": "grace"})).unwrap();
    app.poll_resolving();
    assert!(app.resolving.is_none());
    assert_eq!(to_line(&app), "grace");
    assert!(app.dialog.is_none());
    // Nor is it said to be under way any longer.
    assert_eq!(status(&app), "");
}

/// The message is sent from another account by the time the answer comes: what the first one's
/// organisation had is not put in.
#[test]
fn an_answer_for_another_account_than_the_message_now_goes_from_is_let_go_of() {
    let (mut app, _, asked) = writing("grace");
    let other = app
        .execute(
            "account.add",
            json!({"email": "bea@example.net", "kind": "exchange", "ews": {"url": "https://exchange.example.net/EWS/Exchange.asmx"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    app.run("compose.checkNames", json!({})).unwrap();
    assert_eq!(asked.lock().unwrap().len(), 1);
    app.execute("compose.set", json!({"account": other})).unwrap();
    app.poll_resolving();
    assert!(app.resolving.is_none() && app.dialog.is_none());
    assert_eq!(to_line(&app), "grace");
    assert_eq!(status(&app), "");
}

/// A name with a comma, quoted as an organisation writes it, is looked up as one name and put in.
#[test]
fn a_quoted_name_with_a_comma_is_looked_up_as_one() {
    let (mut app, _, asked) = writing("\"Smith, John\"");
    answers(
        &mut app,
        vec![EwsResolved { text: "Smith, John".into(), people: vec![person("John Smith", "john.smith@example.org", "")], more: false }],
        false,
    );
    let log = asked.clone();
    let host = app.services.ews_resolve.take().unwrap();
    app.services.ews_resolve = Some(Box::new(move |job| {
        log.lock().unwrap().push(job.request.texts.clone());
        host(job)
    }));
    app.run("compose.checkNames", json!({})).unwrap();
    assert_eq!(*asked.lock().unwrap(), [vec!["Smith, John".to_string()]]);
    app.poll_resolving();
    assert_eq!(to_line(&app), "John Smith <john.smith@example.org>");
}

/// One person found for a name the address list has more people for is asked about, not put in.
#[test]
fn one_person_found_where_the_address_list_has_more_is_asked_about() {
    let (mut app, _, _) = writing("grace");
    answers(&mut app, vec![EwsResolved { text: "grace".into(), people: vec![person("Grace Hopper", "grace@example.org", "")], more: true }], false);
    app.run("compose.checkNames", json!({})).unwrap();
    app.poll_resolving();
    assert_eq!(to_line(&app), "grace");
    let Some(Dialog::NameChoice(choice)) = &app.dialog else { panic!("{:?}", app.dialog.as_ref().map(Dialog::name)) };
    assert_eq!((choice.names.len(), choice.more.as_slice()), (1, ["grace".to_string()].as_slice()));
}

/// More names than one request holds are asked for ten at a time, up to fifty; the names beyond
/// are said not to have been looked up, not to have nobody.
#[test]
fn more_names_than_one_request_holds_are_asked_for_ten_at_a_time() {
    let names: Vec<String> = (0..52).map(|i| format!("name{i}")).collect();
    let (mut app, _, asked) = writing(&names.join("; "));
    app.run("compose.checkNames", json!({})).unwrap();
    for _ in 0..10 {
        app.poll_resolving();
    }
    assert!(app.resolving.is_none());
    let sizes: Vec<usize> = asked.lock().unwrap().iter().map(Vec::len).collect();
    assert_eq!(sizes, [10, 10, 10, 10, 10]);
    let said = status(&app);
    assert!(said.starts_with("Nobody matches: name0; name1; "), "{said}");
    assert!(said.ends_with(" · Not looked up in the address list: name50; name51"), "{said}");
}

/// A password the server turns down is forgotten, so that Send/Receive asks for it again, and
/// Check Names says that it is needed instead of asking with it.
#[test]
fn a_password_the_server_turns_down_is_forgotten() {
    let (mut app, account, _) = writing("grace");
    answers(&mut app, Vec::new(), true);
    app.run("compose.checkNames", json!({})).unwrap();
    app.poll_resolving();
    assert!(!app.passwords.contains_key(&account) && app.jmap_refused.contains(&account));
    assert_eq!(status(&app), "The server turned the password down");
    app.run("compose.checkNames", json!({})).unwrap();
    assert!(app.resolving.is_none());
    assert_eq!(status(&app), "The account's password is needed first: press Send/Receive and type it");
}

/// Working offline, only the contacts are matched, and that is why is said.
#[test]
fn working_offline_nothing_is_looked_up() {
    let (mut app, _, asked) = writing("grace");
    app.session.prefs.offline = true;
    app.run("compose.checkNames", json!({})).unwrap();
    assert!(asked.lock().unwrap().is_empty() && app.resolving.is_none());
    assert_eq!(status(&app), crate::send_receive::said());
}

/// The line changed while the owner chose, and the name is no longer on it: OK puts nothing in
/// and says so.
#[test]
fn a_choice_for_a_name_no_longer_on_the_line_says_so() {
    let (mut app, _, _) = writing("ada");
    app.run("compose.checkNames", json!({})).unwrap();
    app.poll_resolving();
    app.execute("compose.set", json!({"to": "somebody else"})).unwrap();
    let mut h = window(app);
    h.run_steps(3);
    assert_eq!(h.state().dialog.as_ref().map(Dialog::name), Some("checkNames"));
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(to_line(h.state()), "somebody else");
    assert_eq!(status(h.state()), "\"ada\" is no longer on the recipient lines");
}

#[test]
fn without_the_password_or_a_host_only_the_contacts_are_matched() {
    let (mut app, account, asked) = writing("grace");
    app.passwords.remove(&account);
    app.run("compose.checkNames", json!({})).unwrap();
    assert!(asked.lock().unwrap().is_empty() && app.resolving.is_none());
    // Why the address list wasn't asked is said.
    assert_eq!(status(&app), "The account's password is needed first: press Send/Receive and type it");

    let (mut app, _, _) = writing("grace");
    app.services.ews_resolve = None;
    app.run("compose.checkNames", json!({})).unwrap();
    assert!(app.resolving.is_none());
    assert_eq!(to_line(&app), "grace");
}

/// Ctrl+K pressed while typing in the To line is Check Names.
#[test]
fn ctrl_k_in_the_to_line_checks_names() {
    let (app, _, asked) = writing("");
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    h.event(egui::Event::Text("grace".into()));
    h.run_steps(3);
    assert_eq!(to_line(h.state()), "grace", "typed into To");
    let ctrl = egui::Modifiers { ctrl: !cfg!(target_os = "macos"), mac_cmd: cfg!(target_os = "macos"), command: true, ..Default::default() };
    h.key_press_modifiers(ctrl, egui::Key::K);
    h.run_steps(4);
    assert_eq!(asked.lock().unwrap().len(), 1, "looked up; status {:?}", status(h.state()));
}
