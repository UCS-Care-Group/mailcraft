// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's list of categories kept in step with its server in the window (#661): a
//! job once a session and a moment after a change here, the list read-only and saying so while
//! the sign-in doesn't cover changing it, and the one question put by itself. Hosts are fakes;
//! every address is invented.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::remote;
use mailcraft_model::outlook_colors::OUTLOOK_COLORS;
use mailcraft_model::remote::{CategoriesDownload, CategoriesUploaded, CategoryChangeKind, CategoryOutcome, CategoryResult};
use mailcraft_model::sync::Tokens;
use mailcraft_model::{Category, Id};
use serde_json::json;

use crate::category_sync::{CategoriesJob, CategoriesResult};
use crate::task_sync::Permission;
use crate::{MailApp, Services, SignInRequest};

const OWNER: &str = "noor@example.org";
const NOW: i64 = 1_791_993_600;

/// Each job: the account, the refresh token it was given and how many changes it carried.
type Jobs = Arc<Mutex<Vec<(Id, String, usize)>>>;
type Answers = Arc<Mutex<VecDeque<CategoriesResult>>>;
type SignIns = Arc<Mutex<Vec<(Id, bool)>>>;

fn tokens(access: &str, refresh: &str) -> Tokens {
    Tokens { access: access.into(), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

fn download(owner: Id, names: &[&str]) -> CategoriesDownload {
    let categories = names.iter().map(|n| Category { name: (*n).into(), color: OUTLOOK_COLORS[0] }).collect();
    CategoriesDownload { account: owner, email: OWNER.into(), categories, complete: true }
}

/// A window with one Microsoft account whose list was read from its server once.
fn window() -> (MailApp, Id, Answers, Jobs, SignIns) {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    remote::apply_categories(&mut s, &download(owner, &["Red category"])).unwrap();
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.tokens.insert(owner, tokens("own-access", "ref-1"));
    let (answers, jobs, sign_ins): (Answers, Jobs, SignIns) = Default::default();
    let (to_answer, log) = (answers.clone(), jobs.clone());
    app.services.graph_categories = Some(Box::new(move |job: CategoriesJob| {
        log.lock().unwrap().push((job.account.id, job.refresh.clone(), job.upload.changes.len()));
        let (tx, rx) = mpsc::channel();
        tx.send(to_answer.lock().unwrap().pop_front().unwrap_or_default()).unwrap();
        rx
    }));
    let log = sign_ins.clone();
    app.services.sign_in = Some(Box::new(move |req: SignInRequest| {
        log.lock().unwrap().push((req.account, req.sync));
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(tokens("sync-access", "ref-2"))).unwrap();
        rx
    }));
    (app, owner, answers, jobs, sign_ins)
}

fn covered(owner: Id, names: &[&str]) -> CategoriesResult {
    CategoriesResult { account: owner, covered: Some(true), download: Some(download(owner, names)), ..Default::default() }
}

/// The host's answer when the sign-in doesn't cover changing the list: that, and nothing else
/// (the calendar's job reads the list meanwhile).
fn not_covered(owner: Id) -> CategoriesResult {
    CategoriesResult { account: owner, covered: Some(false), ..Default::default() }
}

#[test]
fn the_first_look_learns_the_list_may_be_changed_and_a_change_goes_after_a_quiet_moment() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category", "Made in Outlook"]));
    // Checking, before the job has said.
    let (line, _) = app.categories_read_only_line(owner).unwrap();
    assert!(line.contains("Checking whether MailCraft may change them"), "{line}");
    assert!(app.categories_tick_at(0.0));
    // One at a time.
    assert!(!app.categories_tick_at(1.0));
    app.poll_categories();
    assert_eq!(app.category_sync.perm.get(&owner), Some(&Permission::Given));
    assert!(app.categories_read_only_line(owner).is_none(), "the list may be changed: nothing to say");
    assert!(app.session.mailbox.category_of(owner, "Made in Outlook").is_some(), "Outlook's own change shows");
    assert_eq!(jobs.lock().unwrap()[0], (owner, "ref-1".to_string(), 0));
    // Once a session: not again without a change.
    assert!(!app.categories_tick_at(60.0 * 60_000.0));

    // A new one goes, a moment after the last change.
    let green = OUTLOOK_COLORS[4].hex();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": green})).unwrap();
    let at = 60.0 * 60_000.0;
    assert!(!app.categories_tick_at(at + 1.0));
    assert!(!app.categories_tick_at(at + 1_000.0));
    let done = CategoriesUploaded {
        account: owner,
        outcomes: vec![CategoryOutcome {
            kind: CategoryChangeKind::Create,
            name: "Travel".into(),
            color: OUTLOOK_COLORS[4],
            result: CategoryResult::Done,
            error: String::new(),
            found: false,
        }],
        ..Default::default()
    };
    let mut answer = covered(owner, &["Red category", "Made in Outlook", "Travel"]);
    answer.uploaded = Some(done);
    answers.lock().unwrap().push_back(answer);
    assert!(app.categories_tick_at(at + 5_000.0));
    assert_eq!(jobs.lock().unwrap()[1], (owner, "ref-1".to_string(), 1));
    app.poll_categories();
    // Told: nothing waits any more.
    assert_eq!(mailcraft_engine::cmd::categories_remote::categories_waiting(&app.session, owner), 0);
    assert!(!app.categories_tick_at(at + 60_000.0));
}

#[test]
fn without_leave_to_change_it_the_list_is_read_only_and_the_owner_is_asked_once() {
    let (mut app, owner, answers, jobs, sign_ins) = window();
    answers.lock().unwrap().push_back(not_covered(owner));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    assert_eq!(app.category_sync.perm.get(&owner), Some(&Permission::Needed));
    // Read-only, whatever the window or an agent tries.
    let green = OUTLOOK_COLORS[4].hex();
    assert!(app.execute("category.set", json!({"account": owner, "name": "Travel", "color": green})).is_err());
    assert!(app.session.mailbox.category_of(owner, "Travel").is_none());
    // The window asks by itself, the one question.
    app.categories_tick_at(1.0);
    let Some(crate::dialogs::Dialog::Confirm { command, params, ask }) = &app.dialog else { panic!("the owner is asked") };
    assert_eq!((command.as_str(), params["account"].as_u64()), ("ui.allowSync", Some(owner)));
    assert!(ask.text.contains(OWNER) && ask.text.contains("categories"), "{}", ask.text);
    assert!(ask.cancel.is_none(), "one button");
    let (line, allow) = app.categories_read_only_line(owner).unwrap();
    assert!(line.contains("Asking for your permission") && !allow, "{line}");
    assert!(sign_ins.lock().unwrap().is_empty(), "no page before Continue");

    // Gone unanswered: read-only, said with Allow, and not asked again by itself.
    app.dialog = None;
    app.categories_tick_at(2.0);
    assert_eq!(app.category_sync.perm.get(&owner), Some(&Permission::Declined));
    let (line, allow) = app.categories_read_only_line(owner).unwrap();
    assert!(line.contains("No permission to change them") && allow, "{line}");
    for minute in 1..30 {
        assert!(!app.categories_tick_at(f64::from(minute) * 60_000.0));
        assert!(app.dialog.is_none(), "asked again by itself");
    }
    assert_eq!(jobs.lock().unwrap().len(), 1);

    // Allow: the question again, then Microsoft's page, and the next look runs at once.
    assert!(app.ask_sync_again(owner));
    app.dialog = None;
    app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).unwrap();
    assert_eq!(*sign_ins.lock().unwrap(), [(owner, true)]);
    app.poll_sign_in();
    assert_eq!(app.category_sync.perm.get(&owner), None);
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(31.0 * 60_000.0));
    assert_eq!(jobs.lock().unwrap()[1].1, "ref-2", "with the new sign-in");
    app.poll_categories();
    assert_eq!(app.category_sync.perm.get(&owner), Some(&Permission::Given));
    assert!(app.execute("category.set", json!({"account": owner, "name": "Travel", "color": green})).is_ok());
}

#[test]
fn a_server_that_wont_let_the_list_be_changed_says_why_and_asks_nobody() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": OUTLOOK_COLORS[4].hex()})).unwrap();
    let at = 60_000.0;
    assert!(!app.categories_tick_at(at));
    // The server answered 403 to the change, as the host gives it: the change denied, and the
    // list read back after it.
    let denied = CategoryOutcome {
        kind: CategoryChangeKind::Create,
        name: "Travel".into(),
        color: OUTLOOK_COLORS[4],
        result: CategoryResult::Denied,
        error: "this account may not change its list of categories".into(),
        found: false,
    };
    let mut answer = covered(owner, &["Red category"]);
    answer.denied = true;
    answer.uploaded = Some(CategoriesUploaded { account: owner, outcomes: vec![denied], ..Default::default() });
    answers.lock().unwrap().push_back(answer);
    assert!(app.categories_tick_at(at + 5_000.0));
    app.poll_categories();
    assert!(matches!(app.category_sync.perm.get(&owner), Some(Permission::Refused(_))));
    assert!(!app.session.mailbox.category_writes.contains(&owner));
    let (line, allow) = app.categories_read_only_line(owner).unwrap();
    assert!(line.contains("No permission to change them (") && allow, "{line}");
    for minute in 2..30 {
        assert!(!app.categories_tick_at(at + f64::from(minute) * 60_000.0));
    }
    assert!(app.dialog.is_none());
    assert_eq!(jobs.lock().unwrap().len(), 2);
}

#[test]
fn a_refusal_just_after_the_owner_allowed_it_is_tried_again_before_asking() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(not_covered(owner));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    app.categories_tick_at(1.0);
    app.dialog = None;
    app.run("ui.allowSync", json!({"account": owner, "confirmed": true})).unwrap();
    app.poll_sign_in();
    // Refused twice more, then allowed.
    answers.lock().unwrap().extend([not_covered(owner), not_covered(owner), covered(owner, &["Red category"])]);
    let mut at = 2.0;
    for _ in 0..3 {
        while !app.categories_tick_at(at) {
            assert!(app.dialog.is_none(), "asked again {at} ms after allowing");
            at += 1_000.0;
            assert!(at < 10.0 * 60_000.0, "no job was tried again");
        }
        app.poll_categories();
        app.categories_tick_at(at);
        assert!(app.dialog.is_none(), "asked again after a refusal right after allowing");
    }
    assert_eq!(app.category_sync.perm.get(&owner), Some(&Permission::Given));
    assert_eq!(jobs.lock().unwrap().len(), 4);
}

#[test]
fn a_sign_in_microsoft_no_longer_takes_waits_for_a_new_one() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(CategoriesResult { account: owner, sign_in_refused: true, ..Default::default() });
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    assert!(matches!(app.category_sync.perm.get(&owner), Some(Permission::SignInOver(_))));
    let (line, _) = app.categories_read_only_line(owner).unwrap();
    assert!(line.contains("no longer takes the sign-in"), "{line}");
    assert!(!app.categories_tick_at(60_000.0));
    // A new sign-in for mail: the list goes on.
    app.tokens.insert(owner, tokens("new-access", "ref-3"));
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(120_000.0));
    assert_eq!(jobs.lock().unwrap()[1].1, "ref-3");
}

#[test]
fn a_job_that_ends_without_an_answer_is_tried_again_later() {
    let (mut app, owner, _, jobs, _) = window();
    app.services.graph_categories = Some(Box::new({
        let log = jobs.clone();
        move |job: CategoriesJob| {
            log.lock().unwrap().push((job.account.id, job.refresh.clone(), 0));
            mpsc::channel().1
        }
    }));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    assert!(!app.category_sync.busy());
    assert_eq!(app.category_sync.perm.get(&owner), None, "nothing learnt");
    assert!(!app.categories_tick_at(1_000.0), "not at once");
    assert!(app.categories_tick_at(31_000.0));
}

#[test]
fn the_window_takes_in_the_jobs_answer_by_itself() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category", "Made in Outlook"]));
    app.auto_sync = true;
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
    let app = h.state();
    assert_eq!(jobs.lock().unwrap().len(), 1);
    assert!(!app.category_sync.busy(), "the answer was taken in");
    assert!(app.session.mailbox.category_writes.contains(&owner));
    assert!(app.session.mailbox.category_of(owner, "Made in Outlook").is_some());
}

#[test]
fn an_answer_for_a_mailbox_closed_since_is_let_go_of() {
    let (mut app, owner, answers, jobs, _) = window();
    // The mailbox that takes its place has the same account, by the same id.
    let other = app.session.mailbox.clone();
    answers.lock().unwrap().push_back(covered(owner, &["Red category", "Made in Outlook"]));
    assert!(app.categories_tick_at(0.0));
    assert_eq!(jobs.lock().unwrap().len(), 1);
    app.session.replace_mailbox(other);
    app.poll_categories();
    assert!(!app.category_sync.busy());
    assert!(!app.session.mailbox.category_writes.contains(&owner), "the other mailbox's server is not this one's");
    assert!(app.session.mailbox.category_of(owner, "Made in Outlook").is_none());
    // A job started for the mailbox open now is that one's.
    answers.lock().unwrap().push_back(covered(owner, &["Red category", "Made in Outlook"]));
    assert!(app.categories_tick_at(60_000.0));
    app.poll_categories();
    assert!(app.session.mailbox.category_writes.contains(&owner));
    assert!(app.session.mailbox.category_of(owner, "Made in Outlook").is_some());
}

#[test]
fn what_was_known_of_the_categories_goes_with_the_mailbox_it_was_known_of() {
    let (mut app, owner, _, _, _) = window();
    app.category_sync.perm.insert(owner, Permission::Declined);
    let other = app.session.mailbox.clone();
    app.session.replace_mailbox(other);
    app.drop_secrets_if_replaced();
    assert_eq!(app.category_sync.perm.get(&owner), None);
}

#[test]
fn an_answer_for_another_address_of_the_account_is_let_go_of() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category", "Made in Outlook"]));
    assert!(app.categories_tick_at(0.0));
    assert_eq!(jobs.lock().unwrap().len(), 1);
    app.session.run("account.edit", &json!({"id": owner, "email": "someone.else@example.org"})).unwrap();
    app.poll_categories();
    assert!(!app.category_sync.busy());
    assert!(!app.session.mailbox.category_writes.contains(&owner));
    assert!(app.session.mailbox.category_of(owner, "Made in Outlook").is_none());
}

#[test]
fn the_categories_jobs_go_in_the_owners_order_of_the_accounts() {
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    let first = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let second = s.run("account.add", &json!({"email": "sam@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    remote::apply_categories(&mut s, &download(first, &["Red category"])).unwrap();
    let theirs = CategoriesDownload { email: "sam@example.org".into(), ..download(second, &["Red category"]) };
    remote::apply_categories(&mut s, &theirs).unwrap();
    assert_eq!(crate::category_sync::category_accounts(&s.mailbox), [first, second]);
    s.run("account.move", &json!({"id": second, "before": first})).unwrap();
    assert_eq!(crate::category_sync::category_accounts(&s.mailbox), [second, first]);
}

#[test]
fn an_undo_while_the_job_is_on_its_way_never_deletes_what_it_found_in_outlook() {
    let (mut app, owner, answers, _, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    let green = OUTLOOK_COLORS[4].hex();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": green})).unwrap();
    let at = 60.0 * 60_000.0;
    assert!(!app.categories_tick_at(at + 1.0));
    // Outlook had a Travel of its own: the job finds it there.
    let found = CategoriesUploaded {
        account: owner,
        outcomes: vec![CategoryOutcome {
            kind: CategoryChangeKind::Create,
            name: "Travel".into(),
            color: OUTLOOK_COLORS[4],
            result: CategoryResult::Done,
            error: String::new(),
            found: true,
        }],
        ..Default::default()
    };
    let mut answer = covered(owner, &["Red category", "Travel"]);
    answer.uploaded = Some(found);
    answers.lock().unwrap().push_back(answer);
    assert!(app.categories_tick_at(at + 5_000.0));
    // The owner presses Undo while the job is on its way.
    app.execute("edit.undo", json!({})).unwrap();
    app.poll_categories();
    let upload = mailcraft_engine::cmd::categories_remote::category_upload(&app.session, owner).unwrap();
    let deletes: Vec<&str> = upload.changes.iter().filter(|c| c.kind == CategoryChangeKind::Delete).map(|c| c.name.as_str()).collect();
    assert!(deletes.is_empty(), "Outlook's own to be deleted: {deletes:?}");
}

#[test]
fn a_token_refused_while_a_change_goes_is_tried_again_after_a_wait() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    let green = OUTLOOK_COLORS[4].hex();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": green})).unwrap();
    let at = 60_000.0;
    assert!(!app.categories_tick_at(at));
    // The renewal worked, then the server refused the token for the change.
    let refused = CategoriesResult {
        account: owner,
        tokens: Some(tokens("cat-access", "ref-1")),
        covered: Some(true),
        sign_in_refused: true,
        uploaded: Some(CategoriesUploaded { account: owner, token_refused: true, ..Default::default() }),
        ..Default::default()
    };
    answers.lock().unwrap().push_back(refused);
    assert!(app.categories_tick_at(at + 5_000.0));
    app.poll_categories();
    assert_eq!(mailcraft_engine::cmd::categories_remote::categories_waiting(&app.session, owner), 1, "still to go");
    // Not again a moment later, but after the wait.
    assert!(!app.categories_tick_at(at + 9_000.0));
    assert!(!app.categories_tick_at(at + 20_000.0));
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(at + 5_000.0 + 31_000.0));
    assert_eq!(jobs.lock().unwrap().len(), 3);
}

#[test]
fn the_one_question_put_or_turned_down_goes_with_the_mailbox_it_was_for() {
    let (mut app, owner, _, _, _) = window();
    app.sync_asking.insert(owner);
    app.sync_declined.insert(owner);
    let other = app.session.mailbox.clone();
    app.session.replace_mailbox(other);
    app.drop_secrets_if_replaced();
    assert!(app.sync_asking.is_empty() && app.sync_declined.is_empty());
}

#[test]
fn a_change_the_server_was_too_busy_for_is_tried_again_after_a_wait() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    let green = OUTLOOK_COLORS[4].hex();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": green})).unwrap();
    let at = 60_000.0;
    assert!(!app.categories_tick_at(at));
    // Microsoft Graph said "busy" (429) for the change.
    let busy = CategoryOutcome {
        kind: CategoryChangeKind::Create,
        name: "Travel".into(),
        color: OUTLOOK_COLORS[4],
        result: CategoryResult::Failed,
        error: "Microsoft Graph is busy or the category changed there meanwhile".into(),
        found: false,
    };
    let mut answer = covered(owner, &["Red category"]);
    answer.uploaded = Some(CategoriesUploaded { account: owner, outcomes: vec![busy], ..Default::default() });
    answers.lock().unwrap().push_back(answer);
    assert!(app.categories_tick_at(at + 5_000.0));
    app.poll_categories();
    assert_eq!(mailcraft_engine::cmd::categories_remote::categories_waiting(&app.session, owner), 1, "still to go");
    assert!(!app.categories_tick_at(at + 9_000.0), "not again a moment later");
    assert!(!app.categories_tick_at(at + 20_000.0));
    assert!(app.categories_tick_at(at + 5_000.0 + 31_000.0), "after the wait");
    assert_eq!(jobs.lock().unwrap().len(), 3);
}

/// A job that takes long to come to nothing waits its whole wait from when it ended, not from
/// when it began.
#[test]
fn a_change_whose_answer_comes_late_waits_from_the_answer() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": OUTLOOK_COLORS[4].hex()})).unwrap();
    let busy = CategoryOutcome {
        kind: CategoryChangeKind::Create,
        name: "Travel".into(),
        color: OUTLOOK_COLORS[4],
        result: CategoryResult::Failed,
        error: "Microsoft Graph is busy".into(),
        found: false,
    };
    let mut answer = covered(owner, &["Red category"]);
    answer.uploaded = Some(CategoriesUploaded { account: owner, outcomes: vec![busy], ..Default::default() });
    answers.lock().unwrap().push_back(answer);
    let at = 60_000.0;
    assert!(!app.categories_tick_at(at));
    assert!(app.categories_tick_at(at + 5_000.0));
    // The answer comes 40 seconds later.
    assert!(!app.categories_tick_at(at + 45_000.0));
    app.poll_categories();
    assert!(!app.categories_tick_at(at + 46_000.0), "not at once");
    assert!(!app.categories_tick_at(at + 74_000.0));
    assert!(app.categories_tick_at(at + 76_000.0), "after the wait");
    assert_eq!(jobs.lock().unwrap().len(), 3);
}

/// An answer read after the window went a long time without a frame waits from when it was
/// read, not from the last look before the gap (#661).
#[test]
fn an_answer_read_after_a_long_gap_waits_from_when_it_was_read() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": OUTLOOK_COLORS[4].hex()})).unwrap();
    let busy = CategoryOutcome {
        kind: CategoryChangeKind::Create,
        name: "Travel".into(),
        color: OUTLOOK_COLORS[4],
        result: CategoryResult::Failed,
        error: "Microsoft Graph is busy".into(),
        found: false,
    };
    let mut answer = covered(owner, &["Red category"]);
    answer.uploaded = Some(CategoriesUploaded { account: owner, outcomes: vec![busy], ..Default::default() });
    answers.lock().unwrap().push_back(answer);
    let at = 60_000.0;
    assert!(!app.categories_tick_at(at));
    assert!(app.categories_tick_at(at + 5_000.0));
    // No frame for 40 seconds; then the answer is read before any look.
    app.take_in_sync_answers(at + 45_000.0);
    assert_eq!([app.calendar.clock, app.tasks.clock, app.rules.clock], [at + 45_000.0; 3]);
    assert!(!app.categories_tick_at(at + 46_000.0), "not at once");
    assert!(!app.categories_tick_at(at + 74_000.0));
    assert!(app.categories_tick_at(at + 76_000.0), "after the wait");
    assert_eq!(jobs.lock().unwrap().len(), 3);
}

#[test]
fn another_address_is_looked_at_afresh_and_its_list_may_be_changed_without_asking_again() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    assert!(app.session.mailbox.categories_read_only(owner).is_none());
    // The account is given another address: the new mailbox's list is read-only until a job says.
    app.execute("account.edit", json!({"id": owner, "email": "noor.new@example.org"})).unwrap();
    let mut read = download(owner, &["Red category", "Theirs"]);
    read.email = "noor.new@example.org".into();
    remote::apply_categories(&mut app.session, &read).unwrap();
    assert!(app.session.mailbox.categories_read_only(owner).is_some());
    // The next look asks the server again, at once, and the list may be changed once it says so.
    let mut answer = covered(owner, &["Red category", "Theirs"]);
    answer.download.as_mut().unwrap().email = "noor.new@example.org".into();
    answers.lock().unwrap().push_back(answer);
    // (The new address signs in for itself.)
    assert!(!app.tokens.contains_key(&owner));
    app.tokens.insert(owner, tokens("new-access", "ref-3"));
    assert!(app.categories_tick_at(1_000.0), "looked at afresh");
    app.poll_categories();
    assert_eq!(jobs.lock().unwrap().len(), 2);
    assert!(app.session.mailbox.categories_read_only(owner).is_none(), "not read-only until Allow");
}

#[test]
fn what_waits_is_worked_out_again_at_most_once_a_second_while_the_mailbox_changes() {
    let (mut app, owner, answers, _, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    app.categories_tick_at(10.0);
    let worked_out = |app: &MailApp| app.category_sync.ready.as_ref().map(|(_, when, _)| *when);
    assert_eq!(worked_out(&app), Some(10.0));
    // The mailbox changes at every frame: the count waits for a second.
    for frame in 1..50 {
        app.execute("mail.markAllRead", json!({})).ok();
        app.session.mailbox_changed();
        app.categories_tick_at(10.0 + f64::from(frame) * 16.0);
    }
    assert_eq!(worked_out(&app), Some(10.0));
    app.categories_tick_at(1_010.0);
    assert_eq!(worked_out(&app), Some(1_010.0));
}

/// `command` sent by an agent that was allowed nothing in advance: its answer when it has one,
/// and where the answer comes once the owner answers its question.
fn agent(app: &mut MailApp, command: &str, params: serde_json::Value) -> (Option<serde_json::Value>, mpsc::Receiver<serde_json::Value>) {
    use crate::control::{ControlRequest, Outcome};
    let (req, rx) = ControlRequest::new("engine.execute", json!({"command": command, "params": params}));
    let out = crate::control::handle(app, &egui::Context::default(), &req.from_agent(Vec::new()));
    (if let Outcome::Done(v) = out { Some(v) } else { None }, rx)
}

#[test]
fn an_agents_change_to_outlooks_list_runs_on_the_owners_yes() {
    // A list kept in step that MailCraft may change, with one category kept on this computer only.
    let mut s = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    s.mailbox.categories = vec![Category { name: "Old local".into(), color: OUTLOOK_COLORS[3] }];
    let owner = s.run("account.add", &json!({"email": OWNER, "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    s.run("sync.categoryWrites", &json!({"account": owner, "allowed": true})).unwrap();
    remote::apply_categories(&mut s, &download(owner, &["Red category", "Blue category"])).unwrap();
    assert!(s.mailbox.category_is_local(owner, "Old local"));
    let mut app = MailApp::new(s, Services::default());
    app.autosave = false;
    app.auto_sync = false;
    for (command, params) in [
        ("category.addToOutlook", json!({"account": owner, "name": "Old local"})),
        ("category.rename", json!({"account": owner, "from": "Red category", "to": "Crimson"})),
        ("category.remove", json!({"account": owner, "name": "Blue category"})),
    ] {
        let (now, rx) = agent(&mut app, command, params);
        assert!(now.is_none() && !app.asking.is_empty(), "{command} is asked about: {now:?}");
        crate::agent_gate::answer(&mut app, true);
        let r = rx.try_recv().unwrap();
        assert_eq!(r["ok"], true, "{command}: {r}");
    }
    let mb = &app.session.mailbox;
    assert!(!mb.category_is_local(owner, "Old local"), "sent to Outlook");
    assert!(mb.category_of(owner, "Crimson").is_some() && mb.category_of(owner, "Red category").is_none());
    assert!(mb.category_of(owner, "Blue category").is_none());
}

/// A name Outlook had already (made on another device meanwhile) takes Outlook's colour, and the
/// status bar says so: it is no Send/Receive problem, and nothing waits longer for it.
#[test]
fn a_name_outlook_had_already_is_said_in_the_status_bar_and_is_no_problem() {
    let (mut app, owner, answers, _, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": OUTLOOK_COLORS[4].hex()})).unwrap();
    let at = 60_000.0;
    assert!(!app.categories_tick_at(at));
    let found = CategoryOutcome {
        kind: CategoryChangeKind::Create,
        name: "Travel".into(),
        color: OUTLOOK_COLORS[7],
        result: CategoryResult::Done,
        error: String::new(),
        found: true,
    };
    let mut answer = covered(owner, &["Red category", "Travel"]);
    answer.uploaded = Some(CategoriesUploaded { account: owner, outcomes: vec![found], ..Default::default() });
    answers.lock().unwrap().push_back(answer);
    assert!(app.categories_tick_at(at + 5_000.0));
    app.poll_categories();
    let (said, _) = app.status_msg.clone().unwrap();
    assert!(said.contains("\"Travel\" was in Outlook already"), "{said}");
    assert!(!app.sync_problems.iter().any(|(a, ..)| *a == owner), "no problem");
    assert!(!app.category_sync.failures.contains_key(&owner), "no wait");
}

#[test]
fn offline_nothing_is_worked_out_and_nothing_goes() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    let green = OUTLOOK_COLORS[4].hex();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": green})).unwrap();
    app.session.prefs.offline = true;
    app.category_sync.ready = None;
    for at in [10.0, 5_000.0, 60_000.0] {
        assert!(!app.categories_tick_at(at));
    }
    assert!(app.category_sync.ready.is_none(), "what waits isn't looked through while offline");
    assert_eq!(jobs.lock().unwrap().len(), 1);
    // Back online, the change goes after its quiet moment.
    app.session.prefs.offline = false;
    assert!(!app.categories_tick_at(61_000.0));
    assert!(app.categories_tick_at(70_000.0));
    assert_eq!(jobs.lock().unwrap()[1].2, 1);
}

#[test]
fn a_sign_in_refused_just_after_the_owner_allowed_it_waits_before_it_is_tried_again() {
    let (mut app, owner, answers, jobs, _) = window();
    answers.lock().unwrap().push_back(covered(owner, &["Red category"]));
    assert!(app.categories_tick_at(0.0));
    app.poll_categories();
    // A change waits; the owner has just allowed it (Ask again), and every job finds the renewed
    // sign-in refused.
    let green = OUTLOOK_COLORS[4].hex();
    app.execute("category.set", json!({"account": owner, "name": "Travel", "color": green})).unwrap();
    app.categories_allowed(owner);
    let refused = || CategoriesResult { account: owner, sign_in_refused: true, ..Default::default() };
    answers.lock().unwrap().extend([refused(), refused(), refused(), refused(), refused()]);
    let before = jobs.lock().unwrap().len();
    // Ten seconds of frames: a job, then a wait, not a job each time one ends.
    let mut at = 5_000.0;
    while at < 15_000.0 {
        if app.categories_tick_at(at) {
            app.poll_categories();
        }
        at += 16.0;
    }
    let tried = jobs.lock().unwrap().len() - before;
    assert!(tried <= 2, "{tried} jobs in ten seconds");
}
