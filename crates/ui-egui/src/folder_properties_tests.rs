// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder Properties (#314) in the window: opened for a folder and for an account, sorted by its
//! columns, and a folder's name going to that folder.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use super::{FolderProperties, Sort};
use crate::dialogs::Dialog;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// The sample mailbox, with a folder of the owner's own under the first account's Inbox; the
/// account, its Inbox and that folder.
fn app() -> (MailApp, Id, Id, Id) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let account = app.session.mailbox.accounts[0].id;
    let inbox = app.session.mailbox.folder_of_kind(account, FolderKind::Inbox).unwrap();
    let reports = app.session.run("folder.new", &json!({"name": "Quarterly reports", "parent": inbox})).unwrap()["id"].as_u64().unwrap();
    (app, account, inbox, reports)
}

fn open(app: &MailApp) -> &FolderProperties {
    match app.dialog.as_ref() {
        Some(Dialog::FolderProperties(d)) => d,
        _ => panic!("Folder Properties isn't open"),
    }
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(4);
    harness
}

#[test]
fn the_owner_gets_a_table_for_a_folder_and_a_script_gets_the_answer() {
    let (mut a, _, inbox, reports) = app();
    assert_eq!(a.run("folder.properties", json!({"id": inbox})).unwrap()["pending"], "folderProperties");
    let d = open(&a);
    assert_eq!(d.title, "Inbox Properties");
    assert_eq!(d.rows.first().map(|r| r.id), Some(inbox));
    let row = d.rows.iter().find(|r| r.id == reports).unwrap();
    assert_eq!((row.depth, row.name.as_str(), row.path.as_str()), (1, "Quarterly reports", "Inbox › Quarterly reports"));
    let held: u64 = d.rows.iter().map(|r| r.own.messages).sum();
    assert_eq!(d.total.messages, held);
    assert_eq!(d.rows[0].own.messages, a.session.mailbox.count(inbox) as u64);
    // Without an id, the folder in view.
    a.dialog = None;
    a.run("folder.select", json!({"id": reports})).unwrap();
    a.run("folder.properties", json!({})).unwrap();
    assert_eq!(open(&a).title, "Quarterly reports Properties");
    // A script or an agent gets the answer, and no dialog.
    a.dialog = None;
    let answer = a.execute("folder.properties", json!({"id": inbox})).unwrap();
    assert_eq!(answer["folder"], inbox);
    assert!(a.dialog.is_none());
    // Nothing to open for what isn't there.
    assert!(a.execute("ui.dialog", json!({"name": "folderProperties", "id": 999_999})).is_err());
    assert!(a.execute("ui.dialog", json!({"name": "folderProperties", "account": 999_999})).is_err());
    assert!(a.dialog.is_none());
}

#[test]
fn an_account_lists_all_its_folders() {
    let (mut a, account, inbox, _) = app();
    a.run("folder.properties", json!({"account": account})).unwrap();
    let d = open(&a);
    let name = a.session.mailbox.account(account).unwrap().name.clone();
    assert_eq!(d.title, format!("{name} Properties"));
    assert_eq!(d.rows.len(), a.session.mailbox.folders.iter().filter(|f| f.account == account).count());
    assert_eq!(d.rows[0].id, inbox);
    let mine = a.session.mailbox.messages.values().filter(|m| m.account == account).count() as u64;
    assert_eq!(d.total.messages, mine);
}

#[test]
fn the_columns_sort_largest_first_and_a_second_click_turns_them_round() {
    let (mut a, account, _, _) = app();
    a.run("folder.properties", json!({"account": account})).unwrap();
    let Some(Dialog::FolderProperties(d)) = a.dialog.as_mut() else { panic!("not open") };
    let pane: Vec<Id> = d.sorted().iter().map(|r| r.id).collect();
    assert_eq!(pane, d.rows.iter().map(|r| r.id).collect::<Vec<_>>());
    for (sort, key) in [
        (Sort::Messages, (|r: &super::Row| r.own.messages) as fn(&super::Row) -> u64),
        (Sort::Unread, |r| r.own.unread),
        (Sort::Size, |r| r.own.size),
        (Sort::TotalMessages, |r| r.total.messages),
        (Sort::TotalSize, |r| r.total.size),
        (Sort::Server, |r| r.server.map_or(0, |(messages, _)| messages)),
    ] {
        d.sort_by(sort);
        let shown: Vec<u64> = d.sorted().iter().map(|r| key(r)).collect();
        assert!(shown.windows(2).all(|w| w[0] >= w[1]), "{sort:?}: {shown:?}");
        d.sort_by(sort);
        let shown: Vec<u64> = d.sorted().iter().map(|r| key(r)).collect();
        assert!(shown.windows(2).all(|w| w[0] <= w[1]), "{sort:?} turned round: {shown:?}");
    }
    d.sort_by(Sort::Folder);
    assert_eq!(d.sorted().iter().map(|r| r.id).collect::<Vec<_>>(), pane);
}

/// What the folders under a folder hold has columns of its own: the messages, then their size,
/// each lined up on its own.
#[test]
fn the_messages_and_the_size_with_subfolders_have_columns_of_their_own() {
    let (mut a, _, inbox, _) = app();
    a.run("folder.properties", json!({"id": inbox})).unwrap();
    let (own, total) = (open(&a).rows[0].own, open(&a).rows[0].total);
    assert!(total.messages > own.messages && total.size > own.size, "the sample's Inbox has mail in a folder under it");
    let h = window(a);
    for heading in ["Total messages", "Total size"] {
        assert!(h.query_by_label(heading).is_some(), "{heading}");
    }
    assert!(h.query_by_label(&total.messages.to_string()).is_some());
    assert!(h.query_by_label(&super::size(total.size)).is_some());
}

#[test]
fn a_folders_name_goes_to_it_and_closes_the_table() {
    let (mut a, _, inbox, reports) = app();
    a.run("folder.properties", json!({"id": inbox})).unwrap();
    let mut h = window(a);
    assert!(h.query_all_by_label_contains("Inbox Properties").next().is_some());
    h.get_by_label_contains("Quarterly reports").click();
    h.run_steps(4);
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().session.view.folder, reports);
}

fn right_click(h: &mut Harness<'static, MailApp>, pos: egui::Pos2) {
    h.event(egui::Event::PointerMoved(pos));
    h.step();
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE });
        h.step();
    }
    h.run_steps(2);
}

/// Properties… in the right-click menu of a folder and of an account's heading opens the table
/// for it; the Favorites heading has no such menu.
#[test]
fn properties_opens_from_a_folders_and_an_accounts_right_click_menu() {
    for heading in [false, true] {
        let (a, account, _, reports) = app();
        let mut h = window(a);
        let rect = if heading { h.state().hit.rect("folderHeading", account) } else { h.state().hit.rect("folder", reports) };
        right_click(&mut h, rect.unwrap_or_else(|| panic!("no row on screen (heading: {heading})")).center());
        h.get_by_label("Properties…").click();
        h.run_steps(4);
        let d = open(h.state());
        if heading {
            assert_eq!(d.rows.len(), h.state().session.mailbox.folders.iter().filter(|f| f.account == account).count());
        } else {
            assert_eq!(d.title, "Quarterly reports Properties");
            assert_eq!(d.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [reports]);
        }
    }
}

/// Every control the dialog and the account heading's menu add has an accessible name, for a
/// screen reader (what the window had without them is left as it was).
#[test]
fn what_folder_properties_adds_has_names() {
    let (a, account, _, _) = app();
    let mut h = window(a);
    let before = h.unnamed_widgets();
    let added = |h: &Harness<'static, MailApp>| h.unnamed_widgets().into_iter().filter(|w| !before.contains(w)).collect::<Vec<_>>();
    let heading = h.state().hit.rect("folderHeading", account).unwrap();
    right_click(&mut h, heading.center());
    assert_eq!(added(&h), Vec::<String>::new(), "the account heading's menu");
    h.get_by_label("Properties…").click();
    h.run_steps(4);
    assert!(open(h.state()).rows.len() > 1);
    assert_eq!(added(&h), Vec::<String>::new(), "the dialog");
}
