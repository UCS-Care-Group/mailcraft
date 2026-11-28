// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! All Accounts at the top of the folder pane (#625): only with two accounts, and a click on All
//! Inboxes lists every account's Inbox, its row the one highlighted.

use super::tests::window;
use super::*;

fn click(harness: &mut egui_kittest::Harness<'static, MailApp>, pos: Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(2);
}

#[test]
fn all_accounts_shows_with_two_accounts_and_all_inboxes_lists_both() {
    let mut harness = window();
    // One account: no All Accounts.
    assert!(harness.state().hit.rect("folderHeading", ALL_ACCOUNTS).is_none());
    let work = harness.state_mut().session.mailbox.add_account(mailcraft_model::Account {
        name: "Work".into(),
        email: "me@work.example".into(),
        ..Default::default()
    });
    let work_inbox = harness.state().session.mailbox.folder_of_kind(work, FolderKind::Inbox).unwrap();
    let m = mailcraft_model::Message { folder: work_inbox, subject: "From work".into(), date: 1_791_990_000, ..Default::default() };
    harness.state_mut().session.mailbox.add_message(m).unwrap();
    harness.run_steps(3);
    let heading = harness.state().hit.rect("folderHeading", ALL_ACCOUNTS).expect("All Accounts with two accounts");
    let mine = harness.state().session.mailbox.accounts.first().unwrap().id;
    assert!(heading.max.y <= harness.state().hit.rect("folderHeading", mine).unwrap().min.y, "at the top");
    // All Inboxes is its first row.
    let row = harness.state().hit.rect("unifiedFolder", 0).unwrap();
    click(&mut harness, row.center());
    let app = harness.state_mut();
    assert_eq!(mailcraft_engine::unified::of(&app.session), Some(FolderKind::Inbox));
    let rows = app.session.rows();
    let accounts: std::collections::HashSet<Id> = rows.iter().filter_map(|r| app.session.mailbox.message(r.id)).map(|m| m.account).collect();
    assert_eq!(accounts.len(), 2, "both accounts' mail in one list");
    assert_eq!(app.title_stem(), "All Inboxes");
}

/// Empty Folder is greyed over every account's Deleted Items (#625): the ribbon can't say whose;
/// a folder's own menu names it, and the command empties a named folder.
#[test]
fn empty_folder_is_greyed_over_all_deleted_items() {
    let mut harness = window();
    let app = harness.state_mut();
    app.session.mailbox.add_account(mailcraft_model::Account { name: "Work".into(), email: "me@work.example".into(), ..Default::default() });
    app.execute("view.allAccounts", serde_json::json!({"kind": "deleted"})).unwrap();
    assert!(!app.enabled("folder.empty"));
    // Nothing is filled in for it: run without a folder, it refuses.
    assert!(app.execute("folder.empty", serde_json::json!({})).is_err());
}
