// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People › Merge Duplicates (#506): what opens the dialog, and the contacts picked together.

use mailcraft_engine::Session;
use serde_json::json;

use crate::dialogs::Dialog;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_990_000;

fn app() -> MailApp {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.auto_sync = false;
    app
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
