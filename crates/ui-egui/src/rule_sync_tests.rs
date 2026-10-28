// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules kept on a Microsoft account's server, in the window (#566): the server's rules read when
//! they are due, a rule that waits sent only with the owner's permission, which is asked for once
//! and by itself, and what the host brings taken in. Hosts are fakes; every address is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::remote_rules::{RemoteRule, RulesDownload};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Action, Condition, Id};
use serde_json::json;

use crate::rule_sync::{Permission, RulesJob, RulesResult};
use crate::{MailApp, Services, SignInRequest};

const OWNER: &str = "noor@example.org";
const NOW: i64 = 1_791_993_600;

type Jobs = Arc<Mutex<Vec<(Id, usize)>>>;
type Answers = Arc<Mutex<VecDeque<RulesResult>>>;
type SignIns = Arc<Mutex<Vec<(Id, bool, bool)>>>;

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

fn window() -> (MailApp, Id, Answers, Jobs, SignIns) {
    let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
    app.autosave = false;
    let owner = app.execute("account.add", json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    app.tokens.insert(owner, tokens("own-access", "ref-1"));
    let (answers, jobs, sign_ins): (Answers, Jobs, SignIns) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.graph_rules = Some(Box::new(move |job: RulesJob| {
        log.lock().unwrap().push((job.account.id, job.upload.ops.len()));
        let (tx, rx) = mpsc::channel();
        tx.send(to_answer.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        rx
    }));
    let log = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.account, req.rules, req.tasks));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(tokens("rules-access", "ref-2"))).unwrap();
        rx
    }));
    (app, owner, answers, jobs, sign_ins)
}

fn add_rule(app: &mut MailApp, owner: Id) -> Id {
    let made = app
        .execute(
            "rules.add",
            json!({"name": "Receipts", "account": owner, "conditions": [{"type": "subjectContains", "value": "receipt"}], "actions": [{"type": "markRead"}]}),
        )
        .unwrap();
    assert_eq!(made["keep"], "server");
    made["id"].as_u64().unwrap()
}

#[test]
fn the_servers_rules_are_read_when_due_and_shown() {
    let (mut app, owner, answers, jobs, _) = window();
    let made_there = RemoteRule {
        id: "S1".into(),
        name: "From the shop".into(),
        sequence: 1,
        enabled: true,
        conditions: vec![Condition::FromContains("shop@example.org".into())],
        actions: vec![Action::MarkRead],
        ..Default::default()
    };
    let download = RulesDownload { account: owner, email: OWNER.into(), complete: true, rules: vec![made_there] };
    answers.lock().unwrap().push_back(RulesResult { account: owner, download: Some(download), ..Default::default() });
    assert!(app.rules_tick_at(0.0));
    // Nothing waits: the job only reads.
    assert_eq!(*jobs.lock().unwrap(), [(owner, 0)]);
    app.poll_rules();
    let rule = app.session.mailbox.rules.iter().find(|r| r.name == "From the shop").unwrap();
    assert!(rule.on_server() && rule.account == owner);
    assert_eq!(crate::forms::rules::mark(&app, rule, owner), "On the server");
    let (line, again) = app.rules_line(owner).unwrap();
    assert!(!again && line.contains("you're asked before"), "{line}");
    // Not again before it is due.
    assert!(!app.rules_tick_at(60_000.0));
    assert!(app.rules_tick_at(6.0 * 60_000.0));
}

#[test]
fn a_rule_waits_here_until_the_owner_allows_changing_the_rules_on_the_server() {
    let (mut app, owner, answers, jobs, sign_ins) = window();
    let rule = add_rule(&mut app, owner);
    // The first look sends it, with a token that may change the rules: there is none.
    answers.lock().unwrap().push_back(RulesResult { account: owner, consent_needed: true, ..Default::default() });
    // (The first look is due at once, and takes what waits with it.)
    assert!(app.rules_tick_at(0.0));
    assert_eq!(*jobs.lock().unwrap(), [(owner, 1)]);
    app.poll_rules();
    assert_eq!(app.rules.perm.get(&owner), Some(&Permission::Needed));
    // It runs here meanwhile, and says why it is not on the server.
    let r = app.session.mailbox.rules.iter().find(|r| r.id == rule).unwrap().clone();
    assert!(r.waiting() && !r.on_server());
    let mark = crate::forms::rules::mark(&app, &r, owner);
    assert!(mark.contains("Only on this computer for now") && mark.contains("permission"), "{mark}");
    // The window asks first; no page yet.
    assert!(!app.rules_tick_at(6_000.0));
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = &app.dialog else { panic!("the owner is asked first") };
    assert_eq!((command.as_str(), params["account"].as_u64()), ("ui.allowRules", Some(owner)));
    assert!(ask.text.contains(OWNER) && ask.text.contains("stay on this computer"), "{}", ask.text);
    assert_eq!(ask.cancel.as_deref(), Some("Not now"));
    assert!(sign_ins.lock().unwrap().is_empty());
    // Not now: the server's rules are still read, as due, and nothing is sent.
    app.dialog = None;
    assert!(!app.rules_tick_at(7_000.0));
    assert_eq!(app.rules.perm.get(&owner), Some(&Permission::Declined));
    let (line, again) = app.rules_line(owner).unwrap();
    assert!(again && line.contains("not allowed on the server yet"), "{line}");
    assert!(!app.rules_tick_at(60_000.0), "what waits for a permission makes no job");
    assert!(app.rules_tick_at(6.0 * 60_000.0));
    app.poll_rules();
    assert_eq!(jobs.lock().unwrap().last(), Some(&(owner, 0)));
    // Ask again (File › Info): the question, then Microsoft's page for this alone.
    app.ask_rules_again(owner);
    assert!(app.run("ui.allowRules", json!({"account": owner})).is_err());
    assert!(app.run("ui.allowRules", json!({"account": 9_999, "confirmed": true})).is_err());
    app.dialog = None;
    assert_eq!(app.run("ui.allowRules", json!({"account": owner, "confirmed": true})).unwrap()["account"], owner);
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, true, false)]);
    app.poll_sign_in();
    assert_eq!(app.rules.perm.get(&owner), None);
    // Allowed: the next look sends it.
    assert!(app.rules_tick_at(7.0 * 60_000.0));
    assert_eq!(jobs.lock().unwrap().last(), Some(&(owner, 1)));
}

#[test]
fn a_server_that_refuses_changes_is_said_and_the_rules_stay_here() {
    let (mut app, owner, answers, _, _) = window();
    add_rule(&mut app, owner);
    answers.lock().unwrap().push_back(RulesResult { account: owner, denied: true, ..Default::default() });
    assert!(app.rules_tick_at(0.0));
    app.poll_rules();
    assert!(matches!(app.rules.perm.get(&owner), Some(Permission::Refused(_))));
    let (line, again) = app.rules_line(owner).unwrap();
    assert!(again && line.contains("not allowed on the server"), "{line}");
    // Rules of an account whose server keeps none have no line.
    let local = app.execute("account.add", json!({"email": "me@example.org", "kind": "local"})).unwrap()["id"].as_u64().unwrap();
    assert!(app.rules_line(local).is_none());
}

#[test]
fn a_sign_in_for_another_purpose_never_asks_for_the_rules() {
    let (mut app, owner, _, _, sign_ins) = window();
    let oauth = app.session.mailbox.account(owner).unwrap().oauth.clone().unwrap();
    app.begin_sign_in(owner, oauth, OWNER, String::new(), false, true).unwrap();
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, false, false)]);
}
