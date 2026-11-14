// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox that takes the place of the open one starts the sync driver afresh: none of what it
//! remembered of the old mailbox's accounts is kept (their ids may name other accounts). An
//! account that is removed takes with it what the driver remembered of it, whatever job it was for.

use std::collections::HashSet;

use mailcraft_model::{Id, Mailbox};
use serde_json::json;

use super::auto_replies::Trouble;
use super::free_busy::Key;
use super::{Driver, Note};
use crate::Session;

/// What the driver remembers that belongs to no account.
fn global(driver: &mut Driver) {
    driver.thinning.started(1, 10.0);
    driver.thinning.now = true;
    driver.calendar.directory = true;
    driver.calendar.due = true;
    driver.calendar.last_check.push("a line".into());
    driver.calendar.opened = Some((3, None));
    driver.calendar.settled(4);
    driver.calendar.shared_done(4);
    driver.fetch.idle_until = 9.0;
    driver.fetch.offline = true;
}

/// What the driver remembers of `account`, in every part of it.
fn busy_for(driver: &mut Driver, account: Id) {
    driver.calendar.by_hand.insert(account);
    driver.calendar.extras_missing.insert(account);
    driver.calendar.send_waits.insert(account);
    driver.calendar.settings_asked.insert(account, 5.0);
    driver.calendar.book.started(account, 5.0);
    driver.calendar.book.failed(account, 5.0);
    driver.calendar.book.waiting_since(account, 2, 5.0);
    driver.calendar.consent_needed.insert(account);
    driver.calendar.checked.insert(account, 4.0);
    driver.calendar.shared_perm.insert(account, super::SharedPermission::Needed);
    driver.favorites.missing.insert(account, 1.0);
    driver.favorites.notes.insert(account, Note::Offer);
    driver.favorites.connected.insert(account);
    driver.favorites.directories.insert(account, mailcraft_model::Directory::Personal);
    driver.favorites.tenants.insert(account, "t".into());
    driver.fetch.stuck.insert(account, (2, 9.0));
    driver.tasks.perm.insert(account, super::Permission::Needed);
    driver.tasks.settling.insert(account, 2);
    driver.tasks.book.started(account, 5.0);
    driver.rules.perm.insert(account, super::Permission::Needed);
    driver.rules.settling.insert(account, 2);
    driver.rules.read.insert(account);
    driver.rules.unreadable.insert(account);
    driver.rules.book.started(account, 5.0);
    driver.auto_replies.asking(account);
    driver.auto_replies.hold(account, Default::default());
    driver.auto_replies.waiting = Some((account, Default::default()));
    driver.auto_replies.trouble.insert(account, Trouble::SignInToRead);
    driver.auto_replies.started(account, 5.0);
    driver.free_busy.note(
        Key { account, day: mailcraft_time::Date::new(2026, 10, 14).unwrap(), people: vec!["a@b.example".into()] },
        1.0,
        None,
        "x".into(),
    );
    driver.sending.waits.insert(account);
}

#[test]
fn another_mailbox_takes_the_place_with_a_driver_that_remembers_nothing() {
    let mut session = Session::new(Mailbox::with_local_account("", ""));
    global(&mut session.driver);
    busy_for(&mut session.driver, 7);
    assert_ne!(format!("{:?}", session.driver), format!("{:?}", Driver::default()), "the test fills it");
    session.replace_mailbox(Mailbox::with_local_account("", ""));
    assert_eq!(format!("{:?}", session.driver), format!("{:?}", Driver::default()));
}

#[test]
fn what_the_host_set_is_not_the_mailboxs_and_stays() {
    let mut session = Session::new(Mailbox::with_local_account("", ""));
    assert!(session.driver.auto_sync());
    session.driver.set_auto_sync(false);
    session.replace_mailbox(Mailbox::with_local_account("", ""));
    assert!(!session.driver.auto_sync());
    session.driver.set_auto_sync(true);
    session.replace_mailbox(Mailbox::with_local_account("", ""));
    assert!(session.driver.auto_sync());
}

#[test]
fn removed_accounts_take_what_every_job_remembered_of_them_and_the_others_keep_theirs() {
    let mut driver = Driver::default();
    global(&mut driver);
    let mut empty = Driver::default();
    global(&mut empty);
    busy_for(&mut driver, 7);
    busy_for(&mut driver, 8);
    driver.forget_accounts(&HashSet::from([7, 8]));
    assert_eq!(format!("{driver:?}"), format!("{empty:?}"));

    busy_for(&mut driver, 9);
    driver.forget_accounts(&HashSet::from([7]));
    assert!(driver.tasks.perm.contains_key(&9) && driver.rules.read.contains(&9) && driver.auto_replies.perm.contains_key(&9));
    assert!(driver.calendar.checked.contains_key(&9) && driver.favorites.notes.contains_key(&9) && driver.sending.waits.contains(&9));
    assert_eq!(driver.free_busy.known.len(), 1);
}

#[test]
fn removing_an_account_forgets_what_the_driver_remembered_of_it() {
    let mut s = Session::at(crate::sample::mailbox(1_790_000_000, 0), 1_790_000_000, 0);
    let account = s.run("account.add", &json!({"email": "ada@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    busy_for(&mut s.driver, account);
    s.run("account.remove", &json!({"id": account})).unwrap();
    assert!(s.driver.tasks.perm.is_empty() && s.driver.rules.perm.is_empty() && s.driver.auto_replies.perm.is_empty());
    assert!(s.driver.calendar.shared_perm.is_empty() && s.driver.calendar.checked.is_empty() && s.driver.favorites.notes.is_empty());
    assert!(s.driver.free_busy.known.is_empty() && s.driver.sending.waits.is_empty() && s.driver.fetch.stuck.is_empty());
    assert!(s.driver.auto_replies.waiting.is_none() && s.driver.auto_replies.held.is_empty());
}
