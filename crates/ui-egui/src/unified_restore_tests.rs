// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! All Accounts (#625) open at quit opens again at the next start, from the window's settings;
//! a setting this mailbox can't follow (one account, an unknown kind, a damaged value) starts on
//! the usual folder, and never stops the settings from being read.

use mailcraft_engine::Session;
use mailcraft_model::{Account, FolderKind};
use serde_json::json;

use crate::{MailApp, Services, UiState};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The sample mailbox with a second account.
fn two_accounts() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.mailbox.add_account(Account { name: "Work".into(), email: "me@work.example".into(), ..Default::default() });
    app
}

fn saved(app: &MailApp) -> String {
    serde_json::to_string(&app.prefs()).unwrap()
}

#[test]
fn all_accounts_open_at_quit_opens_again() {
    let mut app = two_accounts();
    app.execute("view.allAccounts", json!({"kind": "sent"})).unwrap();
    let text = saved(&app);
    let mut next = two_accounts();
    assert_eq!(mailcraft_engine::unified::of(&next.session), None);
    next.apply_prefs(serde_json::from_str(&text).unwrap());
    assert_eq!(mailcraft_engine::unified::of(&next.session), Some(FolderKind::Sent));
    assert_eq!(next.session.view.unified, Some(FolderKind::Sent));
}

#[test]
fn one_accounts_folder_open_at_quit_is_not_all_accounts_next_time() {
    let mut app = two_accounts();
    app.execute("view.allAccounts", json!({})).unwrap();
    let inbox = app.session.mailbox.inbox().unwrap();
    app.execute("folder.select", json!({"id": inbox})).unwrap();
    let mut next = two_accounts();
    next.apply_prefs(serde_json::from_str(&saved(&app)).unwrap());
    assert_eq!(next.session.view.unified, None);
}

#[test]
fn a_setting_the_mailbox_cant_follow_starts_on_the_usual_folder() {
    let mut app = two_accounts();
    app.execute("view.allAccounts", json!({})).unwrap();
    let text = saved(&app);
    // Only one account in the next mailbox: nothing to put together.
    let mut one = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    one.autosave = false;
    let before = one.session.view.folder;
    one.apply_prefs(serde_json::from_str(&text).unwrap());
    assert_eq!(one.session.view.unified, None);
    assert_eq!(one.session.view.folder, before);
    // A kind All Accounts doesn't offer, or a value nobody wrote: read, and left out; the rest of
    // the settings stand.
    for unified in [json!("junk"), json!("bogus"), json!(12), json!({"a": 1})] {
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["view"]["unified"] = unified.clone();
        value["view"]["conversations"] = json!(true);
        let ui: UiState = serde_json::from_value(value).unwrap_or_else(|e| panic!("{unified}: {e}"));
        let mut next = two_accounts();
        next.apply_prefs(ui);
        assert_eq!(next.session.view.unified, None, "{unified}");
        assert!(next.session.view.conversations, "{unified}: the rest is read");
    }
}
