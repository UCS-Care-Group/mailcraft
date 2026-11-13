// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Mailbox, Message};
use serde_json::json;

use super::EVERY;
use crate::{MailApp, Services};

const NOW: i64 = 1_800_000_000;

#[test]
fn snoozed_mail_that_is_due_comes_back_at_start_and_then_once_a_minute() {
    let mut mailbox = Mailbox::with_local_account("Ann", "ann@example.org");
    let account = mailbox.accounts[0].id;
    let inbox = mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let a = mailbox.add_message(Message { account, folder: inbox, read: true, ..Default::default() }).unwrap();
    let b = mailbox.add_message(Message { account, folder: inbox, read: true, ..Default::default() }).unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.execute("mail.snooze", json!({"ids": [a], "until": NOW + 30})).unwrap();
    app.execute("mail.snooze", json!({"ids": [b], "until": NOW + 100})).unwrap();
    // Closed past the Return time of the first: it comes back at the first look.
    app.session.clock.now = Some(NOW + 40);
    app.snooze_tick();
    assert_eq!(app.session.mailbox.message(a).unwrap().folder, inbox);
    assert_ne!(app.session.mailbox.message(b).unwrap().folder, inbox);
    // Not looked at again within the minute, even when the second is due.
    app.session.clock.now = Some(NOW + 40 + EVERY - 1);
    app.snooze_tick();
    assert_ne!(app.session.mailbox.message(b).unwrap().folder, inbox);
    app.session.clock.now = Some(NOW + 40 + EVERY);
    app.snooze_tick();
    assert_eq!(app.session.mailbox.message(b).unwrap().folder, inbox);
}
