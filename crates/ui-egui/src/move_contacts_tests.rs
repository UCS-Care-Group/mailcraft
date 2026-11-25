// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Move to Contacts Folder… (#1144): what opens the dialog, the folders it offers, and the
//! question about somebody in the folder already.

use mailcraft_engine::Session;
use mailcraft_model::contact_sync::{FolderOwner, RemoteContactFolder};
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use mailcraft_model::{Contact, Id, Labeled};
use serde_json::json;

use super::{MoveContacts, targets};
use crate::dialogs::Dialog;
use crate::{MailApp, Services};

const NOW: i64 = 1_791_990_000;
const ADA: &str = "ada@example.org";

/// The sample mailbox with a Microsoft account whose main folder has Bo, and a "Sales" folder
/// that may only be read; and Rowan on this computer, with Bo's address.
fn app() -> (MailApp, Id) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    app.session.driver.set_auto_sync(false);
    let params = json!({"email": ADA, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"});
    let account = app.execute("account.add", params).unwrap()["id"].as_u64().unwrap();
    let bo = RemoteContact { id: "c-bo".into(), first: "Bo".into(), emails: vec![Labeled::new("work", "bo@example.org")], ..Default::default() };
    let main = ContactsDownload { account, email: ADA.into(), complete: true, contacts: vec![bo], ..Default::default() };
    app.execute("sync.applyContacts", json!({"download": main})).unwrap();
    let sales = RemoteContactFolder {
        id: "f-sales".into(),
        name: "Sales".into(),
        owner: FolderOwner::Mailbox("sales@example.org".into()),
        writable: false,
        ..Default::default()
    };
    let shared = ContactsDownload { account, email: ADA.into(), complete: true, folder: Some(sales), ..Default::default() };
    app.execute("sync.applyContacts", json!({"download": shared})).unwrap();
    let rowan = app.session.mailbox.add_contact(Contact {
        first: "Rowan".into(),
        emails: vec![Labeled::new("home", "BO@example.org")],
        ..Default::default()
    });
    (app, rowan)
}

#[test]
fn move_without_a_folder_opens_the_dialog_and_changes_nothing() {
    let (mut app, rowan) = app();
    let _ = app.execute("people.select", json!({"id": rowan}));
    let before = app.session.mailbox.contacts.clone();
    assert_eq!(app.run("people.moveToFolder", json!({"ids": [rowan]})).unwrap(), json!({"pending": "moveContacts"}));
    assert!(matches!(app.dialog, Some(Dialog::MoveContacts(_))));
    assert_eq!(app.session.mailbox.contacts, before);
    assert_eq!(crate::ribbon::move_params(&app), json!({}));
}

#[test]
fn only_folders_a_contact_can_go_in_are_offered() {
    let (app, _) = app();
    let names: Vec<String> = targets(&app.session.mailbox).into_iter().map(|(_, name)| name).collect();
    assert_eq!(names, ["On this computer", "Contacts (ada@example.org)"]);
}

#[test]
fn somebody_there_already_is_asked_about_before_anything_moves() {
    let (mut app, rowan) = app();
    let main = targets(&app.session.mailbox)[1].0;
    let bo = app.session.mailbox.contacts.values().find(|c| c.first == "Bo").unwrap().id;
    let mut d = MoveContacts::open(&app, &json!({"ids": [rowan]})).unwrap();
    assert!(!d.choose(&mut app, main));
    assert_eq!(d.duplicate, Some((bo, main)));
    assert_eq!(app.session.mailbox.contact(rowan).unwrap().folder, 0);
    // Move anyway: moved, and nothing else asked.
    assert!(d.go(&mut app, main), "{}", d.error);
    assert_eq!(app.session.mailbox.contact(rowan).unwrap().folder, main);
}

#[test]
fn nobody_there_moves_at_once() {
    let (mut app, rowan) = app();
    let bo = app.session.mailbox.contacts.values().find(|c| c.first == "Bo").unwrap().id;
    app.session.mailbox.contact_mut(rowan).unwrap().emails.clear();
    let mut d = MoveContacts::open(&app, &json!({"ids": [bo]})).unwrap();
    assert!(d.choose(&mut app, 0), "{}", d.error);
    assert_eq!(d.duplicate, None);
    assert!(app.session.mailbox.contacts.values().any(|c| c.first == "Bo" && c.folder == 0));
}

#[test]
fn one_still_being_sent_is_left_out_and_named() {
    let (mut app, rowan) = app();
    let main = targets(&app.session.mailbox)[1].0;
    app.session.mailbox.contact_mut(rowan).unwrap().emails.clear();
    let gus = app.execute("people.new", json!({"first": "Gus", "folder": main})).unwrap()["id"].as_u64().unwrap();
    let mut d = MoveContacts::open(&app, &json!({"ids": [rowan, gus]})).unwrap();
    assert!(d.go(&mut app, main), "{}", d.error);
    assert_eq!(app.session.mailbox.contact(rowan).unwrap().folder, main);
    let said = app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("Gus") && said.contains("1 contact wasn't moved"), "{said}");
    // Many: the first few named, the rest counted.
    let mut ids = vec![gus];
    for n in 0..7 {
        ids.push(app.execute("people.new", json!({"first": format!("P{n}"), "folder": main})).unwrap()["id"].as_u64().unwrap());
    }
    let back = app.execute("people.new", json!({"first": "Una", "folder": 0})).unwrap()["id"].as_u64().unwrap();
    ids.push(back);
    let mut d = MoveContacts::open(&app, &json!({"ids": ids})).unwrap();
    assert!(d.go(&mut app, main), "{}", d.error);
    let said = app.status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("8 contacts weren't moved") && said.contains("P3") && !said.contains("P4") && said.contains("and 3 more"), "{said}");
}
