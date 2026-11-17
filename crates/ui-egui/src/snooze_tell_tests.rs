// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A snooze given up (#949) is told to the owner by the run that gave it up: in the problems
//! report and the run's summary, as any other problem of the run is.
use mailcraft_engine::Session;
use mailcraft_model::sync::SyncResult;
use mailcraft_model::{FolderKind, Message};
use serde_json::json;

use crate::{MailApp, MailRun, Services};

const NOW: i64 = 1_800_000_000;

#[test]
fn an_imap_run_that_gives_a_snooze_up_tells_the_owner() {
    let mut app = MailApp::new(Session::empty(), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let account = app.execute("account.add", servers).unwrap()["id"].as_u64().unwrap();
    app.session.clock.now = Some(NOW);
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let id = app.session.mailbox.add_message(Message { account, folder: inbox, subject: "Kickoff".into(), ..Default::default() }).unwrap();
    app.session.mailbox.snooze_messages(&[id], NOW + 60, "Snoozed").unwrap();
    let snoozed = app.session.mailbox.snoozed_folder(account).unwrap();
    // The server refused the move back twice already; the third refusal is seen by this run.
    for _ in 0..2 {
        assert_eq!(app.session.mailbox.return_snoozed(&[id], NOW + 61), vec![id]);
        app.session.mailbox.message_mut(id).unwrap().folder = snoozed;
        app.session.mailbox.settle_snoozes();
    }
    assert_eq!(app.session.mailbox.return_snoozed(&[id], NOW + 61), vec![id]);
    app.session.mailbox.message_mut(id).unwrap().folder = snoozed;

    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(SyncResult { account, ..Default::default() }).unwrap();
    app.sync_rx = Some(MailRun::Imap { account, opening: app.session.openings(), rx });
    app.poll_sync();

    assert!(app.sync_rx.is_none(), "the run is over");
    let lines: Vec<String> = app.sync_problems.iter().flat_map(|(.., lines)| lines).map(ToString::to_string).collect();
    assert!(lines.iter().any(|l| l.contains("no longer snoozed")), "{lines:?}");
    assert!(app.session.mailbox.message(id).unwrap().snooze.is_none());
    // Said once, not twice.
    assert_eq!(lines.iter().filter(|l| l.contains("no longer snoozed")).count(), 1, "{lines:?}");
}
