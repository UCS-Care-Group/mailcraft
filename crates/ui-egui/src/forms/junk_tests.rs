// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Junk E-mail Options (#504): the dialog opens by name and from `junk.list`, adds and removes
//! through the commands, and takes hostile text without trouble.

use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_model::JunkList;
use serde_json::json;

use super::Form;
use super::junk::JunkForm;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app
}

#[test]
fn the_dialog_opens_by_name_on_the_page_asked_for() {
    let a = app();
    let f = Form::open("junkOptions", &a, &json!({})).unwrap();
    assert_eq!((f.name(), f.title().as_str()), ("junkOptions", "Junk E-mail Options"));
    assert!(matches!(Form::open("junkOptions", &a, &json!({"list": "blocked"})), Some(Form::Junk(f)) if f.page == JunkList::Blocked));
    assert!(matches!(Form::open("junkOptions", &a, &json!({"list": "\u{0}nonsense"})), Some(Form::Junk(f)) if f.page == JunkList::Safe));
    // Junk E-mail Options from the command search or the menu bar opens the dialog.
    assert_eq!(crate::dialogs::input_dialog("junk.list", &json!({})), Some("junkOptions"));
}

#[test]
fn add_and_remove_are_the_commands() {
    let mut a = app();
    let mut f = JunkForm::new(&json!({"list": "blocked"}));
    f.new_entry = " Offers@Shop.example ".into();
    let (command, params) = f.add_command();
    a.execute(command, params).unwrap();
    assert_eq!(a.session.mailbox.junk.blocked, ["offers@shop.example"]);
    assert_eq!(f.remove_command(), None);
    f.selected = Some("offers@shop.example".into());
    let (command, params) = f.remove_command().unwrap();
    a.execute(command, params).unwrap();
    assert!(a.session.mailbox.junk.is_empty());
    // Hostile text is the command's to refuse.
    f.new_entry = "\u{0}\u{202e}é\n@".repeat(40_000);
    let (command, params) = f.add_command();
    assert!(a.execute(command, params).is_err());
}

#[test]
fn the_dialog_lays_out_with_long_lists_in_both_themes() {
    for dark in [false, true] {
        let mut a = app();
        for i in 0..300 {
            a.session.mailbox.junk.blocked.push(format!("sender-{i}@{}.example", "x".repeat(i % 60 + 1)));
        }
        a.session.mailbox.junk.safe.push("\u{202e}odd@example.org".into());
        a.execute("ui.dialog", json!({"name": "junkOptions", "list": "blocked"})).unwrap();
        let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
            move |ui, app: &mut MailApp| {
                ui.ctx().set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            a,
        );
        h.run_steps(6);
        assert_eq!(h.state().dialog.as_ref().map(crate::dialogs::Dialog::name), Some("junkOptions"));
    }
}
