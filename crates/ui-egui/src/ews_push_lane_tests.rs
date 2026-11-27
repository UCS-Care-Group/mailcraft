// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What was done to an Exchange server account goes up beside a long read of another account's
//! mail, not after it: a message deleted in one account while another's mail is coming down is
//! told to its server at once, by a run that only takes things up; the account that is being read
//! is not touched by it (its own run goes up first, as before).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use mailcraft_engine::Session;
use mailcraft_engine::cmd::ews_mail::apply_ews_mail;
use mailcraft_model::sync::{EwsFolderDeltas, EwsMailResult, EwsMessagePage, GraphFolder, GraphMessage, ServerMessage};
use mailcraft_model::{Address, FolderKind};
use serde_json::json;

use crate::{MailApp, Services};

const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

/// Which runs the host was handed (account, whether it only goes up), and the end each answers through.
type Jobs = Arc<Mutex<Vec<(u64, bool)>>>;
type Pipes = Arc<Mutex<HashMap<u64, mpsc::Sender<EwsMailResult>>>>;

fn app() -> (MailApp, Jobs, Pipes) {
    let mut a = MailApp::new(Session::empty(), Services::default());
    a.autosave = false;
    let (jobs, pipes): (Jobs, Pipes) = Default::default();
    let (seen, feed) = (jobs.clone(), pipes.clone());
    a.services.ews = Some(Box::new(move |job| {
        let (tx, rx) = mpsc::channel();
        seen.lock().unwrap().push((job.request.account, job.request.push_only));
        feed.lock().unwrap().insert(job.request.account, tx);
        rx
    }));
    (a, jobs, pipes)
}

fn piece(account: u64, email: &str) -> EwsMailResult {
    EwsMailResult { account, email: email.into(), url: URL.into(), partial: true, ..Default::default() }
}

/// An Exchange server account whose folders and one message in its Inbox are the server's.
fn account(a: &mut MailApp, email: &str, message: &str) -> u64 {
    let added = a.execute("account.add", json!({"email": email, "kind": "exchange", "ews": {"url": URL}})).unwrap();
    let id = added["id"].as_u64().unwrap();
    let folder = |id: &str, name: &str, kind| GraphFolder { id: id.into(), name: name.into(), kind, ..Default::default() };
    let folders = vec![folder("F1", "Inbox", FolderKind::Inbox), folder("F2", "Deleted Items", FolderKind::Deleted)];
    let deltas = EwsFolderDeltas { changed: folders, afresh: true, ..Default::default() };
    apply_ews_mail(&mut a.session, EwsMailResult { folders: Some(deltas), hierarchy: "H1".into(), ..piece(id, email) }).unwrap();
    let mail = GraphMessage {
        server: ServerMessage { id: message.into(), size: 1_200, change_key: "CK1".into(), ..Default::default() },
        message_id: format!("{message}@example.org"),
        subject: "Plan".into(),
        from: Address::new("Zoe Park", "zoe@example.org"),
        to: vec![Address::new("Ada", email)],
        sent: 1_699_999_940,
        received: 1_700_000_000,
        ..Default::default()
    };
    let page = EwsMessagePage { folder: "F1".into(), messages: vec![mail], afresh: true, last: true, state: "S1".into(), ..Default::default() };
    apply_ews_mail(&mut a.session, EwsMailResult { pages: vec![page], ..piece(id, email) }).unwrap();
    apply_ews_mail(&mut a.session, EwsMailResult { whole: true, partial: false, ..piece(id, email) }).unwrap();
    a.passwords.insert(id, "invented-password".into());
    id
}

/// The message of `account` deleted here: the server hasn't been told.
fn delete_one(a: &mut MailApp, account: u64) {
    let id = a.session.mailbox.messages.values().find(|m| m.account == account).map(|m| m.id).unwrap();
    a.execute("mail.delete", json!({"id": id})).unwrap();
    assert!(a.waiting_to_go(account) > 0, "the deletion waits to go up");
}

#[test]
fn a_message_deleted_in_one_account_goes_up_while_another_is_read() {
    let (mut a, jobs, pipes) = app();
    let reading = account(&mut a, "ada@example.org", "M1");
    let other = account(&mut a, "bo@example.org", "M2");
    // The first account's mail is coming down: its run is under way, and goes on.
    assert_eq!(a.execute("ui.sync", json!({"account": reading, "password": "invented-password"})).unwrap()["started"], true);
    delete_one(&mut a, other);
    a.auto_sync();
    assert_eq!(*jobs.lock().unwrap(), [(reading, false), (other, true)], "a run of its own goes up for the other account");
    assert!(a.syncing(), "the read goes on");

    // What the server answers is taken in, and the run ends without touching the read.
    pipes.lock().unwrap()[&other].send(EwsMailResult { partial: false, ..piece(other, "bo@example.org") }).unwrap();
    a.poll_sync();
    assert!(a.ews_extra.is_empty() && a.syncing(), "the push is over and the read goes on");
}

#[test]
fn the_account_being_read_is_not_given_a_second_run() {
    let (mut a, jobs, _) = app();
    let reading = account(&mut a, "ada@example.org", "M1");
    assert_eq!(a.execute("ui.sync", json!({"account": reading, "password": "invented-password"})).unwrap()["started"], true);
    delete_one(&mut a, reading);
    a.auto_sync();
    assert_eq!(jobs.lock().unwrap().len(), 1, "its own run is called off to go up first, not shared");
    assert!(a.ews_extra.is_empty());
}

#[test]
fn nothing_goes_up_beside_a_run_while_nothing_is_read() {
    let (mut a, jobs, _) = app();
    let other = account(&mut a, "bo@example.org", "M2");
    delete_one(&mut a, other);
    // Nobody is reading: the round's own look takes it up, as before, in a run that reads too.
    a.auto_sync();
    assert!(jobs.lock().unwrap().iter().all(|(_, only_up)| !only_up));
    assert!(a.ews_extra.iter().all(|run| !run.push_only));
}

/// The round's accounts are sent and received together, three at most, not one after another.
#[test]
fn the_accounts_of_a_round_are_read_together_three_at_most() {
    let (mut a, jobs, pipes) = app();
    let ids: Vec<u64> =
        ["ada", "bo", "cy", "di"].iter().enumerate().map(|(n, who)| account(&mut a, &format!("{who}@example.org"), &format!("M{n}"))).collect();
    // The owner asks for a Send/Receive of all of them: the first goes at once, the rest wait their turn.
    assert_eq!(a.execute("ui.sync", json!({})).unwrap()["started"], true);
    assert_eq!(jobs.lock().unwrap().len(), 1, "one run under way");
    a.auto_sync();
    a.auto_sync();
    let started: Vec<(u64, bool)> = jobs.lock().unwrap().clone();
    assert_eq!(started, [(ids[0], false), (ids[1], false), (ids[2], false)], "three together, in the round's order");
    assert!(a.session.driver.round.queue.iter().any(|(account, _)| *account == ids[3]), "the fourth waits its turn");
    // One ends: the next in line goes, beside the two that go on.
    pipes.lock().unwrap()[&ids[1]].send(EwsMailResult { partial: false, ..piece(ids[1], "bo@example.org") }).unwrap();
    a.poll_sync();
    a.auto_sync();
    assert_eq!(jobs.lock().unwrap().len(), 4, "the fourth goes when one of them ends");
}

/// Two accounts of a round, the second read beside the first.
fn two_together() -> (MailApp, Jobs, Pipes, u64, u64) {
    let (mut a, jobs, pipes) = app();
    let first = account(&mut a, "ada@example.org", "M1");
    let second = account(&mut a, "bo@example.org", "M2");
    assert_eq!(a.execute("ui.sync", json!({})).unwrap()["started"], true);
    a.auto_sync();
    assert_eq!(*jobs.lock().unwrap(), [(first, false), (second, false)]);
    (a, jobs, pipes, first, second)
}

/// What the owner does to an account read beside the first calls its run off, which goes up first.
#[test]
fn a_run_beside_the_first_is_called_off_for_what_the_owner_did_and_made_again() {
    let (mut a, _, pipes, _, second) = two_together();
    delete_one(&mut a, second);
    a.ews_extra_checked = 0.0;
    a.auto_sync();
    assert!(a.ews_extra.iter().all(|run| run.is_cancelled()), "its run is told to stop at the next safe place");
    // It ends as a run called off does, and the account's turn comes again, first in line.
    pipes.lock().unwrap()[&second].send(EwsMailResult { partial: false, stopped: true, ..piece(second, "bo@example.org") }).unwrap();
    a.poll_sync();
    assert!(a.ews_extra.is_empty());
    assert_eq!(a.session.driver.round.queue.front().map(|(account, _)| *account), Some(second));
}

/// A password the server turned down is forgotten at the end of the run, as the first run's is.
#[test]
fn a_run_beside_the_first_whose_password_is_turned_down_forgets_it() {
    let (mut a, _, pipes, first, second) = two_together();
    pipes.lock().unwrap()[&second].send(EwsMailResult { partial: false, auth_refused: true, ..piece(second, "bo@example.org") }).unwrap();
    a.poll_sync();
    assert!(!a.passwords.contains_key(&second), "the next Send/Receive asks for it again");
    assert!(a.passwords.contains_key(&first), "the other account is not touched");
}

/// A run whose thread ends early is over: nothing more comes, and it isn't started again at once.
#[test]
fn a_run_beside_the_first_that_ends_without_its_last_piece_is_let_go_of() {
    let (mut a, jobs, pipes, _, second) = two_together();
    pipes.lock().unwrap().remove(&second);
    a.poll_sync();
    assert!(a.ews_extra.is_empty());
    assert_eq!(jobs.lock().unwrap().len(), 2, "not started again by the look for what waits to go");
}

/// Stop reaches every run, and what a mailbox that takes the place of the open one leaves is let go of.
#[test]
fn stop_calls_off_every_run_under_way() {
    let (mut a, jobs, _) = app();
    for (n, who) in ["ada", "bo", "cy"].iter().enumerate() {
        account(&mut a, &format!("{who}@example.org"), &format!("M{n}"));
    }
    assert_eq!(a.execute("ui.sync", json!({})).unwrap()["started"], true);
    a.auto_sync();
    a.auto_sync();
    assert_eq!(jobs.lock().unwrap().len(), 3);
    a.execute("ui.cancelSync", json!({})).unwrap();
    assert!(a.services.sync_cancel.load(std::sync::atomic::Ordering::Relaxed), "the first run is told to stop");
    assert!(a.ews_extra.iter().all(|run| run.is_cancelled()), "and so are the ones beside it");
}
