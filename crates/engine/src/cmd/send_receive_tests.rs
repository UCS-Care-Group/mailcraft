// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Cancel All and Work Offline. Invented people and servers.

use mailcraft_model::FolderKind;
use serde_json::json;

use super::*;
use crate::CmdError;

/// A session with an IMAP account that sends: the session and the account.
fn imap() -> (Session, Id) {
    let mut s = Session::empty();
    let servers = json!({"email": "avery@mail.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example", "port": 587, "security": "starttls"}});
    let a = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    (s, a)
}

#[test]
fn work_offline_toggles_and_is_kept_with_the_preferences() {
    let (mut s, _) = imap();
    assert!(!s.prefs.offline, "off unless the owner turns it on");
    assert_eq!(s.run("sync.offline", &json!({})).unwrap()["offline"], true);
    assert!(s.prefs.offline);
    assert_eq!(s.run("sync.offline", &json!({"value": true})).unwrap()["offline"], true, "a value sets it");
    // Kept between runs with the other preferences, and left out of them while off.
    let saved = serde_json::to_value(&s.prefs).unwrap();
    assert_eq!(saved["offline"], true);
    let back: crate::Prefs = serde_json::from_value(saved).unwrap();
    assert!(back.offline);
    assert_eq!(s.run("sync.offline", &json!({})).unwrap()["offline"], false);
    assert!(serde_json::to_value(&s.prefs).unwrap().get("offline").is_none());
}

#[test]
fn nothing_asks_a_server_while_offline_and_sent_mail_waits_in_the_outbox() {
    let (mut s, a) = imap();
    s.run("sync.offline", &json!({"value": true})).unwrap();
    for id in ASKS_A_SERVER {
        match s.run(id, &json!({"account": a})) {
            Err(CmdError::Disabled(why)) => assert_eq!(why, OFFLINE, "{id}"),
            other => panic!("{id} wasn't refused: {other:?}"),
        }
    }
    // Sending still works: the message waits in the Outbox.
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Later", "account": a})).unwrap();
    assert_eq!(s.run("compose.send", &json!({})).unwrap()["queued"], true);
    let outbox_folder = s.mailbox.folder_of_kind(a, FolderKind::Outbox).unwrap();
    assert_eq!(s.mailbox.messages_in(outbox_folder).count(), 1);
    assert_eq!(s.run("sync.offline", &json!({"value": true})).unwrap()["outbox"], 1, "the answer counts what waits");
    // Back online, it goes with the next Send/Receive.
    s.run("sync.offline", &json!({"value": false})).unwrap();
    assert_eq!(s.run("sync.request", &json!({"account": a})).unwrap()["outbox"][0]["subject"], "Later");
}

#[test]
fn work_offline_is_no_undo_step_and_undo_leaves_it_alone() {
    let (mut s, a) = imap();
    s.run("folder.new", &json!({"name": "Projects", "account": a})).unwrap();
    s.run("sync.offline", &json!({"value": true})).unwrap();
    s.run("edit.undo", &json!({})).unwrap();
    assert!(s.prefs.offline, "Undo takes back the folder, not Work Offline");
    assert!(!s.mailbox.folders.iter().any(|f| f.name == "Projects"));
}

#[test]
fn cancel_all_stops_the_accounts_waiting_their_turn() {
    let (mut s, a) = imap();
    let quiet = s.run("sync.cancel", &json!({})).unwrap();
    assert_eq!(quiet, json!({"cancelling": false, "stopped": []}), "nothing under way: nothing to stop");
    s.sync_progress.begin(&[a]);
    assert!(s.sync_progress.running());
    let told = s.run("sync.cancel", &json!({})).unwrap();
    assert_eq!(told, json!({"cancelling": true, "stopped": [a]}));
    assert!(!s.sync_progress.running(), "the run is over");
    assert_eq!(s.run("sync.progress", &json!({})).unwrap()["accounts"][0]["state"], "stopped");
}
