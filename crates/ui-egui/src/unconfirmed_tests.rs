// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox opened from a file is unconfirmed until the owner confirms its servers (C1 PR-c).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use mailcraft_engine::Session;
use mailcraft_model::sync::{SyncRequest, SyncResult};
use serde_json::json;

use super::*;
use crate::Services;

const NOW: i64 = 1_791_993_600;

/// A mailbox file with one account at `host`, written under a folder of its own.
fn crafted(name: &str, host: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("mailcraft-c1c-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": "owner@example.org", "incoming": {"host": host, "port": 993}, "outgoing": {"host": host, "port": 465}}))
        .unwrap();
    let path = dir.join("crafted.mailcraft");
    s.run("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
    (dir, path)
}

/// An app with a Send/Receive that counts its runs.
fn app(runs: &Arc<AtomicUsize>) -> MailApp {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let runs = runs.clone();
    a.services.sync = Some(Box::new(move |req: SyncRequest| {
        runs.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        tx.send(SyncResult { account: req.account.id, ..Default::default() }).unwrap();
        rx
    }));
    a
}

fn open(a: &mut MailApp, path: &std::path::Path) {
    a.execute("file.open", json!({"path": path.to_string_lossy()})).unwrap();
}

#[test]
fn a_mailbox_from_a_file_is_unconfirmed_and_nothing_of_it_goes_out() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut a = app(&runs);
    let (dir, path) = crafted("held", "imap.attacker.example");
    open(&mut a, &path);
    assert!(a.is_unconfirmed());
    assert!(a.services.sync.is_none() && a.services.graph.is_none() && a.services.sign_in.is_none());
    assert_eq!(a.account_servers().len(), 1);
    assert!(a.account_servers()[0].contains("imap.attacker.example:993"));
    // Nobody asked: nothing starts.
    assert!(a.execute("ui.sync", json!({"account": a.session.mailbox.accounts[0].id, "quiet": true})).is_err());
    // The owner pressed Send/Receive: the question about the servers comes up instead.
    let r = a.execute("ui.sync", json!({})).unwrap();
    assert_eq!(r["pending"], "confirmServers");
    assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::Confirm { .. })));
    assert_eq!(runs.load(Ordering::Relaxed), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn confirming_lets_it_connect_and_is_remembered_for_that_file() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut a = app(&runs);
    let (dir, path) = crafted("confirm", "imap.example.org");
    open(&mut a, &path);
    assert!(a.is_unconfirmed());
    let r = a.execute("ui.confirmServers", json!({})).unwrap();
    assert_eq!(r["confirmed"], true);
    assert!(!a.is_unconfirmed() && a.services.sync.is_some());
    assert_eq!(a.ui.confirmed_mailboxes.len(), 1);
    // The same file again, as it was confirmed: not asked.
    a.execute("file.new", json!({})).unwrap();
    open(&mut a, &path);
    assert!(!a.is_unconfirmed() && a.services.sync.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_whose_servers_changed_is_asked_about_again() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut a = app(&runs);
    let (dir, path) = crafted("changed", "imap.example.org");
    open(&mut a, &path);
    a.execute("ui.confirmServers", json!({})).unwrap();
    // Somebody swaps the file for one with the same name and the owner's address, other servers.
    let mut s = Session::empty();
    s.run(
        "account.add",
        &json!({"email": "owner@example.org", "incoming": {"host": "imap.attacker.example", "port": 993}, "outgoing": {"host": "imap.example.org", "port": 465}}),
    )
    .unwrap();
    s.run("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
    open(&mut a, &path);
    assert!(a.is_unconfirmed() && a.services.sync.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_owners_own_mailbox_and_a_mailbox_without_servers_are_never_unconfirmed() {
    let runs = Arc::new(AtomicUsize::new(0));
    // A window started on a file: that file is the owner's own.
    let (dir, own) = crafted("own", "imap.example.org");
    let mut session = Session::empty();
    session.run("file.open", &json!({"path": own.to_string_lossy()})).unwrap();
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    assert!(!a.is_unconfirmed());
    a.execute("file.new", json!({})).unwrap();
    open(&mut a, &own);
    assert!(!a.is_unconfirmed());
    // A file with only local accounts has nothing to confirm.
    let mut local = Session::empty();
    let plain = dir.join("local.mailcraft");
    local.run("file.saveAs", &json!({"path": plain.to_string_lossy()})).unwrap();
    let mut b = app(&runs);
    open(&mut b, &plain);
    assert!(!b.is_unconfirmed());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replacing_an_unconfirmed_mailbox_gives_the_jobs_back() {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut a = app(&runs);
    let (dir, path) = crafted("replace", "imap.attacker.example");
    open(&mut a, &path);
    assert!(a.services.sync.is_none());
    a.execute("file.new", json!({})).unwrap();
    assert!(!a.is_unconfirmed() && a.services.sync.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_fingerprint_follows_addresses_and_servers_not_their_order() {
    let mut one = Session::empty();
    let mut two = Session::empty();
    let add = |s: &mut Session, email: &str, host: &str| {
        s.run("account.add", &json!({"email": email, "incoming": {"host": host, "port": 993}})).unwrap();
    };
    add(&mut one, "a@example.org", "imap.example.org");
    add(&mut one, "b@example.org", "imap.example.org");
    add(&mut two, "b@example.org", "IMAP.example.org");
    add(&mut two, "a@example.org", "imap.example.org");
    assert_eq!(fingerprint(&one.mailbox), fingerprint(&two.mailbox));
    add(&mut two, "c@example.org", "imap.example.org");
    assert_ne!(fingerprint(&one.mailbox), fingerprint(&two.mailbox));
    let mut three = Session::empty();
    add(&mut three, "a@example.org", "imap.example.org");
    add(&mut three, "b@example.org", "imap.attacker.example");
    assert_ne!(fingerprint(&one.mailbox), fingerprint(&three.mailbox));
}
