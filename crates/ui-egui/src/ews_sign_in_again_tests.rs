// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Exchange server account whose server turns the password down (#971): a run the owner asked
//! for (the first one after Add Account among them) asks for the password again at once, saying
//! why; a round by itself asks nothing, and the status bar offers Sign In Again, which asks; and
//! a password typed then is tried. A certificate that needs a say is not a refused sign-in.

use std::sync::{Arc, Mutex, mpsc};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::sync::EwsMailResult;
use serde_json::json;

use crate::{MailApp, Services, dialogs};

const NOW: i64 = 1_791_993_600;
const TYPED: &str = "invented-password";
const AGAIN: &str = "another-invented-password";
const EWS: &str = "https://exchange.example.org/EWS/Exchange.asmx";
const ME: &str = "ada@example.org";
/// What the password dialog says when it asks again.
const WHY: &str = "The server turned down the sign-in. Type the password again.";

/// The pieces of the run under way: the test hands them over one by one, as a server would.
type Pipe = Arc<Mutex<Option<mpsc::Sender<EwsMailResult>>>>;

/// A window with one Exchange server account, whose host's runs the test answers. The passwords
/// each run went with, in order.
fn app() -> (MailApp, u64, Pipe, Arc<Mutex<Vec<String>>>) {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    a.session.driver.set_auto_sync(false);
    let (pipe, sent) = (Pipe::default(), Arc::new(Mutex::new(Vec::new())));
    let (feed, went) = (pipe.clone(), sent.clone());
    a.services.ews = Some(Box::new(move |job| {
        went.lock().unwrap().push(job.secret.clone());
        let (tx, rx) = mpsc::channel();
        *feed.lock().unwrap() = Some(tx);
        rx
    }));
    let added = a.execute("account.add", json!({"email": ME, "kind": "exchange", "ews": {"url": EWS, "username": "CORP\\ada"}}));
    let id = added.unwrap()["id"].as_u64().unwrap();
    (a, id, pipe, sent)
}

fn piece(account: u64) -> EwsMailResult {
    EwsMailResult { account, email: ME.into(), url: EWS.into(), ..Default::default() }
}

/// The run under way ends with `last`.
fn end(a: &mut MailApp, pipe: &Pipe, last: EwsMailResult) {
    pipe.lock().unwrap().as_ref().unwrap().send(EwsMailResult { partial: false, ..last }).unwrap();
    a.poll_sync();
    assert!(!a.syncing());
}

fn refused(account: u64) -> EwsMailResult {
    EwsMailResult { auth_refused: true, ..piece(account) }
}

fn dialog(a: &MailApp) -> Option<&'static str> {
    a.dialog.as_ref().map(dialogs::Dialog::name)
}

fn asks_for(a: &MailApp, account: u64) -> bool {
    matches!(a.dialog, Some(dialogs::Dialog::Password { account: asked, .. }) if asked == account)
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

fn says(h: &Harness<'static, MailApp>, text: &str) -> bool {
    h.query_all_by_label_contains(text).next().is_some()
}

#[test]
fn a_run_the_owner_asked_for_that_is_turned_down_asks_for_the_password_again() {
    let (mut a, id, pipe, sent) = app();
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap()["started"], true);
    end(&mut a, &pipe, refused(id));
    assert!(a.session.sync_status.starts_with("The server turned down the sign-in for"), "{}", a.session.sync_status);
    // Asked again at once, and why: the password that was turned down is gone.
    assert!(asks_for(&a, id), "{:?}", dialog(&a));
    assert!(a.ews_turned_down(id));
    assert_eq!(a.ews_to_sign_in(), Some((id, ME.to_string())));
    // The password typed in it goes with the next run, and the account is no longer turned down.
    a.dialog = None;
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": AGAIN})).unwrap()["started"], true);
    assert!(!a.ews_turned_down(id));
    assert_eq!(a.ews_to_sign_in(), None);
    end(&mut a, &pipe, EwsMailResult { whole: true, ..piece(id) });
    assert_eq!(*sent.lock().unwrap(), vec![TYPED.to_string(), AGAIN.to_string()]);
    assert!(a.dialog.is_none());
    assert_eq!(a.ews_to_sign_in(), None);
}

#[test]
fn the_password_dialog_says_why_it_asks_again_and_not_the_first_time() {
    let (mut a, id, pipe, _) = app();
    // The first time: no password yet, and nothing was turned down.
    assert_eq!(a.execute("ui.sync", json!({"account": id})).unwrap()["pending"], "password");
    let mut h = window(a);
    assert!(says(&h, "Password for ada@example.org"));
    assert!(!says(&h, WHY));
    // Turned down: the dialog that comes up says so, and that the user name may be the cause.
    h.state_mut().dialog = None;
    assert_eq!(h.state_mut().execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap()["started"], true);
    end(h.state_mut(), &pipe, refused(id));
    h.run_steps(3);
    assert!(asks_for(h.state(), id));
    assert!(says(&h, WHY));
    assert!(says(&h, "check the user name in the account's settings"));
}

#[test]
fn a_round_by_itself_that_is_turned_down_asks_nothing_and_the_status_bar_offers_to_sign_in_again() {
    let (mut a, id, pipe, sent) = app();
    // A password remembered from before, and a round that started by itself.
    a.adopt_password(id, TYPED.into());
    assert_eq!(a.execute("ui.sync", json!({"account": id, "quiet": true})).unwrap()["started"], true);
    end(&mut a, &pipe, refused(id));
    assert!(a.dialog.is_none(), "nobody asked for the run: {:?}", dialog(&a));
    assert!(a.ews_turned_down(id));
    let mut h = window(a);
    assert!(h.state().dialog.is_none(), "nor does the next frame ask");
    // The status bar's Sign In Again asks for the password, as the account's Send/Receive does.
    let link = h.state().hit.rect("signInAgain", id).expect("Sign In Again in the status bar");
    click_at(&mut h, link.center());
    assert!(asks_for(h.state(), id), "{:?}", dialog(h.state()));
    assert!(says(&h, WHY));
    // Closed without a password: the link stays, and nothing was sent again.
    h.state_mut().dialog = None;
    h.run_steps(2);
    assert!(h.state().hit.rect("signInAgain", id).is_some());
    assert_eq!(sent.lock().unwrap().len(), 1);
    // A password typed: the run goes, and the link is gone while it runs and after it went through.
    assert_eq!(h.state_mut().execute("ui.sync", json!({"account": id, "password": AGAIN})).unwrap()["started"], true);
    h.run_steps(2);
    assert!(h.state().hit.rect("signInAgain", id).is_none());
    end(h.state_mut(), &pipe, EwsMailResult { whole: true, ..piece(id) });
    h.run_steps(2);
    assert!(h.state().hit.rect("signInAgain", id).is_none());
    assert_eq!(sent.lock().unwrap().last().map(String::as_str), Some(AGAIN));
}

#[test]
fn the_password_is_asked_for_once_add_account_is_closed() {
    // As after Add Account: its last page is still open when the first run is turned down.
    let (mut a, id, pipe, _) = app();
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap()["started"], true);
    a.dialog = dialogs::Dialog::open("account", &a, &json!({}));
    end(&mut a, &pipe, refused(id));
    // It is not put aside for the password.
    assert_eq!(dialog(&a), Some("account"));
    assert_eq!(a.ews_to_sign_in(), Some((id, ME.to_string())));
    let mut h = window(a);
    assert_eq!(dialog(h.state()), Some("account"));
    // Closed (Done): the password is asked for, once.
    h.state_mut().dialog = None;
    h.run_steps(3);
    assert!(asks_for(h.state(), id), "{:?}", dialog(h.state()));
    assert!(says(&h, WHY));
    h.state_mut().dialog = None;
    h.run_steps(3);
    assert!(h.state().dialog.is_none(), "asked once: Sign In Again is the way after that");
    assert!(h.state().hit.rect("signInAgain", id).is_some());
}

#[test]
fn a_password_typed_meanwhile_is_not_asked_for_again() {
    let (mut a, id, pipe, _) = app();
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap()["started"], true);
    a.dialog = dialogs::Dialog::open("about", &a, &json!({}));
    end(&mut a, &pipe, refused(id));
    // A password given another way (Account Settings, say) before the question was closed.
    a.adopt_password(id, AGAIN.into());
    a.dialog = None;
    assert!(!a.ask_ews_again());
    assert!(a.dialog.is_none());
}

#[test]
fn a_certificate_that_needs_a_say_is_not_a_refused_sign_in() {
    let (mut a, id, pipe, _) = app();
    assert_eq!(a.execute("ui.sync", json!({"account": id, "password": TYPED})).unwrap()["started"], true);
    end(&mut a, &pipe, EwsMailResult { untrusted: "ab".repeat(32), ..piece(id) });
    assert!(a.dialog.is_none());
    assert!(!a.ews_turned_down(id));
    assert_eq!(a.ews_to_sign_in(), None);
}

#[test]
fn an_imap_account_is_never_offered_the_exchange_sign_in_again() {
    let (mut a, _, _, _) = app();
    let imap =
        a.execute("account.add", json!({"email": "bo@imap.example", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    a.jmap_refused.insert(imap);
    assert!(!a.ews_turned_down(imap));
    assert_eq!(a.ews_to_sign_in(), None);
}
