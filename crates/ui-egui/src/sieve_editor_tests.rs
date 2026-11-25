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

/// The owner's own script on the stand-in server, as Roundcube would write it: CRLF throughout.
const OWN: &str = "# rule:[Lists]\r\nrequire [\"fileinto\"];\r\nif header :contains \"subject\" \"[list]\" {\r\n  fileinto \"Lists\";\r\n}\r\n";

/// What the stand-in server keeps: the owner's script's text, and every write of it.
type Own = Arc<Mutex<(String, Vec<crate::sieve_replies::SieveWrite>)>>;

/// A window with an IMAP account whose server runs the owner's own script `roundcube`. The
/// stand-in host writes it only while it is still the one read (by its digest), as
/// `mailcraft-net`'s `sieve::write_own` does, and reads it back after every job.
fn own_window() -> (MailApp, Id, Own) {
    use mailcraft_engine::sieve::digest;
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let account =
        app.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    app.stand_in_password(account, "an invented password");
    let own: Own = Arc::new(Mutex::new((OWN.to_string(), Vec::new())));
    let server = own.clone();
    app.services.sieve = Some(Box::new(move |job: SieveJob| {
        let (tx, rx) = mpsc::channel();
        let mut kept = server.lock().unwrap();
        let written = job.write.as_ref().map(|w| {
            kept.1.push(w.clone());
            if w.own && w.last_written.as_deref() == Some(digest(&kept.0).as_str()) {
                kept.0 = w.text.clone();
                "done".to_string()
            } else {
                "changedElsewhere".to_string()
            }
        });
        let state = json!({"extensions": EXTENSIONS, "scripts": [{"name": "roundcube", "active": true}], "script": null,
            "activeScript": {"name": "roundcube", "text": kept.0}});
        tx.send(SieveResult { account: job.account.id, state: Some(state), written, ..Default::default() }).unwrap();
        rx
    }));
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    app.poll_sieve();
    (app, account, own)
}

#[test]
fn the_owners_own_script_is_edited_whole_saved_as_typed_and_reloaded_when_it_changed_there() {
    use mailcraft_engine::sieve::digest;
    let (mut app, account, own) = own_window();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    // Beside its rules, shown read-only, a button opens the whole script.
    assert!(h.query_by_label("In your Sieve script \"roundcube\"").is_some());
    h.get_by_label("Edit Your Sieve Script…").click();
    h.run_steps(3);
    assert!(editor_open(&mut h));
    {
        let e = editor(&mut h);
        assert_eq!(e.account, account);
        // Shown with plain line breaks, written with CRLF again: byte for byte when left alone.
        assert_eq!(e.text, OWN.replace("\r\n", "\n"));
        assert_eq!(e.written(), OWN);
        assert_eq!(e.own.as_ref().map(|o| o.digest.clone()), Some(digest(OWN)));
    }
    // Roundcube's mark: the note stays up.
    assert!(h.query_all_by_label_contains("written by a webmail (Roundcube)").next().is_some());
    assert!(!enabled(&h, "Save"), "nothing has changed yet");
    // A mistake keeps it from being saved.
    editor(&mut h).text = OWN.replace("\r\n", "\n").replace("fileinto \"Lists\";", "bogus;");
    h.run_steps(3);
    assert!(editor(&mut h).problems.iter().any(|p| !p.warning && p.line == 4), "{:?}", editor(&mut h).problems);
    assert!(!enabled(&h, "Save"));
    // Changed, saved: written as typed, CRLF and all, under the digest of the read.
    editor(&mut h).text = OWN.replace("\r\n", "\n").replace("\"Lists\"", "\"Lists/Old\"");
    h.run_steps(3);
    assert!(enabled(&h, "Save"));
    h.get_by_label("Save").click();
    h.run_steps(5);
    let edited = OWN.replace("\"Lists\"", "\"Lists/Old\"");
    {
        let kept = own.lock().unwrap();
        let w = kept.1.last().cloned().expect("the host was asked to write");
        assert!(w.own && !w.activate);
        assert_eq!((w.name.as_str(), w.text.as_str(), w.last_written.as_deref()), ("roundcube", edited.as_str(), Some(digest(OWN).as_str())));
        assert_eq!(kept.0, edited);
    }
    assert!(!editor_open(&mut h), "saved, the editor closes");
    // The rules shown are read from the new text.
    let shown = h.state().session.sieve.get(&account).and_then(|k| k.elsewhere.clone()).unwrap();
    assert_eq!(shown.text, edited);

    // Opened again from Advanced…, while the script changes in webmail: nothing is written, and
    // Reload brings the server's text.
    h.get_by_label("Advanced…").click();
    h.run_steps(2);
    h.get_by_label("Edit your Sieve script \"roundcube\"…").click();
    h.run_steps(3);
    assert!(editor_open(&mut h));
    let webmail = format!("{edited}# rule:[Later]\r\nif size :over 1M {{\r\n  discard;\r\n}}\r\n");
    own.lock().unwrap().0 = webmail.clone();
    editor(&mut h).text = "keep;\n".into();
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(5);
    assert_eq!(own.lock().unwrap().0, webmail, "nothing was written over");
    assert!(editor_open(&mut h));
    assert!(editor(&mut h).own.as_ref().is_some_and(|o| o.changed));
    assert!(h.query_all_by_label_contains("changed on the server since you opened it").next().is_some());
    assert!(!enabled(&h, "Save"));
    h.get_by_label("Reload").click();
    h.run_steps(6);
    assert!(editor_open(&mut h));
    let e = editor(&mut h);
    assert_eq!(e.written(), webmail);
    assert!(e.own.as_ref().is_some_and(|o| !o.changed && o.digest == digest(&webmail)));
}

#[test]
fn a_script_of_rainloops_says_so_and_one_without_crlf_stays_without() {
    let (mut app, account, own) = own_window();
    own.lock().unwrap().0 = "# RAINLOOP:SIEVE\nrequire \"fileinto\";\nkeep;\n".into();
    app.sieve_try_again(account);
    app.poll_sieve();
    let e = SieveEditor::own_script(&app, account).expect("the owner's script is known");
    assert_eq!(e.own.as_ref().and_then(|o| o.webmail), Some(mailcraft_engine::cmd::sieve_own::Webmail::RainLoop));
    assert_eq!(e.written(), "# RAINLOOP:SIEVE\nrequire \"fileinto\";\nkeep;\n");
    // Mixed line breaks are kept as they are.
    own.lock().unwrap().0 = "keep;\r\nstop;\n".into();
    app.sieve_try_again(account);
    app.poll_sieve();
    let e = SieveEditor::own_script(&app, account).unwrap();
    assert_eq!((e.text.as_str(), e.written().as_str()), ("keep;\r\nstop;\n", "keep;\r\nstop;\n"));
}
