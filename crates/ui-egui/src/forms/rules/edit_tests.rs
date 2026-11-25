// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Change Rule in the Rules dialog (#9999): the pencil opens the builder on the rule, Save
//! changes it in place, and an IMAP account's Sieve script is written again with it. The host's
//! answers are written out by hand, as `mailcraft-net`'s `sieve` brings them.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable as _;

use super::*;
use crate::dialogs::Dialog;
use crate::forms::Form;
use crate::sieve_replies::{SieveJob, SieveResult, SieveWrite};
use crate::{MailApp, Services};

/// 2026-09-21T13:46:40Z.
const NOW: i64 = 1_790_000_000;

const RULES: [&str; 6] = ["vacation", "date", "relational", "fileinto", "copy", "imap4flags"];

type Writes = Arc<Mutex<Vec<SieveWrite>>>;

/// A window with an IMAP account whose server keeps MailCraft's Sieve script, a folder there, and
/// a rule moving mail into it kept in the script; the host takes every write. The window, the
/// account, the folder, the rule and the scripts written.
fn window() -> (MailApp, Id, Id, Id, Writes) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let account =
        app.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    app.stand_in_password(account, "an invented password");
    let writes: Writes = Arc::default();
    let log = writes.clone();
    app.services.sieve = Some(Box::new(move |job: SieveJob| {
        let (tx, rx) = mpsc::channel();
        let (script, written) = match &job.write {
            Some(write) => {
                log.lock().unwrap().push(write.clone());
                (json!([{"name": write.name, "active": true}]), Some("done".to_string()))
            }
            None => (json!([]), None),
        };
        let text = job.write.as_ref().map(|w| w.text.clone());
        let state = json!({"extensions": RULES, "scripts": script, "script": text});
        tx.send(SieveResult { account: job.account.id, state: Some(state), written, ..Default::default() }).unwrap();
        rx
    }));
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    app.poll_sieve();
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    app.session.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let lists = app.execute("folder.new", json!({"name": "Lists", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    app.session.mailbox.folder_mut(lists).unwrap().remote = "INBOX/Lists".into();
    let made = app
        .execute(
            "rules.add",
            json!({"name": "Lists", "account": account, "conditions": [{"type": "fromContains", "value": "list"}], "actions": [{"type": "moveTo", "value": lists}]}),
        )
        .unwrap();
    assert_eq!(made["keep"], "server");
    app.poll_sieve();
    let rule = made["id"].as_u64().unwrap();
    assert!(app.session.mailbox.rules.iter().any(|r| r.id == rule && r.on_server()));
    (app, account, lists, rule, writes)
}

fn harness(app: MailApp) -> Harness<'static, MailApp> {
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1100.0, 900.0)).build_ui_state(
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

#[test]
fn the_pencil_opens_the_rule_and_save_writes_the_script_again_with_it() {
    let (mut app, account, lists, rule, writes) = window();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    let before = writes.lock().unwrap().len();
    h.get_by_label("Change this rule").click();
    h.run_steps(3);
    assert!(form(&mut h).editing.is_some());
    assert!(h.query_by_label("Save").is_some() && h.query_by_label("Add Rule").is_none());
    {
        let f = form(&mut h);
        assert_eq!((f.name.as_str(), f.conds[0].text.as_str(), f.acts[0].folder, f.server), ("Lists", "list", lists, true));
        f.name = "Mailing lists".into();
        f.conds[0].text = "digest; list".into();
    }
    h.run_steps(2);
    h.get_by_label("Save").click();
    h.run_steps(4);
    // Changed in place, and the builder is a new rule's again.
    let r = h.state().session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().clone();
    assert_eq!(r.name, "Mailing lists");
    assert_eq!(r.conditions, vec![mailcraft_model::Condition::FromContains(vec!["digest".into(), "list".into()])]);
    assert_eq!(form(&mut h).command().0, "rules.add");
    // The script went to the host again, the rule under its id, with its new words.
    let write = writes.lock().unwrap().last().cloned().unwrap();
    assert!(writes.lock().unwrap().len() > before);
    assert!(write.text.contains(&format!("# rule {rule}\r\nif address :all :contains \"from\" [\"digest\", \"list\"]")), "{}", write.text);
    h.state_mut().poll_sieve();
    assert!(h.state().session.mailbox.rules.iter().any(|r| r.id == rule && r.on_server()));
    // Cancel leaves the rule as it is.
    h.get_by_label("Change this rule").click();
    h.run_steps(2);
    form(&mut h).name = "Not kept".into();
    h.run_steps(1);
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state().session.mailbox.rules.iter().any(|r| r.id == rule && r.name == "Mailing lists"));
    assert_eq!(form(&mut h).command().0, "rules.add");
    let _ = account;
}

#[test]
fn the_dialog_opens_on_a_rule_to_change_when_asked() {
    let (app, account, _, rule, _) = window();
    let f = RulesForm::new(&app.session, &json!({"edit": rule}));
    assert_eq!((f.editing, f.account, f.open), (Some((rule, account)), account, true));
    assert_eq!(f.command().0, "rules.edit");
    // One that isn't there: a new rule, as always.
    let f = RulesForm::new(&app.session, &json!({"edit": 424242}));
    assert!(f.editing.is_none());
}
