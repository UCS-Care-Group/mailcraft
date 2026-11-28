// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Contacts folders in People (#1917): where a contact is kept, one that can only be read, a
//! change not sent yet, one folder shown alone, the folder a new contact goes in, and changes
//! the server keeps putting off in the Progress list.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use mailcraft_model::contact_sync::{FolderOwner, RemoteContactFolder};
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use serde_json::{Value, json};

use super::*;

fn add(w: &mut Harness<'_, MailApp>, email: &str) -> Id {
    let params = json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"});
    w.state_mut().execute("account.add", params).unwrap()["id"].as_u64().unwrap()
}

/// `name` in `folder` of the account (none: its main folder); the contact's id.
fn put(w: &mut Harness<'_, MailApp>, account: Id, folder: Option<RemoteContactFolder>, name: &str) -> Id {
    let email = w.state().session.mailbox.account(account).unwrap().email.clone();
    let contact = RemoteContact { id: name.replace(' ', "-"), display_name: name.into(), ..Default::default() };
    let download = ContactsDownload { account, email, complete: true, contacts: vec![contact], folder, ..Default::default() };
    mailcraft_engine::cmd::remote::apply_contacts(&mut w.state_mut().session, &download).unwrap();
    w.state().session.mailbox.contacts.values().find(|c| c.display_name == name).unwrap().id
}

fn remote(id: &str, name: &str, owner: FolderOwner, writable: bool) -> Option<RemoteContactFolder> {
    Some(RemoteContactFolder { id: id.into(), name: name.into(), owner, writable, ..Default::default() })
}

fn run(w: &mut Harness<'_, MailApp>, id: &str, params: Value) {
    w.state_mut().execute(id, params).unwrap_or_else(|e| panic!("{id}: {e}"));
    w.state_mut().execute("ui.dismissReminder", json!({"all": true})).unwrap();
    w.run();
}

/// Open the drop-down that shows `shown`.
fn open_combo(w: &mut Harness<'_, MailApp>, shown: &str) {
    let shown = shown.to_string();
    let is = move |n: &egui_kittest::kittest::AccessKitNode<'_>| {
        n.role() == egui::accesskit::Role::ComboBox && (n.value().as_deref() == Some(shown.as_str()) || n.label().as_deref() == Some(shown.as_str()))
    };
    // (The last: a dialog's, over the list's.)
    w.query_all(egui_kittest::kittest::By::new().predicate(is)).last().expect("the drop-down").click();
    w.run();
}

fn folder_of(w: &Harness<'_, MailApp>, id: Id) -> Id {
    w.state().session.mailbox.contact(id).unwrap().folder
}

#[test]
fn each_folder_is_named_for_whose_it_is() {
    let mut w = testing::window("view.people");
    let ada = add(&mut w, "ada@example.org");
    let main = put(&mut w, ada, None, "Mo Main");
    let friends = put(&mut w, ada, remote("fr", "Friends", FolderOwner::Own, true), "Fay Friend");
    let sales = put(&mut w, ada, remote("sm", "Contacts", FolderOwner::Mailbox("sales@example.org".into()), false), "Sam Sales");
    let shared = put(&mut w, ada, remote("", "Contacts", FolderOwner::Person("bo@example.org".into()), true), "Eli Evans");
    let mailbox = &w.state().session.mailbox;
    let name = |id: Id| folder_name(mailbox, mailbox.contact(id).unwrap().folder);
    assert_eq!(name(main), "Contacts (ada@example.org)");
    assert_eq!(name(friends), "Friends (ada@example.org)");
    assert_eq!(name(sales), "Contacts (sales@example.org)");
    assert_eq!(name(shared), "bo@example.org's shared contacts");
    assert_eq!(folder_name(mailbox, 0), "On this computer");
    assert_eq!(folder_name(mailbox, 9_999_999), "On this computer");
    // A person known by name is named.
    let mut w = w;
    w.state_mut().execute("people.new", json!({"first": "Bo", "last": "Baker", "email": "bo@example.org", "folder": 0})).unwrap();
    let mailbox = &w.state().session.mailbox;
    assert_eq!(folder_name(mailbox, mailbox.contact(shared).unwrap().folder), "Bo Baker's shared contacts");
}

#[test]
fn the_card_says_where_it_is_kept_and_what_waits() {
    let mut w = testing::window("view.people");
    let ada = add(&mut w, "ada@example.org");
    let mine = put(&mut w, ada, None, "Mo Main");
    let sales = put(&mut w, ada, remote("sm", "Contacts", FolderOwner::Mailbox("sales@example.org".into()), false), "Sam Sales");
    let card = |w: &Harness<'_, MailApp>, id: Id| {
        let c = w.state().session.mailbox.contact(id).unwrap();
        (kept_in(w.state(), c), read_only_here(w.state(), c), mailcraft_engine::cmd::contacts_remote::not_sent(&w.state().session.mailbox, id))
    };
    run(&mut w, "people.select", json!({"id": sales}));
    assert_eq!(card(&w, sales), (Some("Contacts (sales@example.org)".to_string()), true, false));
    assert!(!w.state().enabled("people.edit") && !w.state().enabled("people.delete"));
    // Enter opens no form for it: the refusal says why.
    testing::key(&mut w, egui::Key::Enter);
    assert_eq!(testing::dialog(&w), None);

    run(&mut w, "people.select", json!({"id": mine}));
    assert_eq!(card(&w, mine), (Some("Contacts (ada@example.org)".to_string()), false, false));
    assert!(w.state().enabled("people.edit") && w.state().enabled("people.delete"));
    // A change waits to go up: the card with "Not sent yet" draws.
    run(&mut w, "people.edit", json!({"id": mine, "title": "Lead"}));
    assert_eq!(card(&w, mine), (Some("Contacts (ada@example.org)".to_string()), false, true));
}

#[test]
fn one_folder_is_shown_alone_from_the_choice_above_the_list() {
    let mut w = testing::window("view.people");
    let ada = add(&mut w, "ada@example.org");
    put(&mut w, ada, None, "Mo Main");
    let fay = put(&mut w, ada, remote("fr", "Friends", FolderOwner::Own, true), "Fay Friend");
    let ctx = w.state().ctx.clone().unwrap();
    run(&mut w, "view.peopleAccount", json!({"value": "all"}));
    // The choice lists each account's folders under it.
    open_combo(&mut w, "All accounts");
    w.get_by_label("Friends (ada@example.org)").click();
    w.run();
    assert_eq!(people::shown_folder(&w.state().session), Some(folder_of(&w, fay)));
    let list = list_layout(w.state(), &ctx);
    assert_eq!((list.total, list.rows.iter().map(|r| r.id).collect::<Vec<_>>()), (1, vec![fay]));
}

#[test]
fn a_new_contact_goes_in_the_folder_picked() {
    let mut w = testing::window("view.people");
    let ada = add(&mut w, "ada@example.org");
    put(&mut w, ada, None, "Mo Main");
    let fay = put(&mut w, ada, remote("fr", "Friends", FolderOwner::Own, true), "Fay Friend");
    put(&mut w, ada, remote("ro", "Board", FolderOwner::Own, false), "Bea Board");
    let friends = folder_of(&w, fay);
    run(&mut w, "view.peopleFolder", json!({"value": friends}));
    w.state_mut().execute("ui.dialog", json!({"name": "contact", "first": "Nia"})).unwrap();
    w.run();
    assert_eq!(testing::dialog(&w), Some("contact"));
    // It shows where the contact goes by itself; a folder that can only be read isn't offered.
    open_combo(&mut w, "Friends (ada@example.org)");
    assert!(w.query_by_label("Contacts (ada@example.org)").is_some());
    assert!(w.query_by_label("Board (ada@example.org)").is_none());
    w.get_by_label("On this computer").click();
    w.run();
    w.get_by_label("Save").click();
    w.run();
    let nia = w.state().session.mailbox.contacts.values().find(|c| c.first == "Nia").unwrap();
    assert_eq!((nia.folder, nia.origin.is_local()), (0, true));
}

/// An account whose main folder isn't made yet is offered as such, so it can be picked again
/// after another folder was: the contact then goes in that folder, made for it.
#[test]
fn the_folder_a_new_contact_goes_in_by_itself_can_be_picked_again() {
    let mut w = testing::window("view.people");
    let ada = add(&mut w, "ada@example.org");
    put(&mut w, ada, remote("fr", "Friends", FolderOwner::Own, true), "Fay Friend");
    assert_eq!(w.state().session.mailbox.main_contact_folder(ada), None);
    w.state_mut().execute("ui.dialog", json!({"name": "contact", "first": "Nia"})).unwrap();
    w.run();
    open_combo(&mut w, "Contacts (ada@example.org)");
    w.get_by_label("On this computer").click();
    w.run();
    open_combo(&mut w, "On this computer");
    w.get_by_label("Contacts (ada@example.org)").click();
    w.run();
    w.get_by_label("Save").click();
    w.run();
    let mailbox = &w.state().session.mailbox;
    let nia = mailbox.contacts.values().find(|c| c.first == "Nia").unwrap();
    assert_eq!(Some(nia.folder), mailbox.main_contact_folder(ada));
}

#[test]
fn changes_the_server_puts_off_are_in_the_progress_list() {
    let mut w = testing::window("view.people");
    let ada = add(&mut w, "ada@example.org");
    let mo = put(&mut w, ada, None, "Mo Main");
    run(&mut w, "people.edit", json!({"id": mo, "title": "Lead"}));
    // Waiting its first try: nothing to say yet.
    assert_eq!(crate::sync_panel::contacts_waiting(w.state(), ada), None);
    let result = json!({"account": ada, "email": "ada@example.org", "outcomes": [
        {"op": {"kind": "change", "contact": mo, "remote": "Mo-Main"}, "result": "failed", "status": 503, "error": "The server is busy\r\n"}]});
    run(&mut w, "sync.applyContactChanges", json!({"result": result}));
    let line = crate::sync_panel::contacts_waiting(w.state(), ada).unwrap();
    assert_eq!(line, "1 change to contacts isn't on the server yet: it is tried again later (The server is busy)");
    assert!(crate::sync_panel::rows(w.state()).iter().any(|r| r.account == ada && r.problems.contains(&line)));
}

/// What the server said when it put a change off is a line of its own on the card, so a long
/// one never takes the room of "Not sent yet".
#[test]
fn a_long_reason_from_the_server_is_a_line_of_its_own() {
    let mut w = testing::window("view.people");
    let ada = add(&mut w, "ada@example.org");
    let mo = put(&mut w, ada, None, "Mo Main");
    run(&mut w, "people.edit", json!({"id": mo, "title": "Lead"}));
    let long = "The server is busy ".repeat(12).trim().to_string();
    let result = json!({"account": ada, "email": "ada@example.org", "outcomes": [
        {"op": {"kind": "change", "contact": mo, "remote": "Mo-Main"}, "result": "failed", "status": 503, "error": long}]});
    run(&mut w, "sync.applyContactChanges", json!({"result": result}));
    run(&mut w, "people.select", json!({"id": mo}));
    assert!(mailcraft_engine::cmd::contacts_remote::not_sent(&w.state().session.mailbox, mo));
    assert!(w.query_by_label(&long).is_some(), "the reason is a label of its own");
}

/// Every change the server refused for good is said, not only the first: three by name, then
/// how many more.
#[test]
fn every_refusal_from_the_server_is_said() {
    let said = |messages: Vec<String>| crate::feedback("sync.applyContactChanges", &json!({"messages": messages}));
    assert_eq!(said(vec![]), None);
    assert_eq!(said(vec!["A.".into()]).as_deref(), Some("A."));
    assert_eq!(said(vec!["A.".into(), "B.".into()]).as_deref(), Some("A. B."));
    let five: Vec<String> = ["A.", "B.", "C.", "D.", "E."].iter().map(|s| s.to_string()).collect();
    assert_eq!(said(five).as_deref(), Some("A. B. C. And 2 more changes were put back."));
    // The engine keeps 20 lines at most; how many more comes from every refusal.
    let kept: Vec<String> = (0..20).map(|i| format!("{i}.")).collect();
    let line = crate::feedback("sync.applyContactChanges", &json!({"messages": kept, "refused": 21})).unwrap();
    assert_eq!(line, "0. 1. 2. And 18 more changes were put back.");
}
