// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Work Offline, Cancel All, Help's pages and File › Account in the window. Invented people and
//! servers; the hosts are fakes that count what they are asked.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use mailcraft_engine::Session;
use mailcraft_model::FolderKind;
use mailcraft_model::sync::{SyncRequest, SyncResult};
use serde_json::json;

use super::*;
use crate::Services;

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A window with an IMAP account whose host counts the runs it is asked for (and never answers),
/// and whose picture, sender-image and mail-exchanger hosts count theirs.
fn window() -> (MailApp, u64, Rc<Cell<usize>>) {
    let mut a = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    a.autosave = false;
    let asked = Rc::new(Cell::new(0));
    let runs = asked.clone();
    a.services.sync = Some(Box::new(move |_: SyncRequest| {
        runs.set(runs.get() + 1);
        mpsc::channel::<SyncResult>().1
    }));
    let looks = asked.clone();
    a.services.lookup_mx = Some(Box::new(move |_| {
        looks.set(looks.get() + 1);
        mpsc::channel().1
    }));
    let account = a
        .execute(
            "account.add",
            json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}}),
        )
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    a.adopt_password(account, "pw".to_string());
    (a, account, asked)
}

#[test]
fn working_offline_nothing_is_sent_or_received_and_mail_waits_in_the_outbox() {
    let (mut a, account, asked) = window();
    assert!(!a.offline());
    a.execute("sync.offline", json!({})).unwrap();
    assert!(a.offline());
    // Send/Receive by hand is refused with why; nothing reaches the host.
    assert_eq!(a.execute("ui.sync", json!({"account": account})).unwrap_err(), said());
    assert_eq!(a.execute("ui.sync", json!({})).unwrap_err(), said());
    // Mail that is sent waits in the Outbox, and nothing starts by itself to send it.
    a.execute("mail.new", json!({"to": "maya@lumen.example", "subject": "Later", "account": account})).unwrap();
    a.execute("compose.send", json!({})).unwrap();
    let outbox = a.session.mailbox.folder_of_kind(account, FolderKind::Outbox).unwrap();
    assert_eq!(a.session.mailbox.messages_in(outbox).count(), 1);
    a.push_checked = 0.0;
    a.auto_sync();
    assert!(a.sync_queue.is_empty() && !a.syncing());
    // Nor is an Inbox pass.
    assert!(a.start_inbox_pass(account, false, false).is_none());
    assert_eq!(asked.get(), 0, "no host was asked for anything");
    // Back online: a Send/Receive starts at once and takes the Outbox along.
    a.execute("sync.offline", json!({"value": false})).unwrap();
    assert!(a.syncing());
    assert_eq!(asked.get(), 1);
}

#[test]
fn going_offline_calls_off_the_run_under_way() {
    let (mut a, account, _) = window();
    a.execute("ui.sync", json!({"account": account})).unwrap();
    assert!(a.syncing());
    a.sync_queue.push_back((account, true));
    a.execute("sync.offline", json!({"value": true})).unwrap();
    assert!(a.services.sync_cancel.load(Ordering::Relaxed), "the host is told to stop");
    assert!(a.sync_queue.is_empty());
}

#[test]
fn cancel_all_stops_the_run_and_is_greyed_without_one() {
    let (mut a, account, _) = window();
    assert!(!a.enabled("sync.cancel"));
    assert_eq!(a.execute("sync.cancel", json!({})).unwrap()["cancelling"], false);
    a.execute("ui.sync", json!({"account": account})).unwrap();
    assert!(a.enabled("sync.cancel"));
    assert_eq!(a.execute("sync.cancel", json!({})).unwrap()["cancelling"], true);
    assert!(a.services.sync_cancel.load(Ordering::Relaxed));
}

#[test]
fn work_offline_is_not_undone_and_is_kept_with_the_window_settings() {
    let (mut a, _, _) = window();
    a.execute("sync.offline", json!({"value": true})).unwrap();
    let _ = a.execute("edit.undo", json!({}));
    assert!(a.offline(), "Undo of what came before leaves Work Offline on");
    let kept = a.prefs();
    let mut b = MailApp::new(Session::empty(), Services::default());
    b.apply_prefs(kept);
    assert!(b.offline(), "and it is on again the next time");
}

#[test]
fn help_answers_the_projects_pages_and_feedback_opens_report_a_problem() {
    let (mut a, _, _) = window();
    let help = a.run("help.help", json!({})).unwrap();
    assert!(help["url"].as_str().unwrap().starts_with(mailcraft_engine::cmd::help::REPOSITORY));
    let news = a.run("help.whatsNew", json!({})).unwrap();
    assert!(news["url"].as_str().unwrap().ends_with(&format!("/releases/tag/v{}", env!("CARGO_PKG_VERSION"))));
    a.run("help.feedback", json!({})).unwrap();
    assert!(matches!(a.dialog, Some(crate::dialogs::Dialog::ReportProblem(_))), "Report a Problem says what goes before anything does");
}

#[test]
fn file_account_shows_the_about_page() {
    let (mut a, account, _) = window();
    let told = a.run("file.account", json!({})).unwrap();
    assert!(told["accounts"].as_array().unwrap().iter().any(|x| x["id"] == json!(account)));
    assert!(a.ui.backstage);
    assert_eq!(a.ui.backstage_page, "about");
}

#[test]
fn the_window_and_the_engine_say_the_same_words() {
    assert_eq!(said(), mailcraft_engine::cmd::send_receive::OFFLINE);
}
