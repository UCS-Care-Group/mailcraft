// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A shared mailbox opened through an Exchange server account, in the window (#482): it signs in with
//! the owner's password and server, names its own address in the run, and is kept or removed
//! again by what its first read brings. Hosts are fakes; every address is invented.

use std::sync::{Arc, Mutex, mpsc};

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
