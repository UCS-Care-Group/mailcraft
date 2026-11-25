// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The filters of the owner's own Sieve script, as a webmail of the RainLoop kind writes them, in
//! the Rules dialog (#9999): shown as rules that can be changed, the pencil opening the builder on
//! one, Save writing its settings and Sieve again in the webmail's shape, New Filter… adding one,
//! a filter changed by hand staying read-only. The stand-in host writes the script only while it
//! is the one read, as `mailcraft-net`'s `sieve::write_own` does; the script is synthetic.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

use super::*;
use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::sieve_replies::{SieveJob, SieveResult, SieveWrite};
use crate::{MailApp, Services};

/// 2026-09-21T13:46:40Z.
const NOW: i64 = 1_790_000_000;

const EXTENSIONS: [&str; 3] = ["fileinto", "body", "vacation"];

const LIST: &str = r#"{"ID":"6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd","Enabled":true,"Name":"ML-list","Conditions":[{"Field":"Recipient","Type":"Contains","Value":"list@lists.example.org","ValueSecond":""}],"ConditionsType":"Any","ActionType":"MoveTo","ActionValue":"Mailing-Lists/list","ActionValueSecond":"","ActionValueThird":"","ActionValueFourth":"","Keep":true,"Stop":true,"MarkAsRead":false}"#;
const SPAM: &str = r#"{"ID":"0000017000590660000","Enabled":true,"Name":"Spam","Conditions":[{"Field":"Header","Type":"EqualTo","Value":"YES","ValueSecond":"X-Spam-Flag"}],"ConditionsType":"Any","ActionType":"MoveTo","ActionValue":"Junk","ActionValueSecond":"","ActionValueThird":"","ActionValueFourth":"","Keep":true,"Stop":true,"MarkAsRead":false}"#;

fn header(id: &str, json: &str) -> String {
    let b64 = mailcraft_engine::cmd::base64_encode(json.as_bytes());
    let lines: Vec<String> = b64.as_bytes().chunks(74).map(|c| String::from_utf8(c.to_vec()).unwrap()).collect();
    format!("/*\nBEGIN:FILTER:{id}\nBEGIN:HEADER\n{}\nEND:HEADER\n*/", lines.join("\n"))
}

const HEAD: &str =
    "require [\"fileinto\",\"body\"];\n\n# This is SnappyMail sieve script.\n# Please don't change anything here.\n# RAINLOOP:SIEVE\n\n";

fn spam_block() -> String {
    format!(
        "{}\n\nif header :is [\"X-Spam-Flag\"] \"YES\"\n{{\n    fileinto \"Junk\";\n    stop;\n}}\n\n/* END:FILTER */",
        header("0000017000590660000", SPAM)
    )
}

fn list_block(json: &str) -> String {
    let sieve = "if header :contains [\"To\", \"CC\"] \"list@lists.example.org\"\n{\n    fileinto \"Mailing-Lists/list\";\n    stop;\n}";
    format!("{}\n\n{sieve}\n\n/* END:FILTER */", header("6orktqi8w2n5v0c3b7x1z9q4m8k2j6hd", json))
}

fn script() -> String {
    format!("{HEAD}{}\n\n{}\n", spam_block(), list_block(LIST))
}

/// What the stand-in server keeps: the script's text, and every write of it.
type Own = Arc<Mutex<(String, Vec<SieveWrite>)>>;

/// A window with an IMAP account, its folders Junk and Mailing-Lists/list, whose server runs the
/// webmail's script `snappymail`. The window, the account, Junk, and what the server keeps.
fn window() -> (MailApp, Id, Id, Own) {
    use mailcraft_engine::sieve::digest;
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let account =
        app.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    app.stand_in_password(account, "an invented password");
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    app.session.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let junk = app.execute("folder.new", json!({"name": "Junk", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    app.session.mailbox.folder_mut(junk).unwrap().remote = "Junk".into();
    let list = app.execute("folder.new", json!({"name": "list", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    app.session.mailbox.folder_mut(list).unwrap().remote = "Mailing-Lists/list".into();
    let own: Own = Arc::new(Mutex::new((script(), Vec::new())));
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
        let state = json!({"extensions": EXTENSIONS, "scripts": [{"name": "snappymail", "active": true}], "script": null,
            "activeScript": {"name": "snappymail", "text": kept.0}});
        tx.send(SieveResult { account: job.account.id, state: Some(state), written, ..Default::default() }).unwrap();
        rx
    }));
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    app.poll_sieve();
    (app, account, junk, own)
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

fn form<'a>(h: &'a mut Harness<'static, MailApp>) -> &'a mut RulesForm {
    let Some(Dialog::Form(Form::Rules(f))) = &mut h.state_mut().dialog else { panic!("the Rules dialog is open") };
    f
}

/// The pencils of the rules shown, in order, and whether each can be pressed.
fn pencils(h: &Harness<'static, MailApp>) -> Vec<bool> {
    h.get_all_by_label("Change this rule").map(|n| !n.accesskit_node().is_disabled()).collect()
}

#[test]
fn a_filter_is_changed_in_the_builder_and_written_in_the_webmails_shape() {
    let (mut app, account, junk, own) = window();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    assert!(h.query_by_label("In your Sieve script \"snappymail\"").is_some());
    assert!(h.query_by_label("New Filter…").is_some());
    assert_eq!(pencils(&h), vec![true, true]);
    // The second filter, in the builder: its recipient, its folder, stopping.
    h.get_all_by_label("Change this rule").nth(1).unwrap().click();
    h.run_steps(3);
    {
        let f = form(&mut h);
        assert!(f.webmail.is_some() && f.editing.is_some_and(|(_, a)| a == account));
        assert_eq!((f.name.as_str(), f.conds.len(), f.conds[0].kind, f.conds[0].text.as_str()), ("ML-list", 1, When::To, "list@lists.example.org"));
        assert_eq!(f.acts.len(), 1);
        assert!(f.stop);
        f.name = "Lists".into();
        f.acts[0].folder = junk;
    }
    assert!(h.query_by_label("+ condition").is_none() && h.query_by_label("+ action").is_none());
    h.run_steps(2);
    h.get_by_label("Save").click();
    h.run_steps(6);
    let (text, writes) = own.lock().unwrap().clone();
    let w = writes.last().cloned().expect("the host was asked to write");
    assert!(w.own && !w.activate && w.name == "snappymail");
    let json = LIST.replace("\"ML-list\"", "\"Lists\"").replace("\"Mailing-Lists/list\"", "\"Junk\"");
    let changed = list_block(&json).replace("fileinto \"Mailing-Lists/list\"", "fileinto \"Junk\"");
    assert_eq!(text, format!("{HEAD}{}\n\n{changed}\n", spam_block()), "only that filter's bytes changed");
    // Read back: the rule shown is the filter as changed, and can be changed again.
    let shown = h.state().session.sieve.get(&account).and_then(|k| k.elsewhere.clone()).unwrap();
    assert_eq!(shown.rules[1].name, "Lists");
    assert!(form(&mut h).editing.is_none());
    assert_eq!(pencils(&h), vec![true, true]);
}

#[test]
fn a_new_filter_is_added_and_one_changed_by_hand_stays_read_only() {
    let (mut app, account, junk, own) = window();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    h.get_by_label("New Filter…").click();
    h.run_steps(3);
    {
        let f = form(&mut h);
        assert!(f.webmail.is_some() && f.editing.is_none());
        assert_eq!(f.conds[0].kind, When::Header);
        f.name = "Invoices".into();
        f.conds[0].kind = When::HeaderIs;
        f.conds[0].names = "X-Kind".into();
        f.conds[0].text = "invoice".into();
        f.acts[0].folder = junk;
    }
    h.run_steps(2);
    h.get_by_label("Add Rule").click();
    h.run_steps(6);
    let text = own.lock().unwrap().0.clone();
    assert!(text.starts_with(&script().trim_end().to_string()), "what was there stays");
    assert!(text.contains("\nif header :is [\"X-Kind\"] \"invoice\"\n{\n    fileinto \"Junk\";\n    stop;\n}\n\n/* END:FILTER */\n"), "{text}");
    let shown = h.state().session.sieve.get(&account).and_then(|k| k.elsewhere.clone()).unwrap();
    assert_eq!(shown.rules.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["Spam", "ML-list", "Invoices"]);

    // Changed by hand in the server's copy: that filter is shown, and can't be changed here.
    own.lock().unwrap().0 = script().replace("fileinto \"Junk\";", "fileinto \"Junk\";\n    addflag \"\\\\Seen\";");
    h.state_mut().sieve_try_again(account);
    h.state_mut().poll_sieve();
    h.run_steps(3);
    assert_eq!(pencils(&h), vec![false, true]);
    let tip = mailcraft_engine::sieve_rainloop::ReadOnly::Disagrees.said();
    assert!(mailcraft_engine::cmd::rules_webmail::read_only(&h.state().session, shown.rules[0].id).is_some_and(|w| w.said() == tip));
}

/// With `MAILCRAFT_SHOTS_DIR` set, the list and the builder on a filter, in light and dark, to
/// look at.
#[test]
fn shots_of_the_webmails_filters() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else { return };
    for theme in ["light", "dark"] {
        let (mut app, _, _, own) = window();
        own.lock().unwrap().0 = script().replace("fileinto \"Junk\";", "fileinto \"Junk\";\n    addflag \"\\\\Seen\";");
        app.execute("ui.theme", json!({"value": theme})).unwrap();
        app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
        let mut h = harness(app);
        let account = h.state().session.mailbox.accounts.iter().find(|a| a.email == "ada@example.org").map(|a| a.id).unwrap();
        h.state_mut().sieve_try_again(account);
        h.state_mut().poll_sieve();
        h.run_steps(4);
        h.get_all_by_label("Change this rule").nth(1).unwrap().click();
        h.run_steps(4);
        h.render().unwrap().save(std::path::Path::new(&dir).join(format!("webmail-filters-{theme}.png"))).unwrap();
    }
}
