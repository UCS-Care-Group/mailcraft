// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Folder Properties (#314) in the window: General for a folder and for an account, the other
//! tabs, and Folder Size over them: sorted by its columns, a folder's name going to that folder,
//! Escape and Enter closing it before the tabs.

use std::collections::HashMap;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use super::{FolderProperties, Sort, Tab};
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

fn open_mut(app: &mut MailApp) -> &mut FolderProperties {
    match app.dialog.as_mut() {
        Some(Dialog::FolderProperties(d)) => d,
        _ => panic!("Folder Properties isn't open"),
    }
}

fn window(app: MailApp) -> Harness<'static, MailApp> {
    window_of(app, egui::vec2(1280.0, 900.0))
}

fn window_of(app: MailApp, size: egui::Vec2) -> Harness<'static, MailApp> {
    let mut harness = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(size).build_ui_state(
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

fn click(h: &mut Harness<'static, MailApp>, label: &str) {
    h.get_by_label(label).click();
    h.run_steps(3);
}

fn key(h: &mut Harness<'static, MailApp>, key: egui::Key) {
    h.key_press(key);
    h.run_steps(3);
}

fn heading(app: &MailApp, account: Id) -> String {
    crate::nav::heading_name(app, app.session.mailbox.account(account).unwrap())
}

/// Properties opens on General: the folder's name, what kind of folder it is, where it is, and
/// what it holds itself. Folder Size's table is ready for its button.
#[test]
fn the_owner_gets_general_for_a_folder_and_a_script_gets_the_answer() {
    let (mut a, account, inbox, reports) = app();
    assert_eq!(a.run("folder.properties", json!({"id": inbox})).unwrap()["pending"], "folderProperties");
    let account_name = heading(&a, account);
    let unread = a.session.mailbox.messages.values().filter(|m| m.folder == inbox && !m.read).count() as u64;
    let d = open(&a);
    assert_eq!((d.title.as_str(), d.tab, d.size_open), ("Inbox Properties", Tab::General, false));
    let g = &d.general;
    assert_eq!((g.name.as_str(), g.kind.as_str(), g.location.as_deref()), ("Inbox", "Mail folder: the account's Inbox", Some(account_name.as_str())));
    assert_eq!((g.messages, g.unread), (a.session.mailbox.count(inbox) as u64, unread));
    assert_eq!(d.size.rows.first().map(|r| r.id), Some(inbox));
    let row = d.size.rows.iter().find(|r| r.id == reports).unwrap();
    assert_eq!((row.depth, row.name.as_str(), row.path.as_str()), (1, "Quarterly reports", "Inbox › Quarterly reports"));
    let held: u64 = d.size.rows.iter().map(|r| r.own.messages).sum();
    assert_eq!(d.size.total.messages, held);
    // Without an id, the folder in view: one of the owner's own, under the Inbox.
    a.dialog = None;
    a.run("folder.select", json!({"id": reports})).unwrap();
    a.run("folder.properties", json!({})).unwrap();
    let d = open(&a);
    assert_eq!(d.title, "Quarterly reports Properties");
    assert_eq!((d.general.kind.as_str(), d.general.location.clone()), ("Mail folder", Some(format!("{account_name} › Inbox"))));
    // A script or an agent gets the answer, and no dialog.
    a.dialog = None;
    let answer = a.execute("folder.properties", json!({"id": inbox})).unwrap();
    assert_eq!(answer["folder"], inbox);
    assert!(a.dialog.is_none());
    // Nothing to open for what isn't there.
    assert!(a.execute("ui.dialog", json!({"name": "folderProperties", "id": 999_999})).is_err());
    assert!(a.execute("ui.dialog", json!({"name": "folderProperties", "account": 999_999})).is_err());
    assert!(a.dialog.is_none());
    // Run by the owner for what isn't there, the engine's refusal is said in the status bar and,
    // as for any command the owner ran by hand, in a message box.
    a.by_hand = true;
    let refused = a.run("folder.properties", json!({"id": 999_999})).unwrap_err();
    assert!(a.dialog.is_none());
    assert_eq!(a.status_msg.as_ref().map(|(m, _)| m.as_str()), Some(refused.as_str()));
    assert_eq!(a.refused.as_deref(), Some(refused.as_str()));
}

/// An account's Properties: General says what kind of account it is (its servers with it) and
/// what all of its folders hold, with no Location (nothing is above an account), and Folder Size
/// lists them all.
#[test]
fn an_account_has_general_and_all_its_folders() {
    let (mut a, account, inbox, _) = app();
    a.run("folder.properties", json!({"account": account})).unwrap();
    let name = heading(&a, account);
    let acc = a.session.mailbox.account(account).unwrap();
    let kind = crate::backstage::account_detail(&a, acc);
    let mine = a.session.mailbox.messages.values().filter(|m| m.account == account).count() as u64;
    let folders = a.session.mailbox.folders.iter().filter(|f| f.account == account).count();
    let d = open(&a);
    assert_eq!(d.title, format!("{name} Properties"));
    assert_eq!((d.general.name.as_str(), d.general.kind.as_str(), d.general.location.as_deref()), (name.as_str(), kind.as_str(), None));
    assert_eq!((d.general.messages, d.size.total.messages), (mine, mine));
    assert_eq!(d.size.rows.len(), folders);
    assert_eq!(d.size.rows[0].id, inbox);
    let h = window(a);
    assert!(h.query_by_label("Type").is_some() && h.query_by_label("Location").is_none());
}

/// Each column sorts largest first and turns round on a second click; among equals the folder
/// pane's order holds both ways. The pane's order turns round too.
#[test]
fn the_columns_sort_largest_first_and_a_second_click_turns_them_round() {
    let (mut a, account, _, _) = app();
    a.run("folder.properties", json!({"account": account})).unwrap();
    let d = &mut open_mut(&mut a).size;
    // The sample has no counts from a server: two folders get the same count, one a larger one.
    assert!(d.rows.len() > 4);
    for (i, count) in [(1, 5), (3, 5), (4, 9)] {
        d.rows[i].server = Some((count, 0));
    }
    d.server = true;
    let pane: Vec<Id> = d.sorted().iter().map(|r| r.id).collect();
    assert_eq!(pane, d.rows.iter().map(|r| r.id).collect::<Vec<_>>());
    let at: HashMap<Id, usize> = pane.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    for (sort, key) in [
        (Sort::Messages, (|r: &super::Row| r.own.messages) as fn(&super::Row) -> u64),
        (Sort::Unread, |r| r.own.unread),
        (Sort::Size, |r| r.own.size),
        (Sort::TotalMessages, |r| r.total.messages),
        (Sort::TotalSize, |r| r.total.size),
        (Sort::Server, |r| r.server.map_or(0, |(messages, _)| messages)),
    ] {
        for turned in [false, true] {
            d.sort_by(sort);
            let shown = d.sorted();
            for w in shown.windows(2) {
                let (k0, k1) = (key(w[0]), key(w[1]));
                assert!(if turned { k0 <= k1 } else { k0 >= k1 }, "{sort:?} turned {turned}: {k0} then {k1}");
                if k0 == k1 {
                    assert!(at[&w[0].id] < at[&w[1].id], "{sort:?} turned {turned}: equals out of the pane's order");
                }
            }
        }
    }
    d.sort_by(Sort::Folder);
    assert_eq!(d.sorted().iter().map(|r| r.id).collect::<Vec<_>>(), pane);
    d.sort_by(Sort::Folder);
    assert_eq!(d.sorted().iter().rev().map(|r| r.id).collect::<Vec<_>>(), pane);
}

/// General in the window, and every other tab saying it is still to come.
#[test]
fn general_and_the_tabs_still_to_come() {
    let (mut a, _, inbox, _) = app();
    a.run("folder.properties", json!({"id": inbox})).unwrap();
    let mut h = window(a);
    for label in ["Name", "Type", "Location", "Messages", "Unread", "Mail folder: the account's Inbox", "Folder Size…"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    for tab in [Tab::AutoArchive, Tab::Policy, Tab::Permissions, Tab::Synchronization] {
        click(&mut h, tab.label());
        assert_eq!(open(h.state()).tab, tab);
        assert!(h.query_by_label("Pending development").is_some(), "{tab:?}");
        assert!(h.query_by_label("Folder Size…").is_none(), "{tab:?}");
    }
    click(&mut h, "General");
    assert!(h.query_by_label("Folder Size…").is_some() && h.query_by_label("Pending development").is_none());
}

/// Folder Size… opens the table over the tabs; Escape, Enter or its Close close it alone, and
/// the next Escape closes Properties.
#[test]
fn folder_size_opens_over_general_and_closes_before_it() {
    let (mut a, _, inbox, _) = app();
    a.run("folder.properties", json!({"id": inbox})).unwrap();
    let mut h = window(a);
    for close in [egui::Key::Escape, egui::Key::Enter] {
        click(&mut h, "Folder Size…");
        assert!(open(h.state()).size_open);
        assert!(h.query_by_label("Total messages").is_some(), "the table is drawn");
        key(&mut h, close);
        assert!(!open(h.state()).size_open, "{close:?} closes Folder Size");
        assert!(h.query_by_label("Total messages").is_none());
    }
    click(&mut h, "Folder Size…");
    h.get_all_by_label("Close").last().unwrap().click();
    h.run_steps(3);
    assert!(!open(h.state()).size_open, "its own Close");
    key(&mut h, egui::Key::Escape);
    assert!(h.state().dialog.is_none());
}

/// What the folders under a folder hold has columns of its own: the messages, then their size,
/// each lined up on its own.
#[test]
fn the_messages_and_the_size_with_subfolders_have_columns_of_their_own() {
    let (mut a, _, inbox, _) = app();
    a.run("folder.properties", json!({"id": inbox})).unwrap();
    let (own, total) = (open(&a).size.rows[0].own, open(&a).size.rows[0].total);
    assert!(total.messages > own.messages && total.size > own.size, "the sample's Inbox has mail in a folder under it");
    let mut h = window(a);
    click(&mut h, "Folder Size…");
    for heading in ["Total messages", "Total size"] {
        assert!(h.query_by_label(heading).is_some(), "{heading}");
    }
    assert!(h.query_by_label(&total.messages.to_string()).is_some());
    assert!(h.query_by_label(&super::size(total.size)).is_some());
}

#[test]
fn a_folders_name_goes_to_it_and_closes_the_dialog() {
    let (mut a, _, inbox, reports) = app();
    a.run("folder.properties", json!({"id": inbox})).unwrap();
    let mut h = window(a);
    assert!(h.query_all_by_label_contains("Inbox Properties").next().is_some());
    click(&mut h, "Folder Size…");
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

/// Properties… in the right-click menu of a folder and of an account's heading opens Properties
/// for it.
#[test]
fn properties_opens_from_a_folders_and_an_accounts_right_click_menu() {
    for heading_menu in [false, true] {
        let (a, account, _, reports) = app();
        let mut h = window(a);
        let rect = if heading_menu { h.state().hit.rect("folderHeading", account) } else { h.state().hit.rect("folder", reports) };
        right_click(&mut h, rect.unwrap_or_else(|| panic!("no row on screen (heading: {heading_menu})")).center());
        click(&mut h, "Properties…");
        let d = open(h.state());
        assert_eq!(d.tab, Tab::General);
        if heading_menu {
            assert_eq!(d.general.name, heading(h.state(), account));
            assert_eq!(d.size.rows.len(), h.state().session.mailbox.folders.iter().filter(|f| f.account == account).count());
        } else {
            assert_eq!(d.title, "Quarterly reports Properties");
            assert_eq!(d.size.rows.iter().map(|r| r.id).collect::<Vec<_>>(), [reports]);
        }
    }
}

/// A folder removed while Folder Size is open can't be gone to: its name leaves Folder Size open
/// and the folder in view as it was, and no message box names the folder by its number.
#[test]
fn a_folder_removed_meanwhile_leaves_folder_size_open() {
    let (mut a, account, _, reports) = app();
    a.run("folder.properties", json!({"account": account})).unwrap();
    let in_view = a.session.view.folder;
    let mut h = window(a);
    click(&mut h, "Folder Size…");
    h.state_mut().session.mailbox.folders.retain(|f| f.id != reports);
    h.get_by_label_contains("Quarterly reports").click();
    h.run_steps(4);
    assert!(open(h.state()).size_open);
    assert_eq!(h.state().session.view.folder, in_view);
    assert!(h.state().refused.is_none());
}

/// In the smallest window MailCraft opens (760 × 480), Folder Size with a column of the server's
/// counts keeps its title and Close in sight: the folder's name gives up room, the columns scroll
/// sideways, and fewer rows show.
#[test]
fn folder_size_fits_the_smallest_window() {
    let (mut a, account, inbox, _) = app();
    // More folders than there is room for under the summary.
    for i in 0..30 {
        a.session.run("folder.new", &json!({"name": format!("Project {i}"), "parent": inbox})).unwrap();
    }
    a.run("folder.properties", json!({"account": account})).unwrap();
    let d = open_mut(&mut a);
    for r in &mut d.size.rows {
        r.server = Some((3, 1));
    }
    d.size.server = true;
    d.size_open = true;
    let size = egui::vec2(760.0, 480.0);
    let h = window_of(a, size);
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    assert!(h.query_by_label("On the server").is_some());
    for label in ["Folder Size", "Close"] {
        let rects: Vec<egui::Rect> = h.query_all_by_label(label).map(|n| n.rect()).collect();
        assert!(!rects.is_empty(), "{label}");
        for r in rects {
            assert!(screen.contains_rect(r), "{label} at {r:?}");
        }
    }
}

/// Every control the dialog, its tabs, Folder Size and the account heading's menu add has an
/// accessible name, for a screen reader (what the window had without them is left as it was).
#[test]
fn what_folder_properties_adds_has_names() {
    let (a, account, _, _) = app();
    let mut h = window(a);
    let before = h.unnamed_widgets();
    let added = |h: &Harness<'static, MailApp>| h.unnamed_widgets().into_iter().filter(|w| !before.contains(w)).collect::<Vec<_>>();
    let rect = h.state().hit.rect("folderHeading", account).unwrap();
    right_click(&mut h, rect.center());
    assert_eq!(added(&h), Vec::<String>::new(), "the account heading's menu");
    click(&mut h, "Properties…");
    assert_eq!(added(&h), Vec::<String>::new(), "General");
    for tab in [Tab::AutoArchive, Tab::Policy, Tab::Permissions, Tab::Synchronization, Tab::General] {
        click(&mut h, tab.label());
        assert_eq!(added(&h), Vec::<String>::new(), "{tab:?}");
    }
    click(&mut h, "Folder Size…");
    assert!(open(h.state()).size.rows.len() > 1);
    assert_eq!(added(&h), Vec::<String>::new(), "Folder Size");
}

/// Each tab of a folder's Properties, Folder Size over them, and an account's General, in light
/// and dark, as PNG files to look at:
/// `MAILCRAFT_SHOTS_DIR=<dir> cargo test -p mailcraft-ui-egui shots_of_folder_properties -- --ignored`
#[test]
#[ignore = "writes screenshots, for looking at them"]
fn shots_of_folder_properties() {
    let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") else {
        eprintln!("set MAILCRAFT_SHOTS_DIR to the directory to write to");
        return;
    };
    let shots: Vec<(String, Option<Tab>, bool)> = Tab::ALL
        .into_iter()
        .map(|tab| (format!("folder-{tab:?}").to_lowercase(), Some(tab), false))
        .chain([("folder-size".to_string(), None, false), ("account-general".to_string(), Some(Tab::General), true)])
        .collect();
    for theme in ["light", "dark"] {
        for (name, tab, of_account) in &shots {
            let (mut a, account, inbox, _) = app();
            a.execute("ui.theme", json!({"value": theme})).unwrap();
            let params = if *of_account { json!({"account": account}) } else { json!({"id": inbox}) };
            a.run("folder.properties", params).unwrap();
            let d = open_mut(&mut a);
            match tab {
                Some(tab) => d.tab = *tab,
                None => d.size_open = true,
            }
            let mut harness = Harness::builder()
                .with_accessibility_check(false)
                .allow_missing_glyphs()
                .with_size(egui::vec2(1280.0, 800.0))
                .with_step_dt(1.0 / 60.0)
                .wgpu()
                .build_ui_state(
                    |ui, app: &mut MailApp| {
                        let ctx = ui.ctx().clone();
                        app.logic(&ctx);
                        app.ui(ui);
                    },
                    a,
                );
            harness.input_mut().max_texture_side = Some(8192);
            // (The reminders that come up on the first frame aren't what the shots are of.)
            harness.run_steps(2);
            let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
            harness.run_steps(12);
            let path = std::path::Path::new(&dir).join(format!("{name}-{theme}.png"));
            harness.render().unwrap().save(&path).unwrap();
            println!("{}", path.display());
        }
    }
}
