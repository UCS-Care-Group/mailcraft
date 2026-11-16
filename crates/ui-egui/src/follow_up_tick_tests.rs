// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use mailcraft_engine::Session;
use mailcraft_model::{Address, FolderKind, Mailbox, Message};
use serde_json::json;

use super::EVERY;
use crate::{MailApp, Services};

const NOW: i64 = 1_800_000_000;

#[test]
fn a_reminder_due_while_closed_is_raised_at_start_and_then_looked_at_every_half_minute() {
    let mut mailbox = Mailbox::with_local_account("Ann", "ann@example.org");
    let account = mailbox.accounts[0].id;
    let inbox = mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let sent = mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    let a = mailbox.add_message(Message { account, folder: inbox, subject: "Kickoff".into(), read: true, ..Default::default() }).unwrap();
    let b = mailbox
        .add_message(Message {
            account,
            folder: sent,
            subject: "Budget".into(),
            message_id: "b@example.org".into(),
            to: vec![Address::new("Bob", "bob@example.org")],
            read: true,
            ..Default::default()
        })
        .unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.execute("mail.reminder", json!({"ids": [a], "at": NOW + 30})).unwrap();
    app.execute("mail.reminder", json!({"ids": [b], "at": NOW + 100, "ifNoReply": true})).unwrap();
    // Closed past the first: it is raised at the first look, the second isn't due.
    app.session.clock.now = Some(NOW + 40);
    app.follow_up_tick();
    assert_eq!(app.session.mailbox.due_follow_ups(NOW + 40), vec![a]);
    // Not looked at again within the half minute, even though the second is due.
    app.session.clock.now = Some(NOW + 40 + EVERY - 1);
    app.session.mailbox.message_mut(b).unwrap().follow_up.as_mut().unwrap().at = NOW + 41;
    app.follow_up_tick();
    assert_eq!(app.session.mailbox.due_follow_ups(NOW + 70), vec![a]);
    app.session.clock.now = Some(NOW + 40 + EVERY);
    app.follow_up_tick();
    assert_eq!(app.session.mailbox.due_follow_ups(NOW + 70), vec![a, b]);
}

#[test]
fn a_look_with_nothing_due_runs_no_command_and_changes_nothing() {
    let mut mailbox = Mailbox::with_local_account("Ann", "ann@example.org");
    let account = mailbox.accounts[0].id;
    let inbox = mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let a = mailbox.add_message(Message { account, folder: inbox, read: true, ..Default::default() }).unwrap();
    let mut app = MailApp::new(Session::at(mailbox, NOW, 0), Services::default());
    app.autosave = false;
    app.execute("mail.reminder", json!({"ids": [a], "at": NOW + 3_600})).unwrap();
    let rev = app.session.rev();
    for n in 0..5 {
        app.session.clock.now = Some(NOW + 60 * (n + 1));
        app.follow_up_tick();
    }
    assert_eq!(app.session.rev(), rev, "nothing was due: no command ran, nothing was copied or saved");
    // Raised, then dismissed: the look stops for good.
    app.session.clock.now = Some(NOW + 3_700);
    app.follow_up_tick();
    app.execute("mail.dismissReminder", json!({"ids": [a]})).unwrap();
    let rev = app.session.rev();
    for n in 0..5 {
        app.session.clock.now = Some(NOW + 4_000 + 60 * n);
        app.follow_up_tick();
    }
    assert_eq!(app.session.rev(), rev);
}
