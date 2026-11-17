// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1176: Ctrl+Shift+V (Cmd+Shift+V) and Ctrl+Shift+Y, and Move to Folder and Copy to Folder run
//! without a folder, ask which folder in Move Items or Copy Items instead of failing with
//! "'folder' (id) is required"; the folder chosen there is where the messages go.

use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use mailcraft_engine::Session;
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::dialogs::{Dialog, input_dialog};
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

const MOCKUPS: &str = "Homepage mockups, round three";

type Window = egui_kittest::Harness<'static, MailApp>;

fn window() -> Window {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let mut harness = egui_kittest::Harness::builder()
        .with_accessibility_check(false)
        .allow_missing_glyphs()
        .with_size(vec2(1280.0, 800.0))
        .with_step_dt(1.0 / 60.0)
        .build_ui_state(
            |ui, app: &mut MailApp| {
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.run_steps(4);
    let _ = harness.state_mut().execute("ui.dismissReminder", json!({"all": true}));
    harness.run_steps(3);
    harness
}

/// The message to move, selected, and the Archive folder of its account.
fn selected(harness: &mut Window) -> (Id, Id, Id) {
    let mb = &harness.state().session.mailbox;
    let m = mb.messages.values().find(|m| m.subject == MOCKUPS).unwrap();
    let (id, from) = (m.id, m.folder);
    let archive = mb.folder_of_kind(m.account, FolderKind::Archive).unwrap();
    assert_ne!(from, archive);
    harness.state_mut().execute("folder.select", json!({"id": from})).unwrap();
    harness.state_mut().execute("mail.select", json!({"id": id})).unwrap();
    harness.run_steps(3);
    (id, from, archive)
}

fn dialog(harness: &Window) -> Option<&'static str> {
    harness.state().dialog.as_ref().map(Dialog::name)
}

fn press(harness: &mut Window, key: egui::Key) {
    let mac = cfg!(target_os = "macos");
    let modifiers = egui::Modifiers { alt: false, ctrl: !mac, mac_cmd: mac, command: true, shift: true };
    harness.key_press_modifiers(modifiers, key);
    harness.run_steps(4);
}

/// Marks `folder` in Move Items or Copy Items and presses its OK.
fn choose(harness: &mut Window, folder: Id) {
    let Some(Dialog::MoveItems(d)) = harness.state_mut().dialog.as_mut() else { panic!("Move Items or Copy Items is open") };
    d.mark(folder);
    harness.run_steps(2);
    harness.query_all(egui_kittest::kittest::By::new().role(Role::Button).label("OK")).next().expect("an OK button").click();
    harness.run_steps(4);
}

/// The issue's case: Ctrl+Shift+V on a selected message opens Move Items, nothing moves until a
/// folder is chosen, and the message goes to the one chosen.
#[test]
fn the_move_key_asks_for_the_folder() {
    let mut harness = window();
    let (id, from, archive) = selected(&mut harness);
    press(&mut harness, egui::Key::V);
    assert_eq!(dialog(&harness), Some("moveItems"));
    assert!(harness.query_by_label_contains("Move the selected items to the folder:").is_some());
    if let Ok(dir) = std::env::var("MAILCRAFT_SHOTS_DIR") {
        for theme in ["light", "dark"] {
            harness.state_mut().execute("ui.theme", json!({"value": theme})).unwrap();
            harness.run_steps(3);
            harness.render().unwrap().save(std::path::Path::new(&dir).join(format!("move-items-{theme}.png"))).unwrap();
        }
    }
    assert_eq!(harness.state().session.mailbox.message(id).unwrap().folder, from, "nothing moves before a folder is chosen");
    choose(&mut harness, archive);
    assert_eq!(dialog(&harness), None);
    assert_eq!(harness.state().session.mailbox.message(id).unwrap().folder, archive);
}

/// Ctrl+Shift+Y opens Copy Items, and the copy goes to the folder chosen while the message stays.
#[test]
fn the_copy_key_asks_for_the_folder() {
    let mut harness = window();
    let (id, from, archive) = selected(&mut harness);
    let copies = |h: &Window| h.state().session.mailbox.messages.values().filter(|m| m.subject == MOCKUPS && m.folder == archive).count();
    press(&mut harness, egui::Key::Y);
    assert_eq!(dialog(&harness), Some("copyItems"));
    assert_eq!(copies(&harness), 0);
    choose(&mut harness, archive);
    assert_eq!(dialog(&harness), None);
    assert_eq!(copies(&harness), 1);
    assert_eq!(harness.state().session.mailbox.message(id).unwrap().folder, from);
}

/// Cancel moves nothing; a move with its folder (a drag onto a folder, the ribbon's menu) runs
/// at once, without the dialog.
#[test]
fn cancel_moves_nothing_and_a_folder_given_needs_no_dialog() {
    let mut harness = window();
    let (id, from, archive) = selected(&mut harness);
    press(&mut harness, egui::Key::V);
    harness.query_all(egui_kittest::kittest::By::new().role(Role::Button).label("Cancel")).next().expect("a Cancel button").click();
    harness.run_steps(4);
    assert_eq!((dialog(&harness), harness.state().session.mailbox.message(id).unwrap().folder), (None, from));

    assert_eq!(input_dialog("mail.move", &json!({})), Some("moveItems"));
    assert_eq!(input_dialog("mail.copy", &json!({"folder": null})), Some("copyItems"));
    assert_eq!(input_dialog("mail.move", &json!({"folder": archive})), None);
    // A call that names messages isn't the dialog's, which moves the selection: it is refused.
    for named in [json!({"ids": [id]}), json!({"id": id}), json!({"ids": null})] {
        assert_eq!((input_dialog("mail.move", &named), input_dialog("mail.copy", &named)), (None, None), "{named}");
    }
    assert!(harness.state_mut().run("mail.move", json!({"ids": [id]})).is_err());
    assert_eq!(harness.state().session.mailbox.message(id).unwrap().folder, from, "nothing moved");
    harness.state_mut().run("mail.move", json!({"folder": archive})).unwrap();
    harness.run_steps(3);
    assert_eq!((dialog(&harness), harness.state().session.mailbox.message(id).unwrap().folder), (None, archive));
}

/// The folder in view isn't offered for a move.
#[test]
fn the_folder_in_view_is_not_offered_for_a_move() {
    let mut harness = window();
    let (_, from, _) = selected(&mut harness);
    press(&mut harness, egui::Key::V);
    let Some(Dialog::MoveItems(d)) = harness.state_mut().dialog.as_mut() else { panic!("Move Items is open") };
    d.mark(from);
    harness.run_steps(2);
    let Some(Dialog::MoveItems(d)) = harness.state().dialog.as_ref() else { panic!("Move Items is open") };
    assert_eq!(d.marked(), None, "the folder in view can't be marked for a move");
}

/// Each folder is a list option with its place for a name, and says whether it is marked and
/// whether it can be.
#[test]
fn a_screen_reader_hears_each_folder_and_which_is_marked() {
    let mut harness = window();
    let (_, from, archive) = selected(&mut harness);
    press(&mut harness, egui::Key::V);
    let Some(Dialog::MoveItems(d)) = harness.state_mut().dialog.as_mut() else { panic!("Move Items is open") };
    d.mark(archive);
    harness.run_steps(3);
    let option = |h: &Window, name: &str| {
        let node = h
            .query_all(egui_kittest::kittest::By::new().role(Role::ListBoxOption).label(name))
            .next()
            .unwrap_or_else(|| panic!("{name} is an option"));
        let node = node.accesskit_node();
        (node.is_selected(), node.is_disabled())
    };
    let archive_name = crate::nav::folder_path(&harness.state().session.mailbox, archive);
    let here = crate::nav::folder_path(&harness.state().session.mailbox, from);
    assert_eq!(option(&harness, &archive_name), (Some(true), false));
    assert_eq!(option(&harness, &here), (Some(false), true), "the folder in view can't be chosen for a move");
}

/// Typing a folder's name marks it, and Enter moves the messages there.
#[test]
fn typing_a_name_and_enter_move_there() {
    let mut harness = window();
    let (id, _, archive) = selected(&mut harness);
    press(&mut harness, egui::Key::V);
    harness.event(egui::Event::Text("archi".into()));
    harness.run_steps(3);
    let Some(Dialog::MoveItems(d)) = harness.state().dialog.as_ref() else { panic!("Move Items is open") };
    assert_eq!(d.marked(), Some(archive));
    harness.key_press(egui::Key::Enter);
    harness.run_steps(4);
    assert_eq!((dialog(&harness), harness.state().session.mailbox.message(id).unwrap().folder), (None, archive));
}

/// The macOS menu bar's Message › Move to Folder (its item runs the command with no parameters)
/// opens Move Items too.
#[test]
fn the_menu_bar_item_asks_for_the_folder() {
    let mut harness = window();
    selected(&mut harness);
    let action = crate::menubar::Action::Command { id: "mail.move", params: "{}" };
    crate::menubar::run(harness.state_mut(), &action, String::new);
    harness.run_steps(3);
    assert_eq!(dialog(&harness), Some("moveItems"));
}

/// With nothing selected nothing opens; and what opens it can't name other messages: ids given
/// to `ui.dialog` are not what moves, the selection is.
#[test]
fn only_the_selection_moves() {
    let mut harness = window();
    let (id, from, archive) = selected(&mut harness);
    let other = harness.state().session.mailbox.messages.values().find(|m| m.id != id && m.folder == from).unwrap().id;
    harness.state_mut().execute("ui.dialog", json!({"name": "moveItems", "ids": [other]})).unwrap();
    harness.run_steps(3);
    choose(&mut harness, archive);
    let mb = &harness.state().session.mailbox;
    assert_eq!((mb.message(id).unwrap().folder, mb.message(other).unwrap().folder), (archive, from));

    harness.state_mut().session.sel.messages.clear();
    assert!(crate::move_items::MoveItems::open(harness.state(), false).is_none());
}

/// OK moves as the ribbon's Move menu does, through what the window runs a command with: a
/// message moved into a Microsoft account from another account is told about the headers
/// Microsoft's server leaves out (#451).
#[test]
fn ok_says_what_the_window_says_after_a_move() {
    let mut harness = window();
    let (id, _, _) = selected(&mut harness);
    let app = harness.state_mut();
    let account = app.execute("account.add", json!({"email": "owner@example.org", "kind": "microsoft"})).unwrap()["id"].as_u64().unwrap();
    let mb = &app.session.mailbox;
    let (inbox, archive) = (mb.folder_of_kind(account, FolderKind::Inbox).unwrap(), mb.folder_of_kind(account, FolderKind::Archive).unwrap());
    // The Microsoft account's Inbox in view, the other account's message selected (as a search of
    // All Mailboxes leaves them).
    app.session.view.folder = inbox;
    app.session.sel.messages = vec![id];
    app.execute("ui.dialog", json!({"name": "moveItems"})).unwrap();
    harness.run_steps(3);
    choose(&mut harness, archive);
    assert_eq!(harness.state().session.mailbox.message(id).unwrap().folder, archive);
    let said = harness.state().status_msg.as_ref().map(|(text, _)| text.clone()).unwrap_or_default();
    assert!(said.starts_with("1 message moved into the account"), "{said}");
}

/// A search looks through every folder, also those past the most rows the list draws.
#[test]
fn a_search_finds_a_folder_past_the_rows_drawn() {
    let mut harness = window();
    let (id, _, _) = selected(&mut harness);
    let app = harness.state_mut();
    let account = app.session.mailbox.message(id).unwrap().account;
    let parent = app.session.mailbox.add_folder(account, None, "Bulk").unwrap();
    let mut last = 0;
    for i in 0..5_100 {
        last = app.session.mailbox.add_folder(account, Some(parent), &format!("Bulk {i:04}")).unwrap();
    }
    press(&mut harness, egui::Key::V);
    harness.event(egui::Event::Text("bulk 5099".into()));
    harness.run_steps(3);
    let Some(Dialog::MoveItems(d)) = harness.state().dialog.as_ref() else { panic!("Move Items is open") };
    assert_eq!(d.marked(), Some(last));
}

/// The folders are listed in the order Home › Move lists them, a folder's subfolders under it,
/// each with its place in the account, as the folder pane names it.
#[test]
fn the_folders_are_listed_as_home_move_lists_them() {
    let mut harness = window();
    let (id, _, _) = selected(&mut harness);
    let app = harness.state_mut();
    let account = app.session.mailbox.message(id).unwrap().account;
    // A few more, with names that sort differently in case, and a level deeper.
    let projects = app.session.mailbox.add_folder(account, None, "projects").unwrap();
    for name in ["beta", "Alpha", "gamma"] {
        let f = app.session.mailbox.add_folder(account, Some(projects), name).unwrap();
        app.session.mailbox.add_folder(account, Some(f), "Notes").unwrap();
    }
    let dialog = crate::move_items::MoveItems::open(harness.state(), false).unwrap();
    let mb = &harness.state().session.mailbox;
    fn walk(mb: &mailcraft_model::Mailbox, account: Id, parent: Option<Id>, depth: usize, out: &mut Vec<(Id, usize, String)>) {
        for f in mb.children(account, parent) {
            out.push((f.id, depth, crate::nav::folder_path(mb, f.id)));
            walk(mb, account, Some(f.id), depth + 1, out);
        }
    }
    let mut expected = Vec::new();
    walk(mb, account, None, 0, &mut expected);
    assert_eq!(dialog.listed(), expected);
}
