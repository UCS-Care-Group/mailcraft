// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account that has to be signed in again (#1556): an IMAP or SMTP password turned down, a
//! Google or Microsoft token that no longer works, a JMAP or Exchange server's refusal, a shared
//! mailbox's owner whose sign-in is refused all set the one mark, which Account Problems at the
//! top right and the folder pane show until a sign-in goes through. Nothing pops up by itself,
//! and a restart still says it. The hosts are fakes the tests answer; every address and secret
//! is invented.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::Id;
use mailcraft_model::sync::{EwsFolderDeltas, EwsMailResult, GraphMailResult, JmapMailResult, SyncRequest, SyncResult, SyncStage, SyncStep, Tokens};
use serde_json::json;

use crate::{GraphRequest, GraphResult, MailApp, Services, approval, dialogs};

const NOW: i64 = 1_791_993_600;
const TYPED: &str = "invented-password";
const REFUSED: &str = "Couldn't check for new mail: sign-in failed: [AUTHENTICATIONFAILED] invalid credentials";
const SMTP_REFUSED: &str = "Couldn't send \"Hello\": sign-in failed: 535 5.7.8 authentication failed";
const EWS: &str = "https://exchange.example.org/EWS/Exchange.asmx";
const JMAP: &str = "https://jmap.example.org/jmap/session";

fn app() -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    a
}

/// What the IMAP host answers each run with, a run at a time: its pieces, the last one ending it.
type Runs = Arc<Mutex<VecDeque<Vec<SyncResult>>>>;

fn imap_host(a: &mut MailApp) -> Runs {
    let runs = Runs::default();
    let answers = runs.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        for piece in answers.lock().unwrap().pop_front().unwrap_or_default() {
            tx.send(SyncResult { account: req.account.id, ..piece }).unwrap();
        }
        rx
    }));
    runs
}

/// A run turned down with `error`, and one the server lets in.
fn turned_down(error: &str) -> Vec<SyncResult> {
    vec![SyncResult { errors: vec![error.to_string()], ..Default::default() }]
}

fn let_in() -> Vec<SyncResult> {
    vec![SyncResult { partial: true, step: Some(SyncStep::new(SyncStage::Listed, "Inbox", 0, 0)), ..Default::default() }, SyncResult::default()]
}

fn settle(a: &mut MailApp) {
    for _ in 0..12 {
        a.poll_sync();
        a.poll_graph();
        a.poll_sign_in();
    }
}

fn needed(a: &MailApp, id: Id) -> bool {
    a.session.mailbox.account(id).is_some_and(|x| x.sign_in_needed)
}

fn asks_password(a: &MailApp, id: Id) -> bool {
    matches!(a.dialog, Some(dialogs::Dialog::Password { account, .. }) if account == id)
}

fn tokens(refresh: &str) -> Tokens {
    Tokens { access: format!("access-{refresh}"), refresh: refresh.into(), expires_at: mailcraft_time::now() + 3_600, ..Default::default() }
}

/// A sign-in host that answers each sign-in with what comes next, and notes whether each went
/// to the page (no refresh token) and whether it was quiet.
type SignIns = (Arc<Mutex<VecDeque<Result<Tokens, String>>>>, Arc<Mutex<Vec<(bool, bool)>>>);

fn sign_in_host(a: &mut MailApp) -> SignIns {
    let (answers, asked): SignIns = Default::default();
    let (next, note) = (answers.clone(), asked.clone());
    a.services.sign_in = Some(Box::new(move |req| {
        note.lock().unwrap().push((req.refresh.is_empty(), req.quiet));
        let (tx, rx) = mpsc::channel();
        tx.send(next.lock().unwrap().pop_front().unwrap_or_else(|| Err("nothing to answer".into()))).unwrap();
        rx
    }));
    (answers, asked)
}

#[test]
fn an_imap_or_smtp_password_turned_down_marks_the_account_until_a_run_signs_in() {
    for error in [REFUSED, SMTP_REFUSED] {
        let mut a = app();
        let runs = imap_host(&mut a);
        let id = a
            .execute(
                "account.add",
                json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}}),
            )
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        runs.lock().unwrap().push_back(turned_down(error));
        a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
        settle(&mut a);
        assert!(needed(&a, id), "{error}");
        assert_eq!(a.to_sign_in(), vec![(id, "avery@mail.example".to_string())]);
        assert_eq!(
            a.execute("account.list", json!({})).unwrap()["accounts"].as_array().unwrap().iter().find(|x| x["id"] == json!(id)).unwrap()["signInNeeded"],
            true
        );
        // Nothing came up by itself; a round by itself turned down again changes nothing.
        assert!(a.dialog.is_none());
        runs.lock().unwrap().push_back(turned_down(error));
        a.passwords.insert(id, "another".into());
        a.execute("ui.sync", json!({"account": id, "quiet": true})).unwrap();
        settle(&mut a);
        assert!(needed(&a, id) && a.dialog.is_none());
        // Sign in again is the account's Send/Receive: it asks for the password, and a run the
        // server lets in takes the mark away.
        a.execute("ui.sync", json!({"account": id})).unwrap();
        assert!(asks_password(&a, id));
        a.dialog = None;
        runs.lock().unwrap().push_back(let_in());
        a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
        settle(&mut a);
        assert!(!needed(&a, id) && a.to_sign_in().is_empty());
    }
}

/// A refusal in a piece of the run is said at once, even when the run then stops without its
/// last piece (its thread ended early).
#[test]
fn a_refusal_in_a_run_that_stops_early_still_marks_the_account() {
    let mut a = app();
    let runs = imap_host(&mut a);
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    runs.lock().unwrap().push_back(vec![SyncResult { partial: true, errors: vec![REFUSED.to_string()], ..Default::default() }]);
    a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
    settle(&mut a);
    assert!(!a.syncing());
    assert!(needed(&a, id));
    // And the password turned down is forgotten, as at a run's end: the next try asks again.
    assert!(!a.passwords.contains_key(&id) && !a.sync_refused);
    a.execute("ui.sync", json!({"account": id})).unwrap();
    assert!(asks_password(&a, id));
}

/// A run that was for the address the account had before (it was edited while the run was under
/// way) changes no mark, at once or at its end: the engine takes none of its pieces in either.
#[test]
fn a_run_for_the_address_the_account_had_before_leaves_the_mark_as_it_is() {
    let mut a = app();
    let runs = imap_host(&mut a);
    let id =
        a.execute("account.add", json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    let before = |run: Vec<SyncResult>| -> Vec<SyncResult> {
        run.into_iter().map(|piece| SyncResult { email: "before@mail.example".into(), ..piece }).collect()
    };
    runs.lock().unwrap().push_back(before(turned_down(REFUSED)));
    a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
    settle(&mut a);
    assert!(!needed(&a, id), "the old address's refusal doesn't mark this one");
    // Marked by a run of its own, a run the old address's server let in leaves the mark.
    runs.lock().unwrap().push_back(turned_down(REFUSED));
    a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
    settle(&mut a);
    assert!(needed(&a, id));
    runs.lock().unwrap().push_back(before(let_in()));
    a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
    settle(&mut a);
    assert!(needed(&a, id), "the old address's sign-in doesn't take the mark away");
}

fn add_google(a: &mut MailApp) -> Id {
    a.execute(
        "account.add",
        json!({"email": "ada@gmail.example", "auth": "oauth2", "provider": "google", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}}),
    )
    .unwrap()["id"]
        .as_u64()
        .unwrap()
}

/// A Google token revoked: the next Send/Receive is turned down and the account is marked; Sign in
/// again renews it (the refresh token was still good) and the run goes through.
#[test]
fn a_revoked_google_token_marks_the_account_and_sign_in_again_fixes_it() {
    let mut a = app();
    let runs = imap_host(&mut a);
    let (answers, asked) = sign_in_host(&mut a);
    let id = add_google(&mut a);
    a.adopt_tokens(id, Tokens { email: "ada@gmail.example".into(), ..tokens("google-1") });
    runs.lock().unwrap().push_back(turned_down("Couldn't check for new mail: sign-in failed: [AUTHENTICATIONFAILED] Invalid credentials (Failure)"));
    a.execute("ui.sync", json!({"account": id})).unwrap();
    settle(&mut a);
    assert!(needed(&a, id));
    assert!(asked.lock().unwrap().is_empty(), "nothing was asked of Google by itself");
    answers.lock().unwrap().push_back(Ok(Tokens { email: "ada@gmail.example".into(), ..tokens("google-2") }));
    runs.lock().unwrap().push_back(let_in());
    a.execute("ui.sync", json!({"account": id})).unwrap();
    settle(&mut a);
    assert_eq!(*asked.lock().unwrap(), [(false, false)]);
    assert!(!needed(&a, id));
}

/// A round by itself whose quiet renewal Google or Microsoft turns down marks the account and
/// opens nothing; trouble reaching the provider doesn't mark it, nor does a page the owner closed
/// on a first sign-in.
#[test]
fn a_quiet_renewal_turned_down_marks_the_account_and_opens_nothing() {
    let email = "ada@gmail.example";
    let cases: [(String, bool, bool); 6] = [
        (approval::GOOGLE_REAUTH_REQUIRED.to_string(), true, true),
        (approval::GOOGLE_NEEDS_SIGN_IN.to_string(), true, true),
        (approval::sign_in_run_out(email), true, true),
        ("Google sign-in could not reach the authorization server. Retry later.".to_string(), true, false),
        (approval::SIGN_IN_DECLINED.to_string(), false, true),
        (approval::SIGN_IN_TIMED_OUT.to_string(), false, true),
    ];
    for (error, quiet, marks) in cases {
        let mut a = app();
        let (answers, _) = sign_in_host(&mut a);
        let id = add_google(&mut a);
        let oauth = a.session.mailbox.account(id).unwrap().oauth.clone().unwrap();
        answers.lock().unwrap().push_back(Err(error.clone()));
        a.begin_sign_in(id, oauth, email, "refresh-1".into(), quiet, false).unwrap();
        a.poll_sign_in();
        assert_eq!(needed(&a, id), marks, "{error}");
        assert!(a.dialog.is_none(), "{error}");
    }
    // The page closed on a sign-in that had no refresh token to renew says nothing of the account.
    let mut a = app();
    let (answers, _) = sign_in_host(&mut a);
    let id = add_google(&mut a);
    let oauth = a.session.mailbox.account(id).unwrap().oauth.clone().unwrap();
    answers.lock().unwrap().push_back(Err(approval::SIGN_IN_DECLINED.into()));
    a.begin_sign_in(id, oauth, email, String::new(), false, false).unwrap();
    a.poll_sign_in();
    assert!(!needed(&a, id));
}

/// A Microsoft account kept in step through Graph whose token was revoked: a round by itself is
/// turned down and marks the account without opening Microsoft's page; Sign in again opens it
/// (the refresh token is turned down again), and once signed in the mark is gone.
#[test]
fn a_revoked_microsoft_token_marks_the_account_and_sign_in_again_fixes_it() {
    let mut a = MailApp::new(Session::empty(), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    let id = a.execute("account.add", json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    a.tokens.insert(id, tokens("ms-1"));
    let refuse = Arc::new(AtomicBool::new(true));
    let turn_down = refuse.clone();
    a.services.graph = Some(Box::new(move |req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        let (account, email) = (req.account.id, req.account.email.clone());
        if turn_down.load(Ordering::Relaxed) {
            tx.send(GraphResult { account, sign_in_refused: true, ..Default::default() }).unwrap();
        } else {
            if let Some(pieces) = req.mail_pieces.clone() {
                pieces.send(GraphMailResult { account, email, ..Default::default() }).unwrap();
            }
            tx.send(GraphResult { account, ..Default::default() }).unwrap();
        }
        rx
    }));
    a.services.sync = Some(Box::new(|_req: SyncRequest| mpsc::channel().1));
    let (answers, asked) = sign_in_host(&mut a);
    a.execute("ui.sync", json!({"account": id, "quiet": true})).unwrap();
    settle(&mut a);
    assert!(needed(&a, id));
    assert!(asked.lock().unwrap().is_empty() && a.dialog.is_none(), "a round by itself opens no page");
    // Sign in again: turned down once more, so Microsoft's page; signed in there, the mail comes.
    answers.lock().unwrap().push_back(Ok(tokens("ms-2")));
    a.execute("ui.sync", json!({"account": id})).unwrap();
    // (That run was handed its answer as it started; the runs after the sign-in are let in.)
    refuse.store(false, Ordering::Relaxed);
    settle(&mut a);
    settle(&mut a);
    assert_eq!(asked.lock().unwrap().first(), Some(&(true, false)));
    assert!(!needed(&a, id));
}

/// An Exchange server that turns the password down marks the account; the first run it lets in
/// takes the mark away.
#[test]
fn an_exchange_server_that_turns_the_password_down_marks_the_account() {
    let mut a = app();
    let refuse = Arc::new(AtomicBool::new(true));
    let turn_down = refuse.clone();
    a.services.ews = Some(Box::new(move |job| {
        let (tx, rx) = mpsc::channel();
        let (account, email) = (job.request.account, job.request.email.clone());
        let piece = EwsMailResult { account, email, url: EWS.into(), ..Default::default() };
        let piece = if turn_down.load(Ordering::Relaxed) {
            EwsMailResult { auth_refused: true, ..piece }
        } else {
            EwsMailResult { whole: true, folders: Some(EwsFolderDeltas::default()), ..piece }
        };
        tx.send(piece).unwrap();
        rx
    }));
    let id = a.execute("account.add", json!({"email": "cy@example.org", "kind": "exchange", "ews": {"url": EWS}})).unwrap()["id"].as_u64().unwrap();
    a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
    settle(&mut a);
    assert!(needed(&a, id));
    assert_eq!(a.to_sign_in(), vec![(id, "cy@example.org".to_string())]);
    // Sign in again asks for the password; the run it goes with is let in.
    a.execute("ui.sync", json!({"account": id})).unwrap();
    assert!(asks_password(&a, id));
    a.dialog = None;
    refuse.store(false, Ordering::Relaxed);
    a.execute("ui.sync", json!({"account": id, "password": "the-right-one"})).unwrap();
    settle(&mut a);
    assert!(!needed(&a, id));
}

/// A JMAP server that turns the secret down marks the account; a run that ends without being
/// whole (a change the server refused) leaves it, and the first whole run takes it away.
#[test]
fn a_jmap_server_that_turns_the_secret_down_marks_the_account_until_a_whole_run() {
    let mut a = app();
    let answers: Arc<Mutex<VecDeque<JmapMailResult>>> = Default::default();
    let next = answers.clone();
    a.services.jmap = Some(Box::new(move |job: crate::jmap_mail::JmapJob| {
        let (tx, rx) = mpsc::channel();
        let piece = next.lock().unwrap().pop_front().unwrap_or_default();
        tx.send(JmapMailResult { account: job.request.account, email: job.request.email.clone(), ..piece }).unwrap();
        rx
    }));
    let id =
        a.execute("account.add", json!({"email": "ada@example.org", "kind": "jmap", "jmap": {"session": JMAP}})).unwrap()["id"].as_u64().unwrap();
    answers.lock().unwrap().push_back(JmapMailResult { auth_refused: true, ..Default::default() });
    a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
    settle(&mut a);
    assert!(needed(&a, id));
    assert_eq!(a.to_sign_in(), vec![(id, "ada@example.org".to_string())]);
    assert!(a.dialog.is_none());
    // Sign in again asks for the secret; a run it lets in that isn't whole says nothing of it.
    a.execute("ui.sync", json!({"account": id})).unwrap();
    assert!(asks_password(&a, id));
    a.dialog = None;
    answers.lock().unwrap().push_back(JmapMailResult::default());
    a.execute("ui.sync", json!({"account": id, "password": "the-right-one"})).unwrap();
    settle(&mut a);
    assert!(needed(&a, id));
    // A whole run takes the mark away.
    answers.lock().unwrap().push_back(JmapMailResult { whole: true, ..Default::default() });
    a.execute("ui.sync", json!({"account": id, "password": "the-right-one"})).unwrap();
    settle(&mut a);
    assert!(!needed(&a, id) && a.to_sign_in().is_empty());
}

/// A shared mailbox whose run finds its owner's sign-in refused marks the owner's account, not
/// the shared mailbox; Account Problems lists the owner, whose Send/Receive signs in again.
#[test]
fn a_shared_mailbox_whose_owner_s_sign_in_is_refused_marks_the_owner() {
    let mut a = MailApp::new(Session::empty(), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    let owner = a.execute("account.add", json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    a.tokens.insert(owner, tokens("ms-1"));
    let shared = a.execute("account.addSharedMailbox", json!({"account": owner, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    a.services.graph = Some(Box::new(|req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(GraphResult { account: req.account.id, sign_in_refused: true, ..Default::default() }).unwrap();
        rx
    }));
    a.services.sync = Some(Box::new(|_req: SyncRequest| mpsc::channel().1));
    a.execute("ui.sync", json!({"account": shared, "quiet": true})).unwrap();
    settle(&mut a);
    assert!(needed(&a, owner) && !needed(&a, shared));
    assert_eq!(a.to_sign_in().iter().map(|(id, _)| *id).collect::<Vec<_>>(), [owner]);
    assert!(a.dialog.is_none());
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
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

fn click_at(h: &mut Harness<'static, MailApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

/// A window opened on a mailbox whose account has to be signed in again (as after a restart)
/// shows Account Problems and the folder pane's mark, and nothing else comes up; the menu's Sign
/// in again asks for the password; signed in, both are gone.
#[test]
fn account_problems_and_the_folder_pane_say_it_until_a_sign_in_goes_through() {
    let mut a = app();
    let runs = imap_host(&mut a);
    let id = a
        .execute(
            "account.add",
            json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    // Nothing to say for an account that is signed in.
    let mut h = window(a);
    assert!(h.state().hit.rect("accountProblems", 0).is_none() && h.state().hit.rect("accountSignInNeeded", id).is_none());
    mailcraft_engine::cmd::account::note_sign_in_needed(&mut h.state_mut().session, id, true);
    h.run_steps(3);
    let problems = h.state().hit.rect("accountProblems", 0).expect("Account Problems at the top right");
    assert!(problems.min.x > 640.0 && problems.max.y < 60.0, "{problems:?}");
    assert!(h.state().hit.rect("accountSignInNeeded", id).is_some(), "the folder pane's mark");
    assert!(h.query_all_by_label_contains("Account Problems").next().is_some());
    assert!(h.state().dialog.is_none());
    // The menu lists the account with Sign in again, which asks for its password.
    click_at(&mut h, problems.center());
    let again = h.state().hit.rect("accountProblemsSignIn", id).expect("the account's Sign in again");
    click_at(&mut h, again.center());
    assert!(asks_password(h.state(), id), "{:?}", h.state().dialog.as_ref().map(dialogs::Dialog::name));
    // Typed, and let in: nothing is said any more.
    h.state_mut().dialog = None;
    runs.lock().unwrap().push_back(let_in());
    h.state_mut().execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap();
    settle(h.state_mut());
    h.run_steps(3);
    assert!(h.state().hit.rect("accountProblems", 0).is_none() && h.state().hit.rect("accountSignInNeeded", id).is_none());
}

/// The folder pane's mark is Sign in again too.
#[test]
fn the_folder_panes_mark_signs_in_again() {
    let mut a = app();
    let id = a.execute("account.add", json!({"email": "cy@example.org", "kind": "exchange", "ews": {"url": EWS}})).unwrap()["id"].as_u64().unwrap();
    a.services.ews = Some(Box::new(|_job| mpsc::channel().1));
    mailcraft_engine::cmd::account::note_sign_in_needed(&mut a.session, id, true);
    let mut h = window(a);
    let mark = h.state().hit.rect("accountSignInNeeded", id).expect("the folder pane's mark");
    click_at(&mut h, mark.center());
    assert!(asks_password(h.state(), id), "{:?}", h.state().dialog.as_ref().map(dialogs::Dialog::name));
}

/// Signed in again for a shared mailbox, as its owner: the owner's mark goes at once, whatever
/// reading the shared mailbox then comes to.
#[test]
fn an_owner_who_signs_in_again_for_a_shared_mailbox_is_signed_in() {
    let mut a = MailApp::new(Session::empty(), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    let owner = a.execute("account.add", json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    a.tokens.insert(owner, tokens("ms-1"));
    let shared = a.execute("account.addSharedMailbox", json!({"account": owner, "address": "team@example.org"})).unwrap()["id"].as_u64().unwrap();
    a.note_sign_in_needed(owner, true);
    // (Graph won't read it: what the sign-in did is all that is looked at.)
    a.services.graph = Some(Box::new(|req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        tx.send(GraphResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    a.services.sync = Some(Box::new(|_req: SyncRequest| mpsc::channel().1));
    let (answers, _) = sign_in_host(&mut a);
    answers.lock().unwrap().push_back(Ok(tokens("ms-2")));
    a.begin_shared_sign_in(shared).unwrap();
    settle(&mut a);
    assert!(!needed(&a, owner));
}

/// A Microsoft account with the mark, and the one page for an older sign-in, opened from the
/// Tasks (`ui.allowTasks`, as the Rules and Automatic Replies pages open it too), answered with
/// `who`'s tokens.
fn signed_in_on_the_sync_page(who: &str) -> (MailApp, Id) {
    let mut a = MailApp::new(Session::empty(), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    let owner = a.execute("account.add", json!({"email": "noor@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    a.tokens.insert(owner, tokens("ms-1"));
    a.note_sign_in_needed(owner, true);
    // (No run that starts after it ever answers: what the sign-in did is all that is looked at.
    // Each run's sender is kept, unused, as long as the app is, so the run stays unanswered
    // rather than ending.)
    let graph_senders: Arc<Mutex<Vec<mpsc::Sender<GraphResult>>>> = Arc::default();
    a.services.graph = Some(Box::new(move |_req: GraphRequest| {
        let (tx, rx) = mpsc::channel();
        graph_senders.lock().unwrap().push(tx);
        rx
    }));
    let sync_senders: Arc<Mutex<Vec<mpsc::Sender<SyncResult>>>> = Arc::default();
    a.services.sync = Some(Box::new(move |_req: SyncRequest| {
        let (tx, rx) = mpsc::channel();
        sync_senders.lock().unwrap().push(tx);
        rx
    }));
    let (answers, _) = sign_in_host(&mut a);
    answers.lock().unwrap().push_back(Ok(Tokens { email: who.into(), ..tokens("ms-2") }));
    a.run("ui.allowTasks", json!({"account": owner, "confirmed": true})).unwrap();
    settle(&mut a);
    (a, owner)
}

/// Signed in as the account on that page: the mark goes, as for any other sign-in that goes
/// through.
#[test]
fn a_sign_in_on_the_page_the_tasks_rules_and_automatic_replies_open_takes_the_mark_away() {
    let (a, owner) = signed_in_on_the_sync_page("noor@example.org");
    assert!(!needed(&a, owner));
}

/// Signed in there as somebody else: those tokens are nobody's here (#103), so the mark stays.
#[test]
fn somebody_else_signed_in_on_that_page_leaves_the_mark() {
    let (a, owner) = signed_in_on_the_sync_page("someone.else@example.org");
    assert!(needed(&a, owner));
}
