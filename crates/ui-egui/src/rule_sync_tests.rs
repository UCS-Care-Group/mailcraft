// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Rules kept on a Microsoft account's server, in the window (#566): the server's rules read when
//! they are due, a rule that waits sent only with the owner's permission, which is asked for once
//! and by itself, and what the host brings taken in. Hosts are fakes; every address is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use egui_kittest::kittest::{By, Queryable as _};
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
    assert!(!again && line == "Rules: the server's are shown", "{line}");
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

/// A held Space or Enter on a rule's Delete button deletes that rule alone: its repeats don't
/// reach the button that takes its place, the next rule's.
#[test]
fn a_held_key_deletes_one_rule() {
    for held in [egui::Key::Space, egui::Key::Enter] {
        let mut app = MailApp::new(Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0), Services::default());
        app.autosave = false;
        for name in ["One", "Two", "Three"] {
            let rule = json!({"name": name, "conditions": [{"type": "subjectContains", "value": "x"}], "actions": [{"type": "markRead"}]});
            app.execute("rules.add", rule).unwrap();
        }
        app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
        let mut h = egui_kittest::Harness::builder()
            .with_accessibility_check(false)
            .allow_missing_glyphs()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_ui_state(
                |ui, app: &mut MailApp| {
                    let ctx = ui.ctx().clone();
                    app.logic(&ctx);
                    app.ui(ui);
                },
                app,
            );
        h.run_steps(6);
        // The dialog's ×, then the first rule's switch, up, down and Delete.
        for _ in 0..5 {
            h.key_press(egui::Key::Tab);
            h.run_steps(2);
        }
        let key = |pressed, repeat| egui::Event::Key { key: held, physical_key: None, pressed, repeat, modifiers: egui::Modifiers::NONE };
        h.event(key(true, false));
        h.step();
        for _ in 0..6 {
            h.event(key(true, true));
            h.step();
        }
        h.event(key(false, false));
        h.run_steps(3);
        let names: Vec<&str> = h.state().session.mailbox.rules.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Two", "Three"], "{held:?}");
    }
}

fn server_rule(id: &str, name: &str, sequence: i32) -> RemoteRule {
    RemoteRule {
        id: id.into(),
        name: name.into(),
        sequence,
        enabled: true,
        conditions: vec![Condition::SubjectContains(name.into())],
        actions: vec![Action::MarkRead],
        ..Default::default()
    }
}

fn read_whole(owner: Id, rules: Vec<RemoteRule>) -> RulesResult {
    RulesResult { account: owner, download: Some(RulesDownload { account: owner, email: OWNER.into(), complete: true, rules }), ..Default::default() }
}

/// The whole window, laid out a few frames.
fn shown(app: MailApp) -> egui_kittest::Harness<'static, MailApp> {
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    h.run_steps(6);
    h
}

#[test]
fn an_answer_for_a_mailbox_closed_since_is_let_go_of() {
    let (mut app, owner, answers, jobs, _) = window();
    // The mailbox that takes its place has the same account, by the same id.
    let other = app.session.mailbox.clone();
    answers.lock().unwrap().push_back(read_whole(owner, vec![server_rule("S1", "From the shop", 1)]));
    assert!(app.rules_tick_at(0.0));
    assert_eq!(*jobs.lock().unwrap(), [(owner, 0)]);
    app.session.replace_mailbox(other);
    app.poll_rules();
    assert!(app.session.mailbox.rules.is_empty(), "the other mailbox's server is not this one's");
    assert!(!app.rules.busy());
}

#[test]
fn a_rule_whose_answer_was_lost_is_not_made_twice() {
    let (mut app, owner, answers, jobs, _) = window();
    let rule = add_rule(&mut app, owner);
    // The first job ends without a word: the rule may be on the server now.
    let log = jobs.clone();
    let to_answer = answers.clone();
    let lost = Arc::new(Mutex::new(true));
    app.services.graph_rules = Some(Box::new(move |job: RulesJob| {
        log.lock().unwrap().push((job.account.id, job.upload.ops.len()));
        let (tx, rx) = mpsc::channel();
        if !std::mem::replace(&mut *lost.lock().unwrap(), false) {
            tx.send(to_answer.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        }
        rx
    }));
    assert!(app.rules_tick_at(0.0));
    app.poll_rules();
    assert!(app.session.mailbox.rules.iter().any(|r| r.id == rule && r.unanswered));
    // Not sent again: the next job only reads, and the server's rules, read whole, haven't it.
    answers.lock().unwrap().push_back(read_whole(owner, Vec::new()));
    assert!(app.rules_tick_at(6.0 * 60_000.0));
    assert_eq!(*jobs.lock().unwrap(), [(owner, 1), (owner, 0)]);
    app.poll_rules();
    assert!(app.session.mailbox.rules.iter().any(|r| r.id == rule && !r.unanswered && r.waiting()));
    // Then it goes again.
    assert!(app.rules_tick_at(13.0 * 60_000.0));
    assert_eq!(jobs.lock().unwrap().last(), Some(&(owner, 1)));
}

#[test]
fn a_rule_that_went_nowhere_goes_with_the_next_job() {
    let (mut app, owner, answers, jobs, _) = window();
    let rule = add_rule(&mut app, owner);
    // The sign-in doesn't cover changing the rules: nothing is sent, and nothing is marked sent.
    answers.lock().unwrap().push_back(RulesResult { account: owner, consent_needed: true, ..Default::default() });
    assert!(app.rules_tick_at(0.0));
    app.poll_rules();
    assert!(app.session.mailbox.rules.iter().any(|r| r.id == rule && !r.unanswered));
    // And a second job while the first is out sends nothing more: one job at a time.
    app.rules.perm.remove(&owner);
    assert!(app.rules_tick_at(60_000.0));
    assert!(!app.rules_tick_at(61_000.0));
    assert_eq!(*jobs.lock().unwrap(), [(owner, 1), (owner, 1)]);
}

#[test]
fn changes_held_back_from_the_server_say_the_server_still_has_the_rule_as_it_was() {
    let (mut app, owner, answers, _, _) = window();
    answers.lock().unwrap().push_back(read_whole(owner, vec![server_rule("S1", "Shop", 1), server_rule("S2", "Club", 2)]));
    assert!(app.rules_tick_at(0.0));
    app.poll_rules();
    let id = |app: &MailApp, name: &str| app.session.mailbox.rules.iter().find(|r| r.name == name).map(|r| r.id).unwrap();
    let (shop, club) = (id(&app, "Shop"), id(&app, "Club"));
    app.rules.perm.insert(owner, Permission::Declined);
    app.execute("rules.enable", json!({"id": shop, "value": false})).unwrap();
    app.execute("rules.remove", json!({"id": club})).unwrap();
    let r = app.session.mailbox.rules.iter().find(|r| r.id == shop).unwrap().clone();
    let mark = crate::forms::rules::mark(&app, &r, owner);
    assert!(mark.starts_with("On the server, still as it was") && mark.contains("Ask again"), "{mark}");
    // Allowed, the change is the rule's own again.
    app.rules.perm.remove(&owner);
    assert_eq!(crate::forms::rules::mark(&app, &r, owner), "On the server");
    // The rule deleted here is said in the Rules window while it is still on the server.
    app.rules.perm.insert(owner, Permission::Declined);
    app.execute("ui.dialog", json!({"name": "rules"})).unwrap();
    app.auto_sync = false;
    let h = shown(app);
    let said = "1 rule deleted here is still on the server until MailCraft may change the server's rules (File › Info › Ask again).";
    assert!(h.query_by_label(said).is_some());
}

#[test]
fn the_new_rule_is_not_filled_in_from_a_shared_mailboxs_message_or_an_id_that_is_none() {
    use mailcraft_model::sync::{FolderListing, GraphFolder, GraphMailResult};
    use mailcraft_model::{Address, FolderKind, Message};
    let (mut app, _, _, _, _) = window();
    app.auto_sync = false;
    let shared = app.execute("account.addSharedMailbox", json!({"address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("t-in", "Inbox", FolderKind::Inbox), folder("t-ar", "Archive", FolderKind::Archive)];
    let listing = GraphMailResult {
        account: shared,
        email: "team@example.org".into(),
        listing: Some(FolderListing { folders, complete: true }),
        ..Default::default()
    };
    mailcraft_engine::cmd::graph_mail::apply_graph_mail(&mut app.session, listing).unwrap();
    let mb = &app.session.mailbox;
    let (inbox, archive) = (mb.folder_of_kind(shared, FolderKind::Inbox).unwrap(), mb.folder_of_kind(shared, FolderKind::Archive).unwrap());
    let m =
        app.session.mailbox.add_message(Message { folder: inbox, from: Address::new("Club", "news@club.example"), ..Default::default() }).unwrap();
    for bad in [json!("x"), json!(-1), json!(1.5), json!([m])] {
        assert!(app.execute("ui.dialog", json!({"name": "rules", "folder": archive, "id": bad})).is_err(), "{bad}");
        assert!(app.execute("ui.dialog", json!({"name": "rules", "folder": bad, "id": m})).is_err(), "{bad}");
        assert!(app.dialog.is_none());
    }
    app.execute("ui.dialog", json!({"name": "rules", "folder": archive, "id": m})).unwrap();
    let h = shown(app);
    assert!(h.query(By::new().label_contains("Then run it over")).is_none(), "nothing is filled in");
    assert!(h.state().session.mailbox.rules.is_empty());
}
