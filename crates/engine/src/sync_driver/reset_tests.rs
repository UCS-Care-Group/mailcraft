// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox that takes the place of the open one starts the sync driver afresh: none of what it
//! remembered of the old mailbox's accounts is kept (their ids may name other accounts).

use mailcraft_model::Mailbox;

use super::{Driver, Note};
use crate::Session;

/// A driver with something in every part of it.
fn busy(driver: &mut Driver) {
    driver.thinning.started(1, 10.0);
    driver.thinning.now = true;
    driver.calendar.by_hand.insert(7);
    driver.calendar.directory = true;
    driver.calendar.due = true;
    driver.calendar.extras_missing.insert(7);
    driver.calendar.send_waits.insert(7);
    driver.calendar.settings_asked.insert(7, 5.0);
    driver.calendar.last_check.push("a line".into());
    driver.calendar.book.started(7, 5.0);
    driver.calendar.book.failed(7, 5.0);
    driver.calendar.opened = Some((3, None));
    driver.calendar.settled(4);
    driver.calendar.shared_done(4);
    driver.favorites.missing.insert(7, 1.0);
    driver.favorites.notes.insert(7, Note::Offer);
    driver.favorites.connected.insert(7);
    driver.fetch.idle_until = 9.0;
    driver.fetch.offline = true;
    driver.fetch.stuck.insert(7, (2, 9.0));
}

#[test]
fn another_mailbox_takes_the_place_with_a_driver_that_remembers_nothing() {
    let mut session = Session::new(Mailbox::with_local_account("", ""));
    busy(&mut session.driver);
    assert_ne!(format!("{:?}", session.driver), format!("{:?}", Driver::default()), "the test fills it");
    session.replace_mailbox(Mailbox::with_local_account("", ""));
    assert_eq!(format!("{:?}", session.driver), format!("{:?}", Driver::default()));
}
