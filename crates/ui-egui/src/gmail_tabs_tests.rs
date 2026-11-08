// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Gmail's categories as tabs above a Gmail Inbox's list (#911).

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable};
use mailcraft_engine::Session;
use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{FolderKind, GmailCategory, Id, Message};
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Harness = egui_kittest::Harness<'static, MailApp>;

use crate::nav::tests::window;

/// The window with a Gmail account whose Inbox has a message in Primary, Social and Promotions
/// each; the Inbox's id.
fn gmail_window() -> (Harness, Id) {
    let mut harness = window();
    let app = harness.state_mut();
    let servers = json!({"email": "ada@mail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}});
    let account = app.execute("account.add", servers).unwrap()["id"].as_u64().unwrap();
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let new: Vec<Message> = [GmailCategory::Primary, GmailCategory::Social, GmailCategory::Promotions]
        .into_iter()
        .zip(1..)
        .map(|(category, uid)| Message { uid, subject: format!("Message {uid}"), gmail_category: Some(category), ..Default::default() })
        .collect();
    let delta =
        FolderDelta { id: Some(inbox), remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 3, new, ..Default::default() };
    app.execute("sync.apply", json!({"result": SyncResult { account, folders: vec![delta], ..Default::default() }})).unwrap();
    app.execute("folder.select", json!({"id": inbox})).unwrap();
    harness.run_steps(4);
    (harness, inbox)
}

fn tabs(harness: &Harness) -> Vec<String> {
    harness
        .query_all_by_role(Role::Tab)
        .filter_map(|n| n.accesskit_node().label())
        .filter(|l| ["Primary", "Social", "Promotions", "Updates", "Forums"].iter().any(|c| l.starts_with(c)))
        .collect()
}

fn rows(harness: &Harness) -> usize {
    harness
        .query_all_by_role(Role::TreeItem)
        .filter(|n| n.accesskit_node().label().is_some_and(|l| l.starts_with("Social") || l.starts_with("Forums")))
        .count()
}

#[test]
fn the_categories_are_folders_until_tabs_are_chosen() {
    let (mut harness, inbox) = gmail_window();
    assert!(tabs(&harness).is_empty());
    assert_eq!(rows(&harness), 2, "Social and Forums under the Inbox");
    harness.state_mut().execute("view.gmailCategories", json!({"value": "tabs"})).unwrap();
    harness.run_steps(3);
    // Five tabs, each read out with its unread count; no rows under the Inbox.
    assert_eq!(tabs(&harness), ["Primary, 1 unread", "Social, 1 unread", "Promotions, 1 unread", "Updates", "Forums"]);
    assert_eq!(rows(&harness), 0);
    // The whole Inbox: no tab chosen.
    assert!(
        harness
            .query_all_by_role(Role::Tab)
            .filter(|n| n.accesskit_node().label().is_some_and(|l| tabs(&harness).contains(&l)))
            .all(|n| n.accesskit_node().is_selected() != Some(true))
    );
    // A tab does what the folder pane's row does; the chosen one again is the whole Inbox.
    harness.get_by_role_and_label(Role::Tab, "Social, 1 unread").click();
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.gmail, Some((inbox, GmailCategory::Social)));
    assert_eq!(harness.state_mut().session.rows().len(), 1);
    assert_eq!(harness.get_by_role_and_label(Role::Tab, "Social, 1 unread").accesskit_node().is_selected(), Some(true));
    harness.get_by_role_and_label(Role::Tab, "Social, 1 unread").click();
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.gmail, None);
    assert_eq!(harness.state_mut().session.rows().len(), 3);
}

/// Tab reaches a tab, the arrow keys go to the next, and Enter chooses it.
#[test]
fn a_tab_is_reached_and_chosen_with_the_keyboard() {
    let (mut harness, inbox) = gmail_window();
    harness.state_mut().execute("view.gmailCategories", json!({"value": "tabs"})).unwrap();
    harness.run_steps(3);
    harness.get_by_role_and_label(Role::Tab, "Social, 1 unread").focus();
    harness.run_steps(2);
    harness.key_press(egui::Key::ArrowRight);
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.gmail, Some((inbox, GmailCategory::Promotions)));
    assert_eq!(mailcraft_engine::gmail::in_view(&harness.state().session), Some(GmailCategory::Promotions));
    harness.key_press(egui::Key::ArrowLeft);
    harness.run_steps(2);
    harness.key_press(egui::Key::Space);
    harness.run_steps(3);
    assert_eq!(harness.state().session.view.gmail, Some((inbox, GmailCategory::Social)));
}

/// Only over the list of a Gmail Inbox with categories: not over another folder's, nor another
/// account's Inbox.
#[test]
fn tabs_are_only_over_a_gmail_inbox() {
    let (mut harness, inbox) = gmail_window();
    harness.state_mut().execute("view.gmailCategories", json!({"value": "tabs"})).unwrap();
    harness.run_steps(3);
    assert_eq!(tabs(&harness).len(), 5);
    let app = harness.state_mut();
    let account = app.session.mailbox.folder(inbox).map(|f| f.account).unwrap();
    let sent = app.session.mailbox.folder_of_kind(account, FolderKind::Sent).unwrap();
    app.execute("folder.select", json!({"id": sent})).unwrap();
    harness.run_steps(3);
    assert!(tabs(&harness).is_empty());
    let app = harness.state_mut();
    let other = app.session.mailbox.accounts.iter().find(|a| a.id != account).map(|a| a.id).unwrap();
    let theirs = app.session.mailbox.folder_of_kind(other, FolderKind::Inbox).unwrap();
    app.execute("folder.select", json!({"id": theirs})).unwrap();
    harness.run_steps(3);
    assert!(tabs(&harness).is_empty());
}

/// The choice is kept in the window's settings with the rest of the view, and View offers it only
/// with a Gmail account.
#[test]
fn the_choice_is_kept_and_offered_only_with_gmail() {
    let offered = |app: &MailApp| crate::ribbon::describe_tab(app, "View").items().any(|item| item.key == "gmailCategories");
    let plain = window();
    assert!(!offered(plain.state()));
    let (mut harness, _) = gmail_window();
    assert!(offered(harness.state()));
    harness.state_mut().execute("view.gmailCategories", json!({"value": "tabs"})).unwrap();
    let saved = harness.state().prefs();
    let text = serde_json::to_string(&saved).unwrap();
    assert!(text.contains("\"gmailTabs\":true"), "{text}");
    let mut fresh = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    assert!(!fresh.session.view.gmail_tabs);
    fresh.apply_prefs(serde_json::from_str(&text).unwrap());
    assert!(fresh.session.view.gmail_tabs);
}
