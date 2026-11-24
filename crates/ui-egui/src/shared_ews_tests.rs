// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox opened through an Exchange server account, in the window (#482): it signs in with
//! the owner's password and server, names its own address in the run, and is kept or removed
//! again by what its first read brings. Hosts are fakes; every address is invented.

use std::sync::{Arc, Mutex, mpsc};

use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::sync::{EwsFolderDeltas, EwsMailRequest, EwsMailResult, GraphFolder};
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;
const TYPED: &str = "invented-password";
const OWNER: &str = "ada@example.org";
const SHARED: &str = "support@example.org";
const EWS: &str = "https://exchange.example.org/EWS/Exchange.asmx";

type Sender = Arc<Mutex<Option<mpsc::Sender<EwsMailResult>>>>;
/// The requests and secrets the host was asked to run.
type Jobs = Arc<Mutex<Vec<(EwsMailRequest, String)>>>;

/// A window with an Exchange server account, a shared mailbox opened through it, and a host that
/// records each job and hands over the pieces of the run under way when the test says.
fn window() -> (MailApp, Id, Id, Jobs, Sender) {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let (jobs, feed): (Jobs, Sender) = Default::default();
    let (seen, pipe) = (jobs.clone(), feed.clone());
    a.services.ews = Some(Box::new(move |job| {
        seen.lock().unwrap().push((job.request.clone(), job.secret.clone()));
        let (tx, rx) = mpsc::channel();
        *pipe.lock().unwrap() = Some(tx);
        rx
    }));
    let owner = a.execute("account.add", json!({"email": OWNER, "kind": "exchange", "ews": {"url": EWS, "username": "CORP\\ada"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    let shared = a.execute("account.addSharedMailbox", json!({"account": owner, "address": SHARED})).unwrap()["id"].as_u64().unwrap();
    (a, owner, shared, jobs, feed)
}

fn send(a: &mut MailApp, feed: &Sender, piece: EwsMailResult) {
    feed.lock().unwrap().as_ref().unwrap().send(piece).unwrap();
    a.poll_sync();
}

fn piece(account: Id) -> EwsMailResult {
    EwsMailResult { account, email: SHARED.into(), url: EWS.into(), partial: true, ..Default::default() }
}

fn listed(inbox: &str) -> EwsFolderDeltas {
    EwsFolderDeltas {
        changed: vec![GraphFolder { id: inbox.into(), name: "Inbox".into(), kind: FolderKind::Inbox, ..Default::default() }],
        afresh: true,
        ..Default::default()
    }
}

#[test]
fn a_shared_mailbox_is_read_with_the_owners_password_and_server_and_names_itself_in_the_run() {
    let (mut a, owner, shared, jobs, _) = window();
    a.execute("ui.sync", json!({"account": owner, "password": TYPED})).unwrap();
    // (The owner's run is over, or called off: the shared mailbox is the one asked for.)
    a.services.sync_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    a.sync_rx = None;
    a.sync_run = None;
    jobs.lock().unwrap().clear();
    let started = a.execute("ui.sync", json!({"account": shared})).unwrap();
    assert_eq!(started["started"], true, "{started}");
    assert!(a.dialog.is_none(), "no password is asked for: the owner's is at hand");
    let jobs = jobs.lock().unwrap();
    let (request, secret) = jobs.last().expect("a run was handed to the host");
    assert_eq!(secret, TYPED);
    assert_eq!((request.account, request.email.as_str(), request.mailbox.as_str()), (shared, SHARED, SHARED));
    assert_eq!((request.url.as_str(), request.username.as_str()), (EWS, "CORP\\ada"), "the owner's server, signed in as the owner");
}

#[test]
fn the_password_asked_for_is_the_owners_and_what_is_typed_is_kept_for_the_owner() {
    let (mut a, owner, shared, jobs, _) = window();
    let pending = a.execute("ui.sync", json!({"account": shared})).unwrap();
    assert_eq!(pending["pending"], "password");
    assert!(
        matches!(a.dialog, Some(Dialog::Password { account, .. }) if account == owner),
        "the owner's password is asked for, not the shared mailbox's"
    );
    assert!(jobs.lock().unwrap().is_empty());
    // Typed with the run: it is the owner's.
    a.dialog = None;
    let started = a.execute("ui.sync", json!({"account": shared, "password": TYPED})).unwrap();
    assert_eq!(started["started"], true);
    assert_eq!(a.passwords.get(&owner).map(String::as_str), Some(TYPED));
    assert!(!a.passwords.contains_key(&shared));
}

#[test]
fn a_shared_mailbox_being_added_is_kept_once_its_inbox_is_read() {
    let (mut a, owner, shared, jobs, feed) = window();
    a.passwords.insert(owner, TYPED.into());
    a.begin_shared_check(shared).unwrap();
    assert!(a.shared_checking.contains(&shared));
    assert_eq!(jobs.lock().unwrap().last().map(|(r, _)| r.mailbox.clone()).as_deref(), Some(SHARED));
    send(&mut a, &feed, EwsMailResult { folders: Some(listed("F1")), hierarchy: "H1".into(), ..piece(shared) });
    assert!(!a.shared_checking.contains(&shared), "kept");
    assert!(a.session.mailbox.account(shared).is_some());
    assert!(a.session.mailbox.folder_by_server(shared, "F1").is_some());
}

#[test]
fn the_add_shared_mailbox_dialog_tells_an_exchange_owner_of_their_server_not_of_microsoft() {
    let (mut a, owner, shared, _, _) = window();
    a.passwords.insert(owner, TYPED.into());
    a.begin_shared_check(shared).unwrap();
    a.dialog = Some(Dialog::SharedMailbox { account: owner, address: SHARED.into(), error: String::new(), checking: Some(shared) });
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
    h.run_steps(2);
    let _ = h.get_by_label_contains(&format!("Opened with {OWNER}'s server and password."));
    let _ = h.get_by_label_contains("Checking the address with the server");
    assert!(h.query_by_label_contains("Microsoft asks").is_none());
    assert!(h.query_by_label_contains("Checking the address with Microsoft").is_none());
}

#[test]
fn a_shared_mailbox_the_server_will_not_open_is_removed_again_with_the_reason() {
    let (mut a, owner, shared, _, feed) = window();
    a.passwords.insert(owner, TYPED.into());
    a.dialog = Some(Dialog::SharedMailbox { account: owner, address: SHARED.into(), error: String::new(), checking: Some(shared) });
    a.begin_shared_check(shared).unwrap();
    let why = "The server won't open the shared mailbox: it isn't there, or you have no access to it";
    send(&mut a, &feed, EwsMailResult { partial: false, errors: vec![why.into()], ..piece(shared) });
    assert!(a.session.mailbox.account(shared).is_none(), "removed again");
    assert!(!a.shared_checking.contains(&shared));
    match &a.dialog {
        Some(Dialog::SharedMailbox { error, checking, .. }) => assert_eq!((error.as_str(), *checking), (why, None)),
        other => panic!("the dialog stays with the reason: {}", other.is_some()),
    }
    assert!(a.session.mailbox.account(owner).is_some(), "the owner is untouched");
}

#[test]
fn an_exchange_account_whose_server_cannot_be_used_is_not_offered_a_shared_mailbox() {
    let (mut a, owner, shared, ..) = window();
    let can = |a: &MailApp, id: Id| crate::shared_mailboxes::can_open_shared(a.session.mailbox.account(id).unwrap());
    assert!(can(&a, owner));
    assert!(!can(&a, shared), "a shared mailbox opens no other");
    a.session.mailbox.account_mut(owner).unwrap().ews.as_mut().unwrap().url = "http://exchange.example.org/EWS".into();
    assert!(!can(&a, owner));
}

#[test]
fn a_shared_mailbox_being_added_is_read_once_its_owners_password_is_typed() {
    let (mut a, owner, shared, jobs, _) = window();
    a.begin_shared_check(shared).unwrap();
    assert!(matches!(a.dialog, Some(Dialog::Password { account, .. }) if account == owner), "the owner's password is asked for");
    assert!(jobs.lock().unwrap().is_empty());
    // Typed in the Sign In dialog: the shared mailbox's run goes with it, not the owner's.
    a.dialog = None;
    let started = a.sign_in_answered(owner, Some(TYPED)).unwrap();
    assert_eq!(started["started"], true, "{started}");
    let jobs = jobs.lock().unwrap();
    let (request, secret) = jobs.last().expect("the shared mailbox's run was handed to the host");
    assert_eq!((request.account, request.mailbox.as_str(), secret.as_str()), (shared, SHARED, TYPED));
    assert_eq!(a.passwords.get(&owner).map(String::as_str), Some(TYPED));
    assert!(a.shared_checking.contains(&shared), "still being read");
    assert!(a.ews_password_then.is_none());
}

#[test]
fn a_shared_mailbox_being_added_whose_owner_gives_no_password_is_not_kept() {
    let (mut a, owner, shared, jobs, _) = window();
    a.begin_shared_check(shared).unwrap();
    a.dialog = None;
    a.sign_in_answered(owner, None).unwrap();
    assert!(a.session.mailbox.account(shared).is_none(), "removed again");
    assert!(!a.shared_checking.contains(&shared));
    assert!(jobs.lock().unwrap().is_empty());
    assert!(a.session.mailbox.account(owner).is_some(), "the owner is untouched");
}

#[test]
fn a_shared_mailbox_being_added_whose_owners_password_is_refused_forgets_the_owners() {
    let (mut a, owner, shared, _, feed) = window();
    a.passwords.insert(owner, TYPED.into());
    a.begin_shared_check(shared).unwrap();
    send(&mut a, &feed, EwsMailResult { partial: false, auth_refused: true, ..piece(shared) });
    assert!(a.session.mailbox.account(shared).is_none(), "removed again");
    // (Though the shared mailbox is gone, the password was its owner's: forgotten, and asked for again.)
    assert!(!a.passwords.contains_key(&owner), "the owner's password is forgotten");
    assert!(a.jmap_refused.contains(&owner));
}

/// The shared mailbox's run is turned down for the owner's password; then the owner signs in again
/// and their own run goes through.
fn refused_then_signed_in_again(a: &mut MailApp, owner: Id, shared: Id, feed: &Sender) {
    a.passwords.insert(owner, TYPED.into());
    a.execute("ui.sync", json!({"account": shared})).unwrap();
    send(a, feed, EwsMailResult { partial: false, auth_refused: true, ..piece(shared) });
    assert!(a.jmap_refused.contains(&shared) && a.jmap_refused.contains(&owner));
    assert!(!a.can_sync_quietly(shared));
    a.execute("ui.sync", json!({"account": owner, "password": TYPED})).unwrap();
    send(a, feed, EwsMailResult { account: owner, email: OWNER.into(), url: EWS.into(), whole: true, ..Default::default() });
}

#[test]
fn a_shared_mailbox_held_by_its_owners_turned_down_password_is_read_again_once_the_owner_signs_in() {
    let (mut a, owner, shared, _, feed) = window();
    refused_then_signed_in_again(&mut a, owner, shared, &feed);
    assert!(a.can_sync_quietly(owner));
    assert!(a.can_sync_quietly(shared), "automatic reads take it up again");
}

#[test]
fn a_shared_mailbox_waiting_on_the_certificate_still_waits_when_the_owner_signs_in() {
    let (mut a, owner, shared, _, feed) = window();
    a.ews_certificate.insert(shared, "the server's certificate".into());
    refused_then_signed_in_again(&mut a, owner, shared, &feed);
    assert!(a.can_sync_quietly(owner));
    assert!(!a.can_sync_quietly(shared), "its certificate question is still open");
}

#[test]
fn a_shared_mailbox_whose_run_stopped_at_the_certificate_waits_for_the_owner() {
    let (mut a, owner, shared, _, _) = window();
    a.passwords.insert(owner, TYPED.into());
    assert!(a.can_sync_quietly(shared));
    // Its own run stopped at the certificate question: no round reads it by itself until that is answered.
    a.jmap_refused.insert(shared);
    assert!(!a.can_sync_quietly(shared));
    assert!(a.can_sync_quietly(owner), "the owner's own isn't held by it");
}
