// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Every rule of a Sieve account can be changed (#2057): the pencil of a rule of the owner's own
//! script opens the rule builder when that loses nothing, else the Sieve editor on that rule's text
//! alone, whose Save writes the script with those bytes alone changed; Edit as Sieve… is in a row's
//! menu; one of MailCraft's own rules that the builder can't show opens as Sieve. The stand-in host
//! writes the script only while it is the one read, as `mailcraft-net`'s `sieve::write_own` does;
//! the script is synthetic.

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

const EXTENSIONS: [&str; 3] = ["fileinto", "imap4flags", "vacation"];

/// The owner's script: a rule the builder writes as it is, one it would write as other Sieve (a
/// header test on From), and one with an `else`.
const OWN: &str = "require [\"fileinto\"];\r\n\r\n# rule:[Lists]\r\nif header :contains \"subject\" \"[list]\" {\r\n  fileinto \"INBOX/Lists\";\r\n}\r\n\r\n# from the boss\r\nif header :contains \"from\" \"boss@example.org\" {\r\n  fileinto \"INBOX/Lists\";\r\n}\r\nif address :is \"from\" \"a@example.org\" {\r\n  stop;\r\n} else {\r\n  keep;\r\n}\r\n";

/// What the stand-in server keeps: the script's text, and every write of it.
type Own = Arc<Mutex<(String, Vec<SieveWrite>)>>;

/// A window with an IMAP account and its folder INBOX/Lists, whose server runs the owner's script
/// `mine`. The window, the account, and what the server keeps.
fn window() -> (MailApp, Id, Own) {
    use mailcraft_engine::sieve::digest;
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let account =
        app.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    app.stand_in_password(account, "an invented password");
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    app.session.mailbox.folder_mut(inbox).unwrap().remote = "INBOX".into();
    let lists = app.execute("folder.new", json!({"name": "Lists", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    app.session.mailbox.folder_mut(lists).unwrap().remote = "INBOX/Lists".into();
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
        let state = json!({"extensions": EXTENSIONS, "scripts": [{"name": "mine", "active": true}], "script": null,
            "activeScript": {"name": "mine", "text": kept.0}});
        tx.send(SieveResult { account: job.account.id, state: Some(state), written, ..Default::default() }).unwrap();
        rx
    }));
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    app.poll_sieve();
    (app, account, own)
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
fn the_pencil_opens_the_builder_when_it_loses_nothing_else_the_rules_sieve() {
    let (mut app, account, own) = window();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    // Lists, the boss's, the chain's if and its else: every one can be changed.
    assert_eq!(pencils(&h), vec![true, true, true, true]);

    // Lists: the builder, which writes it as it is.
    h.get_all_by_label("Change this rule").next().unwrap().click();
    h.run_steps(3);
    {
        let f = form(&mut h);
        assert!(f.open && f.own.is_some() && f.sieve.is_none());
        assert_eq!((f.name.as_str(), f.conds[0].kind, f.conds[0].text.as_str()), ("Lists", When::Subject, "[list]"));
    }
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(!form(&mut h).open);

    // The boss's: a header test the builder would write as an address test, so its Sieve.
    h.get_all_by_label("Change this rule").nth(1).unwrap().click();
    h.run_steps(3);
    let text = {
        let f = form(&mut h);
        assert!(!f.open);
        let editor = f.sieve.as_ref().expect("the Sieve editor is open");
        assert_eq!(editor.own.as_ref().and_then(|o| o.rule.as_ref()).map(|r| r.filter), Some(false));
        editor.text.clone()
    };
    assert_eq!(text, "# from the boss\nif header :contains \"from\" \"boss@example.org\" {\n  fileinto \"INBOX/Lists\";\n}");
    assert!(h.query_by_label_contains("The rule builder can't change it").is_some());
    // Changed and saved: those bytes alone are written, with the script's CRLF.
    if let Some(editor) = form(&mut h).sieve.as_mut() {
        editor.text = text.replace("boss@", "chief@");
    }
    h.run_steps(30);
    h.get_by_label("Save").click();
    h.run_steps(8);
    let kept = own.lock().unwrap().0.clone();
    assert_eq!(kept, OWN.replace("boss@example.org", "chief@example.org"));
    assert!(form(&mut h).sieve.is_none(), "saved, the editor closes");
    let shown = h.state().session.sieve.get(&account).and_then(|k| k.elsewhere.clone()).unwrap();
    assert_eq!(shown.rules[1].conditions, vec![mailcraft_model::Condition::FromContains(vec!["chief@example.org".into()])]);

    // The chain: its Sieve, the if and its else together.
    h.get_all_by_label("Change this rule").nth(3).unwrap().click();
    h.run_steps(3);
    let chain = form(&mut h).sieve.as_ref().map(|e| e.text.clone()).unwrap_or_default();
    assert_eq!(chain, "if address :is \"from\" \"a@example.org\" {\n  stop;\n} else {\n  keep;\n}");
}

#[test]
fn edit_as_sieve_is_in_a_rows_menu_and_one_of_mailcrafts_rules_opens_as_sieve() {
    let (mut app, account, _) = window();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    // Lists can be changed in the builder; its menu has Edit as Sieve… too.
    h.get_all_by_label("Change this rule").next().unwrap().click();
    h.run_steps(3);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    let rule = h.state().session.sieve.get(&account).and_then(|k| k.elsewhere.as_ref()).map(|e| e.rules[0].id).unwrap();
    // (The menu's choice, as a right click and a click on it make it.)
    {
        let s = &h.state().session;
        assert!(crate::sieve_editor::text_offered(h.state(), s.sieve[&account].elsewhere.as_ref().unwrap().rules.first().unwrap()));
    }
    let editor = crate::sieve_editor::SieveEditor::as_sieve(h.state(), rule).unwrap();
    assert_eq!(editor.text, "# rule:[Lists]\nif header :contains \"subject\" \"[list]\" {\n  fileinto \"INBOX/Lists\";\n}");
    assert_eq!(editor.command().0, "sieve.putOwnBlock");

    // One of MailCraft's own rules, kept in its script on a server without the owner's script:
    // its words have a semicolon the builder sets words apart with, so it opens as Sieve.
    let (mut app, account, _) = window();
    let made = app
        .execute(
            "rules.add",
            json!({"account": account, "name": "Odd", "conditions": [{"type": "subjectContains", "value": ["a;b", "c"]}], "actions": [{"type": "flag"}], "server": true}),
        )
        .unwrap();
    let id = made["id"].as_u64().unwrap();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = harness(app);
    // (It is listed first, on the server, over the owner's script's rules.)
    assert_eq!(pencils(&h).first(), Some(&true));
    h.get_all_by_label("Change this rule").next().unwrap().click();
    h.run_steps(3);
    let (convert, text) = form(&mut h).sieve.as_ref().map(|e| (e.convert, e.text.clone())).unwrap();
    assert!(convert);
    assert_eq!(text, "if header :contains \"subject\" [\"a;b\", \"c\"] {\n    addflag \"\\\\Flagged\";\n}\n");
    assert!(h.query_by_label_contains("kept as Sieve text from now on").is_some());
    h.run_steps(30);
    h.get_by_label("Save").click();
    h.run_steps(4);
    let r = h.state().session.mailbox.rules.iter().find(|r| r.id == id).cloned().unwrap();
    assert!(r.is_sieve() && r.name == "Odd");
}
