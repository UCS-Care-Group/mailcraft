// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Sieve rule editor in Rules and Alerts (#9999), driven as the owner would: Advanced… › New
//! Sieve rule…, the check here as the text changes, a click on a problem putting the cursor there,
//! Check asking the (stand-in) server and its line said as the rule's own, Save, and the pencil
//! opening a Sieve rule again. The host's answers are written by hand, as `mailcraft-net`'s
//! `sieve` brings them; every address is invented.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use mailcraft_model::Id;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::sieve_editor::{SieveEditor, char_at};
use crate::sieve_replies::{ServerCheck, SieveJob, SieveResult};
use crate::{MailApp, Services};

/// 2026-09-21T13:46:40Z.
const NOW: i64 = 1_790_000_000;

const EXTENSIONS: [&str; 6] = ["vacation", "fileinto", "copy", "imap4flags", "reject", "body"];

/// The scripts the host was asked to check.
type Checks = Arc<Mutex<Vec<String>>>;

/// A window with an IMAP account whose server keeps MailCraft's Sieve script. The stand-in host
/// takes every write, and checks a script by refusing any line with `not-an-address` in it, by
/// its line in the whole script, as Dovecot says it.
fn window() -> (MailApp, Id, Checks) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let account =
        app.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    app.stand_in_password(account, "an invented password");
    let checks: Checks = Arc::default();
    let log = checks.clone();
    app.services.sieve = Some(Box::new(move |job: SieveJob| {
        let (tx, rx) = mpsc::channel();
        if let Some(script) = &job.check {
            log.lock().unwrap().push(script.clone());
            let bad = script.split("\r\n").position(|l| l.contains("not-an-address"));
            let checked = match bad {
                Some(i) => ServerCheck::Errors(format!("line {}: error: redirect: invalid address.\nerror: validation failed.", i + 1)),
                None => ServerCheck::Fine,
            };
            tx.send(SieveResult { account: job.account.id, checked: Some(checked), ..Default::default() }).unwrap();
            return rx;
        }
        let (scripts, written) = match &job.write {
            Some(write) => (json!([{"name": write.name, "active": true}]), Some("done".to_string())),
            None => (json!([]), None),
        };
        let text = job.write.as_ref().map(|w| w.text.clone());
        let state = json!({"extensions": EXTENSIONS, "scripts": scripts, "script": text});
        tx.send(SieveResult { account: job.account.id, state: Some(state), written, ..Default::default() }).unwrap();
        rx
    }));
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    app.poll_sieve();
    (app, account, checks)
}

fn harness(app: MailApp) -> Harness<'static, MailApp> {
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1100.0, 1000.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn editor<'a>(h: &'a mut Harness<'static, MailApp>) -> &'a mut SieveEditor {
    let Some(Dialog::Form(Form::Rules(f))) = &mut h.state_mut().dialog else { panic!("the Rules dialog is open") };
    f.sieve.as_mut().expect("the Sieve rule editor is open")
}

fn editor_open(h: &mut Harness<'static, MailApp>) -> bool {
    matches!(&h.state().dialog, Some(Dialog::Form(Form::Rules(f))) if f.sieve.is_some())
}

/// Whether the button `label` can be pressed.
fn enabled(h: &Harness<'static, MailApp>, label: &str) -> bool {
    !h.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn a_new_sieve_rule_is_written_checked_here_and_on_the_server_and_saved() {
    let (mut app, account, checks) = window();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    h.get_by_label("Advanced…").click();
    h.run_steps(2);
    h.get_by_label("New Sieve rule…").click();
    h.run_steps(3);
    assert!(editor_open(&mut h));
    assert_eq!(editor(&mut h).account, account);
    // The starter is Sieve with no mistake; without a name it can't be saved yet.
    assert!(editor(&mut h).text.contains("fileinto"));
    assert!(editor(&mut h).problems.is_empty());
    assert!(!enabled(&h, "Save"));

    // A mistake on line 2: listed, Save waits, and a click on it puts the cursor there.
    {
        let e = editor(&mut h);
        e.name = "Newsletters".into();
        e.text = "if address :is \"from\" \"news@example.org\" {\n  bogus;\n}\n".into();
    }
    h.run_steps(3);
    let problems = editor(&mut h).problems.clone();
    assert!(problems.iter().any(|p| !p.warning && p.line == 2), "{problems:?}");
    assert!(!enabled(&h, "Save"));
    h.query_all_by_label_contains("Line 2, column 3").next().expect("the problem is listed").click();
    h.run_steps(3);
    assert!(editor(&mut h).goto.is_none(), "the cursor was put there");

    // Corrected, then refused by the server: its line is the rule's own.
    editor(&mut h).text = "if address :is \"from\" \"news@example.org\" {\n  redirect \"not-an-address\";\n}\n".into();
    h.run_steps(3);
    assert!(editor(&mut h).problems.is_empty(), "{:?}", editor(&mut h).problems);
    assert!(enabled(&h, "Save") && enabled(&h, "Check"));
    h.get_by_label("Check").click();
    h.run_steps(4);
    let asked = checks.lock().unwrap().last().cloned().unwrap();
    assert!(
        asked.contains("# Sieve rule: Newsletters\r\nif address :is \"from\" \"news@example.org\" {\r\n  redirect \"not-an-address\";"),
        "{asked}"
    );
    assert!(h.query_by_label("The server doesn't take this rule:").is_some());
    assert!(h.query_all_by_label_contains("Line 2 of the rule: line").next().is_some());
    // The server's words that name no line of the rule, with a word on its line numbers.
    assert!(h.query_by_label("error: validation failed.").is_some());
    // Warnings and the server's words don't keep it from being saved.
    editor(&mut h).text = "if address :is \"from\" \"news@example.org\" {\n  fileinto \"INBOX\";\n}\n".into();
    h.run_steps(3);
    h.get_by_label("Check").click();
    h.run_steps(4);
    assert!(h.query_by_label("The server takes this rule.").is_some());
    h.get_by_label("Save").click();
    h.run_steps(4);
    assert!(!editor_open(&mut h));
    let rule = h.state().session.mailbox.rules.iter().find(|r| r.is_sieve()).cloned().expect("the Sieve rule is kept");
    assert_eq!((rule.name.as_str(), rule.account), ("Newsletters", account));
    assert!(rule.sieve.contains("fileinto \"INBOX\";"));

    // The pencil opens it in the editor again; Save changes it in place.
    h.get_by_label("Change this rule").click();
    h.run_steps(3);
    assert!(editor_open(&mut h));
    assert_eq!((editor(&mut h).id, editor(&mut h).text.clone()), (Some(rule.id), rule.sieve.clone()));
    editor(&mut h).text = "discard;\n".into();
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(4);
    let changed = h.state().session.mailbox.rules.iter().find(|r| r.id == rule.id).cloned().unwrap();
    assert_eq!((changed.name.as_str(), changed.sieve.as_str()), ("Newsletters", "discard;\n"));
    assert!(!editor_open(&mut h));
}

#[test]
fn check_waits_while_working_offline_and_cancel_keeps_nothing() {
    let (mut app, _, checks) = window();
    app.session.prefs.offline = true;
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    h.get_by_label("Advanced…").click();
    h.run_steps(2);
    h.get_by_label("New Sieve rule…").click();
    h.run_steps(3);
    editor(&mut h).name = "Big".into();
    h.run_steps(2);
    assert!(!enabled(&h, "Check"));
    assert!(enabled(&h, "Save"));
    assert!(h.query_by_label("You're working offline.").is_some());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(!editor_open(&mut h));
    assert!(!h.state().session.mailbox.rules.iter().any(|r| r.is_sieve()));
    assert!(checks.lock().unwrap().is_empty());
}

#[test]
fn advanced_is_offered_only_for_an_account_whose_server_keeps_sieve() {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let h = harness(app);
    assert!(h.query_by_label("Advanced…").is_none());
}

#[test]
fn the_cursor_goes_to_a_line_and_column_within_the_text() {
    let text = "ab\nçd\n\nlast";
    assert_eq!(char_at(text, 1, 1), 0);
    assert_eq!(char_at(text, 2, 2), 4);
    // Past the end of a line: its end; past the last line: the end of the text.
    assert_eq!(char_at(text, 2, 99), 5);
    assert_eq!(char_at(text, 3, 1), 6);
    assert_eq!(char_at(text, 4, 3), 9);
    assert_eq!(char_at(text, 99, 1), text.chars().count());
    assert_eq!(char_at("", 0, 0), 0);
}
