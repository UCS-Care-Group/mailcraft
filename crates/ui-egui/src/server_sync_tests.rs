// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The one question for everything a Microsoft account keeps in step with Outlook (#661): asked
//! once for the to-dos and the categories together, one page of Microsoft's for both, not put
//! again by itself once it went away, and one line in File › Info. Hosts are fakes; every address
//! is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::remote;
use mailcraft_model::Id;
use mailcraft_model::outlook_colors::OUTLOOK_COLORS;
use mailcraft_model::remote::CategoriesDownload;
use mailcraft_model::sync::Tokens;
use serde_json::json;

use crate::category_sync::{CategoriesJob, CategoriesResult};
use crate::dialogs::Dialog;
use crate::task_sync::{Permission, TasksJob, TasksResult};
use crate::{MailApp, Services, SignInRequest};

const OWNER: &str = "noor@example.org";
const NOW: i64 = 1_791_993_600;

type SignIns = Arc<Mutex<Vec<(Id, bool)>>>;
type Answer = Arc<Mutex<Result<Tokens, String>>>;

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

fn download(owner: Id) -> CategoriesDownload {
    let red = mailcraft_model::Category { name: "Red category".into(), color: OUTLOOK_COLORS[0] };
    CategoriesDownload { account: owner, email: OWNER.into(), categories: vec![red], complete: true }
}

/// A window with one Microsoft account whose sign-in covers neither its to-dos nor changing its
/// categories, until Microsoft's page gives `answer`.
fn window() -> (MailApp, Id, SignIns, Answer) {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    remote::apply_categories(&mut s, &download(owner)).unwrap();
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.tokens.insert(owner, tokens("own-access", "ref-1"));
    let tasks: Arc<Mutex<VecDeque<TasksResult>>> = Default::default();
    tasks.lock().unwrap().push_back(TasksResult { account: owner, consent_needed: true, ..Default::default() });
    app.services.graph_tasks = Some(Box::new(move |_: TasksJob| {
        let (tx, rx) = mpsc::channel();
        tx.send(tasks.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        rx
    }));
    app.services.graph_categories = Some(Box::new(move |_: CategoriesJob| {
        let (tx, rx) = mpsc::channel();
        tx.send(CategoriesResult { account: owner, covered: Some(false), ..Default::default() }).unwrap();
        rx
    }));
    let (sign_ins, answer): (SignIns, Answer) = (Default::default(), Arc::new(Mutex::new(Ok(tokens("sync-access", "ref-2")))));
    let (log, given) = (sign_ins.clone(), answer.clone());
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.account, req.sync));
        let (tx, rx) = mpsc::channel();
        tx.send(given.lock().unwrap().clone()).unwrap();
        rx
    }));
    (app, owner, sign_ins, answer)
}

/// Both jobs find their sign-in doesn't cover them: one question is up.
fn both_need_it(app: &mut MailApp, owner: Id) {
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    // The to-dos' question is up; the categories' job runs meanwhile and joins it.
    app.tasks_tick_at(1.0);
    assert!(app.dialog.is_some());
    assert!(app.categories_tick_at(2.0));
    app.poll_categories();
    app.categories_tick_at(3.0);
    app.tasks_tick_at(3.0);
    assert_eq!(app.tasks.perm.get(&owner), Some(&Permission::Asking));
    assert!(matches!(app.category_sync.perm.get(&owner), Some(Permission::Needed | Permission::Asking)));
}

fn questions(app: &MailApp) -> usize {
    usize::from(matches!(&app.dialog, Some(Dialog::Confirm { command, .. }) if command == "ui.allowSync"))
}

#[test]
fn one_question_and_one_page_for_the_to_dos_and_the_categories() {
    let (mut app, owner, sign_ins, _) = window();
    both_need_it(&mut app, owner);
    assert_eq!(questions(&app), 1);
    let (line, _, _) = app.sync_line(owner).unwrap();
    assert!(line.contains("asking for your permission"), "{line}");
    // Continue: one page, for both.
    let Some(Dialog::Confirm { command, params, ask }) = app.dialog.take() else { panic!("asked") };
    assert_eq!(ask.action, "Continue");
    let with = crate::dialogs::answered(&params, &ask.with, None);
    app.run(&command, with).unwrap();
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, true)]);
    assert_eq!(app.category_sync.perm.get(&owner), Some(&Permission::Asking));
    app.poll_sign_in();
    // Allowed: both look again at once, and nobody is asked again.
    assert_eq!(app.tasks.perm.get(&owner), None);
    assert_eq!(app.category_sync.perm.get(&owner), None);
    assert_eq!(app.tokens.get(&owner).map(|t| t.refresh.as_str()), Some("ref-2"));
    app.tasks_tick_at(4.0);
    assert!(app.dialog.is_none());
}

#[test]
fn a_question_that_goes_away_is_a_no_for_all_of_it_and_file_info_asks_again() {
    let (mut app, owner, sign_ins, _) = window();
    both_need_it(&mut app, owner);
    app.dialog = None;
    app.tasks_tick_at(4.0);
    app.categories_tick_at(4.0);
    assert_eq!(app.tasks.perm.get(&owner), Some(&Permission::Declined));
    assert_eq!(app.category_sync.perm.get(&owner), Some(&Permission::Declined));
    // Not put again by itself, by either of them.
    for minute in 1..30 {
        app.tasks_tick_at(f64::from(minute) * 60_000.0);
        app.categories_tick_at(f64::from(minute) * 60_000.0);
        assert!(app.dialog.is_none(), "asked again by itself at minute {minute}");
    }
    // One line in File › Info, off, with Ask again; what each stands at is in its tip.
    let (line, details, again) = app.sync_line(owner).unwrap();
    assert!(line.contains("off") && again, "{line}");
    assert!(details.contains("Tasks") && details.contains("Categories"), "{details}");
    // Ask again: the question, then the page.
    assert!(app.ask_sync_again(owner));
    assert_eq!(questions(&app), 1);
    assert!(!app.ask_sync_again(owner), "not twice over");
    app.dialog = None;
    app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).unwrap();
    assert_eq!(sign_ins.lock().unwrap().len(), 1);
}

#[test]
fn a_page_that_says_no_is_said_once_and_leaves_ask_again() {
    let (mut app, owner, sign_ins, answer) = window();
    *answer.lock().unwrap() = Err("the owner closed the page".into());
    both_need_it(&mut app, owner);
    app.dialog = None;
    app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    assert!(matches!(app.tasks.perm.get(&owner), Some(Permission::Refused(_))));
    assert!(matches!(app.category_sync.perm.get(&owner), Some(Permission::Refused(_))));
    for minute in 1..30 {
        app.tasks_tick_at(f64::from(minute) * 60_000.0);
        app.categories_tick_at(f64::from(minute) * 60_000.0);
        assert!(app.dialog.is_none());
    }
    assert_eq!(sign_ins.lock().unwrap().len(), 1);
    let (line, _, again) = app.sync_line(owner).unwrap();
    assert!(line.contains("off") && again, "{line}");
}

#[test]
fn the_question_is_the_owners_alone() {
    let (mut app, owner, sign_ins, _) = window();
    both_need_it(&mut app, owner);
    app.dialog = None;
    // Not meant without a yes; nothing for an account that is not there, or one without a
    // Microsoft sign-in.
    assert!(app.run("ui.allowSync", json!({"account": owner})).is_err());
    assert!(app.run("ui.allowSync", json!({"account": 9_999, "confirmed": true})).is_err());
    assert!(app.run("ui.allowSync", json!({"account": 0, "confirmed": true})).is_err());
    assert!(sign_ins.lock().unwrap().is_empty());
    // An agent asks the owner first, as for adding an account.
    use mailcraft_engine::agent::{Access, Class, access};
    assert_eq!(access("ui.allowSync"), Some(Access::Owner(Class::Accounts)));
}

#[test]
fn a_browser_that_cant_be_started_is_said_for_all_four() {
    let (mut app, owner, _, answer) = window();
    *answer.lock().unwrap() = Err(crate::approval::NO_BROWSER.into());
    both_need_it(&mut app, owner);
    app.dialog = None;
    app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    let (said, _) = app.status_msg.clone().unwrap();
    for part in ["tasks and rules", "automatic replies", "categories", crate::approval::NO_BROWSER] {
        assert!(said.contains(part), "{part}: {said}");
    }
    assert_eq!(app.category_sync.perm.get(&owner), Some(&Permission::Declined));
}

/// A Microsoft account still on IMAP keeps its to-dos and its automatic replies in step with
/// Outlook, never its rules or its categories: the question, File › Info's tip and the status bar
/// say only those.
#[test]
fn a_microsoft_account_still_on_imap_is_asked_only_for_its_to_dos_and_automatic_replies() {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let added = json!({"email": OWNER, "auth": "oauth2", "provider": "microsoft", "kind": "imap"});
    let owner = s.run("account.add", &added).unwrap()["id"].as_u64().unwrap();
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.tokens.insert(owner, tokens("own-access", "ref-1"));
    app.services.graph_tasks = Some(Box::new(move |_: TasksJob| {
        let (tx, rx) = mpsc::channel();
        tx.send(TasksResult { account: owner, consent_needed: true, ..Default::default() }).unwrap();
        rx
    }));
    app.services.sign_in = Some(Box::new(|_: SignInRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(Err(crate::approval::NO_BROWSER.to_string())).unwrap();
        rx
    }));
    assert!(app.tasks_tick_at(0.0));
    app.poll_tasks();
    app.tasks_tick_at(1.0);
    let Some(Dialog::Confirm { command, ask, .. }) = &app.dialog else { panic!("asked") };
    assert_eq!(command, "ui.allowSync");
    let tip = app.sync_tip(owner);
    for said in [ask.text.as_str(), tip] {
        assert!(said.contains("tasks and automatic replies"), "{said}");
        assert!(!said.contains("rules") && !said.contains("categories"), "{said}");
    }
    app.dialog = None;
    app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    let (said, _) = app.status_msg.clone().unwrap();
    assert!(said.contains("tasks stay on this computer") && said.contains(crate::approval::NO_BROWSER), "{said}");
    assert!(!said.contains("rules") && !said.contains("categories"), "{said}");
}

/// An owner whose to-dos are kept in step already (their permission given by itself, as an older
/// version asked it) is asked only for the rest: neither the question, File › Info's tip nor the
/// status bar says that the to-dos stay on this computer.
#[test]
fn to_dos_kept_in_step_already_are_left_out_of_the_question() {
    let (mut app, owner, _, answer) = window();
    *answer.lock().unwrap() = Err(crate::approval::NO_BROWSER.into());
    app.tasks.perm.insert(owner, Permission::Given);
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    app.categories_tick_at(1.0);
    let Some(Dialog::Confirm { command, ask, .. }) = &app.dialog else { panic!("asked") };
    assert_eq!(command, "ui.allowSync");
    let tip = app.sync_tip(owner);
    for said in [ask.text.as_str(), tip] {
        assert!(said.contains("rules, automatic replies and categories"), "{said}");
        assert!(!said.contains("tasks") && !said.contains("To Do"), "{said}");
    }
    app.dialog = None;
    app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    let (said, _) = app.status_msg.clone().unwrap();
    assert!(said.contains("rules stay on this computer") && said.contains(crate::approval::NO_BROWSER), "{said}");
    assert!(!said.contains("tasks"), "{said}");
    assert_eq!(app.tasks.perm.get(&owner), Some(&Permission::Given));
}

/// An organisation whose administrator has to approve first: File › Info's tip and the Categories
/// window say the reason whole, with where to go next, not cut before it.
#[test]
fn the_reason_an_administrator_has_to_approve_is_said_whole() {
    let (mut app, owner, _, answer) = window();
    *answer.lock().unwrap() = Err(crate::favorites_sync::NEEDS_APPROVAL.into());
    both_need_it(&mut app, owner);
    app.dialog = None;
    app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    let (_, details, _) = app.sync_line(owner).unwrap();
    for part in ["Tasks:", "Categories:"] {
        let line = details.lines().find(|l| l.starts_with(part)).unwrap_or_default();
        assert!(line.contains("once: File › Info"), "{part} {details}");
    }
    let (said, _) = app.categories_read_only_line(owner).unwrap();
    assert!(said.contains("once: File › Info"), "{said}");
}

/// File › Info's one line says on or off only when every part is: a list not yet known to be
/// changeable (offline, say) is read-only and says so, and parts that differ are partly on.
#[test]
fn the_line_is_partly_on_when_the_parts_differ() {
    let (mut app, owner, _, _) = window();
    let line = |app: &MailApp| app.sync_line(owner).unwrap();
    let (said, details, _) = line(&app);
    assert_eq!(said, "Sync with Outlook: partly on", "{details}");
    assert!(details.contains("Categories: read-only here until"), "{details}");
    app.category_sync.perm.insert(owner, Permission::Refused("x".into()));
    assert_eq!(line(&app).0, "Sync with Outlook: partly on");
    app.tasks.perm.insert(owner, Permission::Refused("x".into()));
    assert_eq!(line(&app).0, "Sync with Outlook: off");
    app.tasks.perm.remove(&owner);
    app.category_sync.perm.insert(owner, Permission::Given);
    assert_eq!(line(&app).0, "Sync with Outlook: on");
}
