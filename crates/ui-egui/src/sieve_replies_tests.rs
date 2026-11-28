// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An IMAP account's Automatic Replies in the window (#2057): the job, File › Info's line, and the
//! dialog in its Sieve shape. The host's answers are written out by hand, as `mailcraft-net`'s
//! `sieve` brings them.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_engine::sieve::SCRIPT_NAME;
use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::auto_replies::Draft;
use crate::dialogs::Dialog;
use crate::sieve_replies::{SieveJob, SieveResult, SieveTrouble, SieveWrite};
use crate::{MailApp, Services};

/// 2026-09-21T13:46:40Z.
const NOW: i64 = 1_790_000_000;

type Answers = Arc<Mutex<VecDeque<SieveResult>>>;
type Jobs = Arc<Mutex<Vec<(Id, Option<SieveWrite>, String)>>>;

/// A window with an IMAP account whose password was typed, and a host that answers from a queue.
fn window() -> (MailApp, Id, Answers, Jobs) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let added = app.execute("account.add", json!({"email": "ada@example.org", "incoming": {"host": "imap.example.org"}})).unwrap();
    let account = added["id"].as_u64().unwrap();
    app.stand_in_password(account, "an invented password");
    let (answers, jobs): (Answers, Jobs) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.sieve = Some(Box::new(move |job: SieveJob| {
        log.lock().unwrap().push((job.account.id, job.write.clone(), job.secret.clone()));
        let (tx, rx) = mpsc::channel();
        // An answer of the test's, else a write taken as it came, MailCraft's script then active.
        let answer = to_answer.lock().unwrap().pop_front().unwrap_or_else(|| match &job.write {
            Some(write) => SieveResult {
                account: job.account.id,
                state: Some(json!({"extensions": ["vacation", "date", "relational", "fileinto", "copy", "imap4flags"],
                    "scripts": [{"name": write.name, "active": true}], "script": write.text})),
                written: Some("done".into()),
                ..Default::default()
            },
            None => SieveResult::default(),
        });
        tx.send(answer).unwrap();
        rx
    }));
    (app, account, answers, jobs)
}

fn state(extensions: &[&str], active: Option<&str>, script: Option<&str>) -> Value {
    let scripts: Vec<Value> = active.iter().map(|name| json!({"name": name, "active": true})).collect();
    json!({"extensions": extensions, "scripts": scripts, "script": script})
}

const ALL: [&str; 3] = ["vacation", "date", "relational"];

/// The window reads them, and the host answers with `result`.
fn read(app: &mut MailApp, account: Id, answers: &Answers, result: SieveResult) {
    app.sieve.read.clear();
    answers.lock().unwrap().push_back(SieveResult { account, ..result });
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    app.poll_sieve();
}

fn said(app: &MailApp) -> String {
    app.status_msg.clone().unwrap_or_default().0
}

#[test]
fn a_read_goes_to_the_host_with_the_password_and_is_taken_in() {
    let (mut app, account, answers, jobs) = window();
    assert_eq!(app.sieve_line(account), "Automatic replies: not read yet");
    read(&mut app, account, &answers, SieveResult { state: Some(state(&ALL, None, None)), ..Default::default() });
    assert_eq!(jobs.lock().unwrap().as_slice(), &[(account, None, "an invented password".to_string())]);
    assert_eq!(app.sieve_line(account), "Automatic replies: off");
    // Read a moment ago: not asked again.
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    assert_eq!(jobs.lock().unwrap().len(), 1);
    // Offline: nothing goes.
    app.sieve.read.clear();
    app.session.prefs.offline = true;
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    assert_eq!(jobs.lock().unwrap().len(), 1);
}

#[test]
fn a_change_writes_the_engines_script_and_says_how_it_went() {
    let (mut app, account, answers, jobs) = window();
    read(&mut app, account, &answers, SieveResult { state: Some(state(&ALL, None, None)), ..Default::default() });
    // The host writes, then reads back what the server has: here, what was written.
    let change =
        app.session.run("account.setAutomaticReplies", &json!({"account": account, "status": "alwaysEnabled", "internal": "Away."})).unwrap();
    let text = change["sieve"]["text"].as_str().unwrap().to_string();
    answers.lock().unwrap().push_back(SieveResult {
        account,
        state: Some(state(&ALL, Some(SCRIPT_NAME), Some(&text))),
        written: Some("done".into()),
        ..Default::default()
    });
    app.execute("account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "Away."})).unwrap();
    app.poll_sieve();
    let write = jobs.lock().unwrap()[1].1.clone().unwrap();
    assert_eq!((write.name.as_str(), write.text.as_str(), write.last_written, write.activate), (SCRIPT_NAME, text.as_str(), None, true));
    assert_eq!(said(&app), "Automatic replies are on");
    assert_eq!(app.auto_replies_on(), vec![account]);
    assert_eq!(app.sieve_line(account), "Automatic replies: on, until you turn them off");
}

#[test]
fn what_the_server_cant_do_is_said_in_file_info() {
    let (mut app, account, answers, _) = window();
    read(&mut app, account, &answers, SieveResult { unavailable: true, ..Default::default() });
    assert!(app.sieve_line(account).contains("doesn't offer Sieve scripts"));
    assert_eq!(app.sieve.trouble.get(&account), Some(&SieveTrouble::Unavailable));

    let (mut app, account, answers, _) = window();
    read(&mut app, account, &answers, SieveResult { sign_in_refused: true, ..Default::default() });
    assert!(app.sieve_line(account).contains("didn't take your sign-in"));

    let (mut app, account, answers, _) = window();
    read(&mut app, account, &answers, SieveResult { state: Some(state(&["fileinto"], None, None)), ..Default::default() });
    assert!(app.sieve_line(account).contains("can't send them"));

    let (mut app, account, answers, _) = window();
    read(&mut app, account, &answers, SieveResult { state: Some(state(&ALL, Some(SCRIPT_NAME), Some("keep;"))), ..Default::default() });
    assert!(app.sieve_line(account).contains("changed elsewhere"));

    let (mut app, account, answers, _) = window();
    read(&mut app, account, &answers, SieveResult { state: Some(state(&ALL, Some("roundcube"), None)), ..Default::default() });
    assert_eq!(app.sieve_line(account), "Automatic replies: off · your Sieve script \"roundcube\" runs instead");
}

#[test]
fn without_a_password_nothing_is_asked() {
    let (mut app, account, _, jobs) = window();
    app.passwords.clear();
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    assert!(jobs.lock().unwrap().is_empty());
}

/// What the host found where the server was looked for, as `mailcraft-net` says it: a port that
/// expects encryption from the first byte.
fn unreached() -> SieveResult {
    SieveResult {
        unavailable: true,
        why: "sieve.example.org:4190 expects an encrypted connection from the start, which ManageSieve doesn't use".into(),
        advice: "On Dovecot, set ssl = no on the sieve listener.".into(),
        ..Default::default()
    }
}

#[test]
fn what_was_found_where_the_server_was_looked_for_is_said_everywhere() {
    let (mut app, account, answers, jobs) = window();
    read(&mut app, account, &answers, unreached());
    assert_eq!(app.sieve.trouble.get(&account), Some(&SieveTrouble::Unavailable));
    // File › Info's line names where, and offers Try again with what to do as its tip.
    assert_eq!(
        app.sieve_line(account),
        "Automatic replies: not available (sieve.example.org:4190 expects an encrypted connection from the start, which ManageSieve doesn't use)"
    );
    let (_, again) = app.auto_replies_line(account).unwrap();
    assert_eq!(again, Some(crate::auto_replies::Again::TryAgain("On Dovecot, set ssl = no on the sieve listener.".into())));
    // Not asked again unasked...
    app.sieve.read.clear();
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    assert_eq!(jobs.lock().unwrap().len(), 1);
    // ...but at Try Again, and then it is said no more once the server answers.
    answers.lock().unwrap().push_back(SieveResult { account, state: Some(state(&ALL, None, None)), ..Default::default() });
    app.sieve_try_again(account);
    app.poll_sieve();
    assert_eq!(jobs.lock().unwrap().len(), 2);
    assert_eq!(app.sieve_line(account), "Automatic replies: off");
    assert!(app.sieve_unreached(account).is_none() && app.auto_replies_line(account).unwrap().1.is_none());

    // Another Sieve server named in Account Settings is asked at once.
    let (mut app, account, answers, jobs) = window();
    read(&mut app, account, &answers, unreached());
    app.execute("account.edit", json!({"id": account, "sieve": {"host": "imap.example.org", "port": 4191}})).unwrap();
    app.sieve.read.clear();
    app.execute("account.automaticReplies", json!({"account": account})).unwrap();
    assert_eq!(jobs.lock().unwrap().len(), 2);

    // A change that couldn't go says why in the status bar.
    let (mut app, account, answers, _) = window();
    read(&mut app, account, &answers, SieveResult { state: Some(state(&ALL, None, None)), ..Default::default() });
    answers.lock().unwrap().push_back(SieveResult { account, ..unreached() });
    app.execute("account.setAutomaticReplies", json!({"account": account, "status": "alwaysEnabled", "internal": "Away."})).unwrap();
    app.poll_sieve();
    assert_eq!(
        said(&app),
        "Your automatic replies weren't changed: sieve.example.org:4190 expects an encrypted connection from the start, which ManageSieve doesn't use"
    );
}

#[test]
fn the_dialog_says_what_was_found_and_offers_try_again() {
    let (mut app, account, answers, jobs) = window();
    read(&mut app, account, &answers, unreached());
    let mut h = dialog(app, account);
    assert!(!draft(&h).editable(h.state()));
    assert!(
        h.query_by_label(
            "MailCraft can't keep automatic replies on this account's server: sieve.example.org:4190 expects an encrypted connection from the start, which ManageSieve doesn't use. On Dovecot, set ssl = no on the sieve listener."
        )
        .is_some()
    );
    let before = jobs.lock().unwrap().len();
    h.get_by_label("Try again").click();
    h.run_steps(2);
    assert_eq!(jobs.lock().unwrap().len(), before + 1);
}

/// The window drawing the dialog for `account`.
fn dialog(mut app: MailApp, account: Id) -> Harness<'static, MailApp> {
    let draft = Draft::open(&mut app, &json!({"account": account})).unwrap();
    app.dialog = Some(Dialog::AutomaticReplies(Box::new(draft)));
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1000.0, 760.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn draft<'a>(h: &'a Harness<'static, MailApp>) -> &'a Draft {
    match h.state().dialog.as_ref() {
        Some(Dialog::AutomaticReplies(d)) => d,
        _ => panic!("no Automatic Replies dialog"),
    }
}

#[test]
fn the_dialog_offers_what_a_sieve_server_can_keep() {
    let (mut app, account, answers, _) = window();
    read(&mut app, account, &answers, SieveResult { state: Some(state(&["vacation"], None, None)), ..Default::default() });
    let mut h = dialog(app, account);
    assert!(draft(&h).sieve && draft(&h).editable(h.state()));
    // No time range without date tests, said; no "my contacts only" outside.
    assert!(h.query_by_label("This server can't send them only during a time range.").is_some());
    h.get_by_label("Send automatic replies").click();
    h.run_steps(2);
    h.get_by_label("Outside My Organization (On)").click();
    h.run_steps(2);
    assert!(h.query_by_label("Only my contacts").is_none());
    assert!(h.query_by_label("Reply to people outside my organization").is_some());
    assert_eq!(draft(&h).params()["audience"], "all");
}

#[test]
fn a_script_changed_elsewhere_is_replaced_only_by_the_owners_click() {
    let (mut app, account, answers, _) = window();
    read(&mut app, account, &answers, SieveResult { state: Some(state(&ALL, Some(SCRIPT_NAME), Some("keep;"))), ..Default::default() });
    let mut h = dialog(app, account);
    assert!(!draft(&h).editable(h.state()));
    h.get_by_label("Replace with MailCraft's").click();
    h.run_steps(2);
    assert!(draft(&h).replace.is_some() && draft(&h).editable(h.state()));
    // Bound to the script as the owner saw it (#2058).
    assert_eq!(draft(&h).params()["replace"], mailcraft_engine::sieve::digest("keep;"));
    assert!(h.query_by_label("Replace with MailCraft's").is_none());
    // Changed elsewhere again before OK: the engine refuses to write over what the owner didn't see.
    let params = draft(&h).params();
    let app = h.state_mut();
    read(app, account, &answers, SieveResult { state: Some(state(&ALL, Some(SCRIPT_NAME), Some("discard;"))), ..Default::default() });
    let e = app.session.run("account.setAutomaticReplies", &params).unwrap_err().to_string();
    assert!(e.contains("changed again"), "{e}");
}

const RULES: [&str; 6] = ["vacation", "date", "relational", "fileinto", "copy", "imap4flags"];

/// The window's IMAP account with a folder on its server, and a rule moving mail there that its
/// server is to keep; the folder and the rule.
fn with_rule(app: &mut MailApp, account: Id, answers: &Answers) -> (Id, Id) {
    read(app, account, answers, SieveResult { state: Some(state(&RULES, None, None)), ..Default::default() });
    let inbox = app.session.mailbox.folder_of_kind(account, mailcraft_model::FolderKind::Inbox).unwrap();
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
    (lists, made["id"].as_u64().unwrap())
}

#[test]
fn a_rule_kept_on_the_server_is_written_into_the_script_and_shown_there() {
    let (mut app, account, answers, jobs) = window();
    let (_, rule) = with_rule(&mut app, account, &answers);
    // The change went to the host as the whole script, replies (off) first, then the rule.
    let write = jobs.lock().unwrap().last().and_then(|(_, w, _)| w.clone()).unwrap();
    assert!(write.text.contains(&format!("# rule {rule}")) && write.text.contains("fileinto \"INBOX/Lists\""), "{}", write.text);
    assert!(write.activate);

    // The server took it and runs it.
    app.poll_sieve();
    assert_eq!(said(&app), "Your rules are kept on the server");
    let r = app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().clone();
    assert_eq!(crate::forms::rules::mark(&app, &r, account), "On the server");
    assert_eq!(app.sieve_rules_note(account), None);
}

#[test]
fn the_rules_window_says_what_keeps_them_off_the_server_and_offers_the_replacement() {
    let (mut app, account, answers, jobs) = window();
    let (_, rule) = with_rule(&mut app, account, &answers);
    app.poll_sieve();
    // The owner's own script runs, MailCraft's beside it as written.
    let written = jobs.lock().unwrap().iter().rev().find_map(|(_, w, _)| w.clone()).unwrap().text;
    let ours_and_theirs =
        json!({"extensions": RULES, "scripts": [{"name": SCRIPT_NAME, "active": false}, {"name": "roundcube", "active": true}], "script": written});
    read(&mut app, account, &answers, SieveResult { state: Some(ours_and_theirs), ..Default::default() });
    let (note, replace) = app.sieve_rules_note(account).unwrap();
    assert!(note.contains("\"roundcube\" runs on the server instead") && !replace, "{note}");
    let r = app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().clone();
    assert!(
        crate::forms::rules::mark(&app, &r, account).contains("another Sieve script of yours"),
        "{}",
        crate::forms::rules::mark(&app, &r, account)
    );
    // Changed elsewhere: the Rules window offers Replace with MailCraft's, and only its click writes.
    let theirs = "require \"fileinto\";\r\nfileinto \"Other\";\r\n";
    read(&mut app, account, &answers, SieveResult { state: Some(state(&RULES, Some(SCRIPT_NAME), Some(theirs))), ..Default::default() });
    assert!(app.sieve_rules_note(account).is_some_and(|(note, replace)| replace && note.contains("changed elsewhere")));
    let writes = || jobs.lock().unwrap().iter().filter(|(_, w, _)| w.is_some()).count();
    let before = writes();
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1100.0, 800.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    assert_eq!(writes(), before, "nothing is written over unasked");
    h.get_by_label("Replace with MailCraft's").click();
    h.run_steps(3);
    let write = jobs.lock().unwrap().iter().rev().find_map(|(_, w, _)| w.clone()).unwrap();
    assert_eq!(write.last_written.as_deref(), Some(mailcraft_engine::sieve::digest(theirs).as_str()));
    assert!(write.text.contains(&format!("# rule {rule}")));
}

const THEIRS: &str = "require [\"fileinto\", \"vacation\"];\r\nif address :contains \"from\" \"shop.example\" { fileinto \"INBOX/Lists\"; stop; }\r\nif header :regex \"subject\" \"x\" { redirect \"elsewhere@example.net\"; }\r\nif true { vacation \"Away (theirs).\"; }\r\n";

#[test]
fn the_owners_own_script_is_listed_read_only_with_its_replies_said_in_file_info() {
    let (mut app, account, answers, _) = window();
    let (_, _) = with_rule(&mut app, account, &answers);
    app.poll_sieve();
    let ours = app.session.sieve.get(&account).and_then(|k| k.now.clone());
    assert!(ours.is_some());
    let written = jobs_last_text(&mut app);
    let state = json!({"extensions": RULES, "scripts": [{"name": SCRIPT_NAME, "active": false}, {"name": "roundcube", "active": true}],
        "script": written, "activeScript": {"name": "roundcube", "text": THEIRS}});
    read(&mut app, account, &answers, SieveResult { state: Some(state), ..Default::default() });
    assert_eq!(app.sieve_line(account), "Automatic replies: set by your Sieve script \"roundcube\", which runs on the server");
    let theirs = app.session.sieve.get(&account).and_then(|k| k.elsewhere.clone()).unwrap();
    assert_eq!(theirs.rules.len(), 2);
    let id = theirs.rules[0].id;
    // Shown in Rules and Alerts under its script, with nothing to press.
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1100.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    assert!(h.query_by_label("In your Sieve script \"roundcube\"").is_some());
    assert!(h.query_by_label("It also sends automatic replies of its own.").is_some());
    let rules_before = h.state().session.mailbox.rules.len();
    // Whatever asks: refused, and nothing changes.
    for (command, params) in [("rules.remove", json!({"id": id})), ("rules.enable", json!({"id": id, "value": false}))] {
        let said = h.state_mut().execute(command, params).unwrap_err();
        assert!(said.contains("\"roundcube\""), "{said}");
    }
    assert_eq!(h.state().session.mailbox.rules.len(), rules_before);
}

/// The text of the last script the window had the host write.
fn jobs_last_text(app: &mut MailApp) -> String {
    let account = app.sieve_accounts()[0];
    let due = app.session.run("sync.sieveScript", &json!({"account": account})).unwrap();
    // Nothing due: what is written is what the rules make now.
    assert!(due["sieve"].is_null(), "{due}");
    let known = app.session.auto_replies.get(&account).cloned().unwrap_or_default();
    let vacation = mailcraft_engine::sieve::Vacation {
        status: known.status,
        start: known.start,
        end: known.end,
        internal: known.internal,
        external: known.external,
        audience: known.audience,
    };
    mailcraft_engine::cmd::sieve_replies::whole_script(&app.session.mailbox, account, &vacation).0
}

#[test]
fn the_rules_window_says_what_was_found_where_the_server_was_looked_for() {
    let (mut app, account, answers, jobs) = window();
    let (lists, _) = with_rule(&mut app, account, &answers);
    app.poll_sieve();
    // A rule change that couldn't go to the server says why.
    answers.lock().unwrap().push_back(SieveResult { account, ..unreached() });
    let before = jobs.lock().unwrap().len();
    app.execute(
        "rules.add",
        json!({"name": "News", "account": account, "conditions": [{"type": "fromContains", "value": "news"}], "actions": [{"type": "moveTo", "value": lists}]}),
    )
    .unwrap();
    app.poll_sieve();
    assert_eq!(jobs.lock().unwrap().len(), before + 1);
    assert_eq!(
        said(&app),
        "Your rules weren't changed on the server: sieve.example.org:4190 expects an encrypted connection from the start, which ManageSieve doesn't use"
    );
    let (note, replace) = app.sieve_rules_note(account).unwrap();
    assert_eq!(
        note,
        "MailCraft can't keep this account's rules on its server, so they run on this computer: sieve.example.org:4190 expects an encrypted connection from the start, which ManageSieve doesn't use. On Dovecot, set ssl = no on the sieve listener."
    );
    assert!(!replace);
}
