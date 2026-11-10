// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Moving mail between Gmail's categories in the window (#911): the message menu's Move to
//! Category, and messages let go over a category's row or tab. Moves ship switched off; these
//! tests switch them on, but for the ones that hold the window to offering nothing while off.

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT as _, Queryable};
use mailcraft_model::sync::{FolderDelta, SyncResult};
use mailcraft_model::{FolderKind, GmailCategory, Id, Message, OAuth, OAuthProvider};
use serde_json::json;

use crate::MailApp;
use crate::nav::tests::window;

type Harness = egui_kittest::Harness<'static, MailApp>;

/// The window on a Gmail account's Inbox, signing in with Google (`google`) or an app password,
/// with moves switched on (`on`): a message in Primary (1), Social (2) and Promotions (3). The
/// account's id and the Inbox's.
fn gmail_window(google: bool, on: bool) -> (Harness, Id, Id) {
    let mut harness = window();
    let app = harness.state_mut();
    app.session.gmail_category_moves = on;
    let servers = json!({"email": "ada@mail.example", "incoming": {"host": "imap.gmail.com"}, "outgoing": {"host": "smtp.gmail.com"}});
    let account = app.execute("account.add", servers).unwrap()["id"].as_u64().unwrap();
    if google {
        let a = app.session.mailbox.accounts.iter_mut().find(|a| a.id == account).unwrap();
        a.oauth = Some(OAuth { provider: OAuthProvider::Google, ..Default::default() });
    }
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let new: Vec<Message> = [GmailCategory::Primary, GmailCategory::Social, GmailCategory::Promotions]
        .into_iter()
        .zip(1..)
        .map(|(category, uid)| Message {
            uid,
            subject: format!("Message {uid}"),
            message_id: format!("<{uid}@sender.example>"),
            gmail_category: Some(category),
            gmail_id: 7_000 + u64::from(uid),
            ..Default::default()
        })
        .collect();
    let delta =
        FolderDelta { id: Some(inbox), remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 3, new, ..Default::default() };
    app.execute("sync.apply", json!({"result": SyncResult { account, folders: vec![delta], ..Default::default() }})).unwrap();
    app.execute("folder.select", json!({"id": inbox})).unwrap();
    harness.run_steps(4);
    (harness, account, inbox)
}

fn message(harness: &Harness, account: Id, uid: u32) -> Message {
    harness.state().session.mailbox.messages.values().find(|m| m.account == account && m.uid == uid).map(|m| m.as_ref().clone()).unwrap()
}

fn right_click(harness: &mut Harness, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    harness.step();
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        harness.step();
    }
    harness.run_steps(3);
}

/// The menu of message `id`'s row.
fn menu_of(harness: &mut Harness, id: Id) {
    let row = harness.state().hit.rect("message", id).unwrap();
    right_click(harness, row.center());
}

fn has_label(harness: &Harness, label: &str) -> bool {
    harness.query_all_by_label(label).next().is_some()
}

/// The pointer pressed at `from`, moved to `to` and let go there.
fn drag(harness: &mut Harness, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    harness.input_mut().events.extend([egui::Event::PointerMoved(from), button(from, true)]);
    harness.run_steps(2);
    let mid = from + (to - from) / 2.0;
    harness.input_mut().events.push(egui::Event::PointerMoved(mid));
    harness.run_steps(2);
    harness.input_mut().events.push(egui::Event::PointerMoved(to));
    harness.run_steps(2);
    harness.input_mut().events.push(button(to, false));
    harness.run_steps(3);
}

#[test]
fn the_message_menu_moves_to_one_of_the_five() {
    let (mut harness, account, _) = gmail_window(true, true);
    let social = message(&harness, account, 2);
    menu_of(&mut harness, social.id);
    harness.get_by_role_and_label(Role::Button, "Move to Category").click();
    harness.run_steps(3);
    for name in ["Primary", "Social", "Promotions", "Updates", "Forums"] {
        assert!(harness.query_all_by_role_and_label(Role::Button, name).next().is_some(), "{name}");
    }
    harness.get_by_role_and_label(Role::Button, "Forums").click();
    harness.run_steps(3);
    let moved = message(&harness, account, 2);
    assert_eq!((moved.gmail_category, moved.gmail_synced), (Some(GmailCategory::Forums), Some(GmailCategory::Social)));
    // Waiting for the next Send/Receive, by Gmail's id.
    let request = mailcraft_engine::cmd::account::sync_request(&harness.state().session, account).unwrap();
    assert_eq!(request.gmail_moves.iter().map(|m| (m.id, m.category)).collect::<Vec<_>>(), [(7_002, GmailCategory::Forums)]);
}

#[test]
fn an_app_password_account_has_the_item_greyed_with_why() {
    let (mut harness, account, _) = gmail_window(false, true);
    let social = message(&harness, account, 2);
    menu_of(&mut harness, social.id);
    let item = harness.get_by_role_and_label(Role::Button, "Move to Category");
    assert!(item.accesskit_node().is_disabled());
    assert!(has_label(&harness, "Needs Google sign-in"));
    assert_eq!(message(&harness, account, 2).gmail_category, Some(GmailCategory::Social));
}

#[test]
fn switched_off_the_menu_has_no_such_item_and_nothing_takes_a_drop() {
    let (mut harness, account, inbox) = gmail_window(true, false);
    let social = message(&harness, account, 2);
    menu_of(&mut harness, social.id);
    assert!(!has_label(&harness, "Move to Category"));
    harness.key_press(egui::Key::Escape);
    harness.run_steps(3);
    // Let go over Primary's row: it stays where it was.
    let from = harness.state().hit.rect("message", social.id).unwrap().center();
    let to = harness.state().hit.rect("gmailPrimary", inbox).unwrap().center();
    drag(&mut harness, from, to);
    let after = message(&harness, account, 2);
    assert_eq!((after.gmail_category, after.gmail_synced.is_some_and(|s| Some(s) != after.gmail_category)), (Some(GmailCategory::Social), false));
    assert!(mailcraft_engine::cmd::account::sync_request(&harness.state().session, account).unwrap().gmail_moves.is_empty());
}

#[test]
fn switched_off_command_search_finds_no_move_to_category() {
    let found = |on: bool| {
        let (mut harness, _, _) = gmail_window(true, on);
        let query = "move to cat";
        harness.state_mut().dialog = Some(crate::dialogs::Dialog::Commands { query: query.into(), selected: 0 });
        harness.run_steps(3);
        assert!(has_label(&harness, "Search Commands"));
        let app = harness.state();
        crate::dialogs::all_commands(app)
            .into_iter()
            .any(|c| c.0 == "mail.setGmailCategory" && crate::dialogs::score(query, &c.1, &c.2, &c.0).is_some())
    };
    assert!(!found(false), "not even greyed while it waits for Google's review");
    assert!(found(true));
}

#[test]
fn messages_let_go_over_a_categorys_row_move_to_it() {
    let (mut harness, account, inbox) = gmail_window(true, true);
    let promotions = message(&harness, account, 3);
    let from = harness.state().hit.rect("message", promotions.id).unwrap().center();
    let to = harness.state().hit.rect("gmailPrimary", inbox).unwrap().center();
    drag(&mut harness, from, to);
    assert_eq!(message(&harness, account, 3).gmail_category, Some(GmailCategory::Primary));
    // And still in the Inbox: no folder move.
    assert_eq!(message(&harness, account, 3).folder, inbox);
}

#[test]
fn messages_let_go_over_a_tab_move_to_it() {
    let (mut harness, account, _) = gmail_window(true, true);
    harness.state_mut().execute("view.gmailCategories", json!({"value": "tabs"})).unwrap();
    harness.run_steps(3);
    let primary = message(&harness, account, 1);
    let from = harness.state().hit.rect("message", primary.id).unwrap().center();
    // The fifth tab: Forums.
    let to = harness.state().hit.rect("gmailTab", 4).unwrap().center();
    drag(&mut harness, from, to);
    assert_eq!(message(&harness, account, 1).gmail_category, Some(GmailCategory::Forums));
}

#[test]
fn mail_of_no_gmail_inbox_has_no_such_item() {
    let (mut harness, account, _) = gmail_window(true, true);
    // A message of the Gmail account filed out of its Inbox: in none of Gmail's categories.
    let filed = message(&harness, account, 2).id;
    let app = harness.state_mut();
    let archive = app.session.mailbox.add_folder(account, None, "Kept").unwrap();
    app.execute("mail.move", json!({"ids": [filed], "folder": archive})).unwrap();
    app.execute("folder.select", json!({"id": archive})).unwrap();
    harness.run_steps(4);
    menu_of(&mut harness, filed);
    assert!(has_label(&harness, "Move to Folder") || has_label(&harness, "Move"), "the menu is open");
    assert!(!has_label(&harness, "Move to Category"));
}
