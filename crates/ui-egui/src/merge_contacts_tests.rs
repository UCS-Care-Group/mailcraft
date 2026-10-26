// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People › Merge Duplicates (#506): what opens the dialog, the contacts picked together, the
//! contacts left out of a merge, and Enter, which never merges.

use egui_kittest::Harness;
use mailcraft_engine::Session;
use mailcraft_model::{Contact, Id, Labeled};
use serde_json::json;

use super::MergeContacts;
use crate::dialogs::Dialog;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_990_000;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app
}

/// Three contacts with one address: a group of three.
fn three(app: &mut MailApp) -> Vec<Id> {
    (0..3)
        .map(|i| {
            app.session.mailbox.add_contact(Contact {
                first: "Rowan".into(),
                last: format!("Dup{i}"),
                emails: vec![Labeled::new("work", "rowan@dup.example")],
                ..Default::default()
            })
        })
        .collect()
}

#[test]
fn merge_without_a_kept_contact_opens_the_dialog_and_changes_nothing() {
    let mut app = app();
    let before = app.session.mailbox.contacts.clone();
    assert_eq!(app.run("people.merge", json!({})).unwrap(), json!({"pending": "mergeContacts"}));
    assert!(matches!(app.dialog, Some(Dialog::MergeContacts(_))));
    assert_eq!(app.session.mailbox.contacts, before);
}

#[test]
fn picking_with_ctrl_gathers_contacts_and_a_plain_selection_ends_it() {
    let mut app = app();
    let ids: Vec<u64> = app.session.mailbox.contacts.keys().copied().take(3).collect();
    let _ = app.execute("people.select", json!({"id": ids[0]}));
    crate::people_view::pick(&mut app, ids[1]);
    crate::people_view::pick(&mut app, ids[2]);
    assert_eq!(app.people_picked, ids);
    assert_eq!(app.session.sel.contact, Some(ids[2]));
    assert_eq!(crate::ribbon::merge_params(&app), json!({"ids": ids}));
    // Taken out again, down to one: nothing is picked together any more.
    crate::people_view::pick(&mut app, ids[2]);
    crate::people_view::pick(&mut app, ids[1]);
    assert!(app.people_picked.is_empty());
    crate::people_view::pick(&mut app, ids[1]);
    let _ = app.execute("people.select", json!({"id": ids[2]}));
    crate::people_view::keep_picked(&mut app);
    assert!(app.people_picked.is_empty());
    assert_eq!(crate::ribbon::merge_params(&app), json!({}));
}

#[test]
fn a_contact_unticked_is_left_as_it_is() {
    let mut app = app();
    let ids = three(&mut app);
    let mut d = MergeContacts::open(&app, &json!({"ids": ids})).unwrap();
    let keep = d.keep;
    let out = *ids.iter().find(|id| **id != keep).unwrap();
    let merged = *ids.iter().find(|id| **id != keep && **id != out).unwrap();
    let left = app.session.mailbox.contact(out).cloned();
    d.left_out.insert(out);
    // The kept one can't be left out.
    d.left_out.insert(keep);
    assert_eq!(d.others(), vec![merged]);
    assert!(d.press_merge(&mut app), "{}", d.error);
    assert!(app.session.mailbox.contact(merged).is_none());
    assert!(app.session.mailbox.contact(keep).is_some());
    assert_eq!(app.session.mailbox.contact(out).cloned(), left);
}

#[test]
fn picked_contacts_that_are_gone_are_said_not_merged_silently() {
    let mut app = app();
    let ids = three(&mut app);
    let mut d = MergeContacts::open(&app, &json!({"ids": ids[..2]})).unwrap();
    app.session.mailbox.remove_contact(ids[1]).unwrap();
    app.session.mailbox.remove_contact(ids[0]).ok();
    assert!(!d.press_merge(&mut app));
    assert!(d.error.contains("Fewer than two"), "{}", d.error);
    // Opened on picks of which fewer than two are left: it says so.
    let d = MergeContacts::open(&app, &json!({"ids": [ids[0], ids[1], ids[2]]})).unwrap();
    assert!(d.groups.is_empty() && d.note.contains("Fewer than two"));
}

#[test]
fn enter_doesnt_merge() {
    let mut app = app();
    let ids = three(&mut app);
    let before = app.session.mailbox.contacts.clone();
    app.run("people.merge", json!({"ids": ids})).unwrap();
    let mut h = Harness::builder().with_accessibility_check(false).allow_missing_glyphs().with_size(egui::vec2(1280.0, 900.0)).build_ui_state(
        |ui, app: &mut MailApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    assert!(matches!(h.state().dialog, Some(Dialog::MergeContacts(_))));
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(h.state().session.mailbox.contacts, before);
    assert!(matches!(h.state().dialog, Some(Dialog::MergeContacts(_))));
}
