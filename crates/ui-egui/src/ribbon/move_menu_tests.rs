// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Move to Folder and Copy to Folder list every mailbox's folders: the account in view's first,
//! another's under its name, and a search finds it by its mailbox too (#451).

use egui_kittest::kittest::Queryable as _;
use mailcraft_engine::Session;
use mailcraft_model::Id;
use serde_json::json;

use super::{FolderScope, folder_menu_all, folder_rows};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The menu drawn on its own: the app, and the folder chosen.
type Picker = egui_kittest::Harness<'static, (MailApp, Option<Id>)>;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

/// The menu every account's folders are drawn in, as Move to Folder and Copy to Folder draw it.
fn picker_all(app: MailApp) -> Picker {
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(400.0, 500.0)).build_ui_state(
            |ui, (app, picked): &mut (MailApp, Option<Id>)| {
                folder_menu_all(ui, app, |_, id| *picked = Some(id));
            },
            (app, None),
        );
    h.run_steps(3);
    h
}

fn type_into(h: &mut Picker, text: &str) {
    h.event(egui::Event::Text(text.to_string()));
    h.run_steps(3);
}

fn press(h: &mut Picker, key: egui::Key) {
    h.key_press(key);
    h.run_steps(3);
}

#[test]
fn every_mailboxs_folders_are_in_the_move_menu() {
    let mut a = app();
    let local = a.session.mailbox.default_account().map(|x| x.id).unwrap();
    let other = a.execute("account.add", json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let archive = a.session.mailbox.folder_of_kind(other, mailcraft_model::FolderKind::Archive).unwrap();
    let (rows, many) = folder_rows(&a, FolderScope::EveryAccount);
    assert!(many, "two mailboxes are listed");
    assert!(rows.iter().any(|row| row.id == archive), "another mailbox's folder is listed");
    assert!(rows.iter().any(|row| row.heading.as_deref() == Some("owner@example.org")), "its account is a heading");
    let first = &rows[0];
    assert_eq!(a.session.mailbox.folder(first.id).map(|f| f.account), Some(local), "the mailbox in view's folders come first");

    // Typing the mailbox's address finds a folder in it, and Enter chooses that folder.
    let mut h = picker_all(a);
    type_into(&mut h, "owner@example.org");
    press(&mut h, egui::Key::Enter);
    let chosen = h.state().1.expect("a folder of that mailbox is chosen");
    assert_eq!(h.state().0.session.mailbox.folder(chosen).map(|f| f.account), Some(other));
}

/// The folders are read once, when the menu opens, and kept while it stays open: a folder added
/// meanwhile doesn't appear until it is opened again. Dismissing it without choosing a folder
/// and opening it again a single pass later reads them again (#1176).
#[test]
fn the_menu_keeps_its_rows_while_it_stays_open_and_reads_them_again_when_reopened() {
    let a = app();
    let account = a.session.mailbox.default_account().map(|x| x.id).unwrap();
    let mut h =
        egui_kittest::Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(400.0, 500.0)).build_ui_state(
            |ui, (app, picked): &mut (MailApp, Option<Id>)| {
                ui.menu_button("Folders", |ui| folder_menu_all(ui, app, |_, id| *picked = Some(id)));
            },
            (a, None),
        );
    h.run_steps(3);
    h.get_by_label("Folders").click_accesskit();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Zebra").is_none(), "no such folder yet");
    h.state_mut().0.session.mailbox.add_folder(account, None, "Zebra").unwrap();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Zebra").is_none(), "the menu keeps the rows it opened with");
    // Dismissed without a folder chosen: one pass with the menu shut, then opened again.
    h.get_by_label("Folders").click_accesskit();
    h.run_steps(1);
    assert!(h.query_by_label_contains("Zebra").is_none() && h.query_by_label_contains("Inbox").is_none(), "the menu is shut");
    h.get_by_label("Folders").click_accesskit();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Zebra").is_some(), "a menu opened again reads the folders again");
}
