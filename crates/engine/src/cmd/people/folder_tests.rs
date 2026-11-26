// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One contacts folder of an account in People (#1917): the list shows it alone, and a new
//! contact goes there.

use mailcraft_model::contact_sync::{FolderOwner, RemoteContactFolder};
use mailcraft_model::remote::{ContactsDownload, RemoteContact};
use mailcraft_model::{AccountChoice, Id};
use serde_json::{Value, json};

use super::{shown_folder, sorted};
use crate::Session;
use crate::cmd::contacts_remote::default_contact_folder;
use crate::cmd::remote::apply_contacts;

const NOW: i64 = 1_790_000_000;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

/// One contact named `name` in `folder` of `account` (none: its main folder); the folder's id.
fn folder(s: &mut Session, account: Id, folder: Option<RemoteContactFolder>, name: &str) -> Id {
    let email = s.mailbox.account(account).unwrap().email.clone();
    let contact = RemoteContact { id: name.replace(' ', "-"), display_name: name.into(), ..Default::default() };
    apply_contacts(s, &ContactsDownload { account, email, complete: true, contacts: vec![contact], folder, ..Default::default() }).unwrap();
    s.mailbox.contacts.values().find(|c| c.display_name == name).unwrap().folder
}

fn subfolder(id: &str, writable: bool) -> Option<RemoteContactFolder> {
    Some(RemoteContactFolder { id: id.into(), name: id.into(), owner: FolderOwner::Own, writable, ..Default::default() })
}

/// The sample with one Microsoft account: its main folder, a folder of its own and one it may
/// only read.
fn session() -> (Session, Id, [Id; 3]) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let account =
        run(&mut s, "account.add", json!({"email": "ada@example.org", "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"]
            .as_u64()
            .unwrap();
    let main = folder(&mut s, account, None, "Mo Main");
    let friends = folder(&mut s, account, subfolder("Friends", true), "Fay Friend");
    let shared = folder(&mut s, account, subfolder("Board", false), "Bea Board");
    (s, account, [main, friends, shared])
}

fn names(s: &Session) -> Vec<String> {
    sorted(s, "").into_iter().map(|id| s.mailbox.contact(id).unwrap().display_name.clone()).collect()
}

#[test]
fn a_folder_shows_its_contacts_alone_until_the_account_is_chosen_again() {
    let (mut s, account, [_, friends, _]) = session();
    run(&mut s, "view.peopleFolder", json!({"value": friends}));
    assert_eq!((s.view.people_account, shown_folder(&s)), (AccountChoice::Account(account), Some(friends)));
    assert_eq!(names(&s), ["Fay Friend"]);
    assert_eq!(run(&mut s, "people.list", json!({}))["contacts"].as_array().unwrap().len(), 1);
    run(&mut s, "view.peopleAccount", json!({"value": account}));
    assert_eq!((shown_folder(&s), names(&s).len()), (None, 3));
    // Anything but a contacts folder a server keeps is refused, and nothing changes.
    for junk in [json!(0), json!(-1), json!("x"), json!(null), json!(9_999_999)] {
        assert!(s.run("view.peopleFolder", &json!({"value": junk})).is_err(), "{junk}");
    }
    assert_eq!(shown_folder(&s), None);
}

#[test]
fn a_contact_picked_elsewhere_moves_the_list_off_the_folder() {
    let (mut s, _, [main, friends, _]) = session();
    run(&mut s, "view.peopleFolder", json!({"value": friends}));
    let mo = s.mailbox.contacts.values().find(|c| c.folder == main).unwrap().id;
    run(&mut s, "people.select", json!({"id": mo}));
    assert_eq!(shown_folder(&s), None);
    assert!(sorted(&s, "").contains(&mo));
}

#[test]
fn a_new_contact_goes_in_the_folder_shown_unless_it_can_only_be_read() {
    let (mut s, _, [main, friends, shared]) = session();
    // One account: its main folder.
    assert_eq!(default_contact_folder(&s), Ok(main));
    run(&mut s, "view.peopleFolder", json!({"value": friends}));
    assert_eq!(default_contact_folder(&s), Ok(friends));
    let made = run(&mut s, "people.new", json!({"first": "Nia"}));
    assert_eq!(made["folder"], json!(friends));
    // A folder that can only be read: as if none were shown.
    run(&mut s, "view.peopleFolder", json!({"value": shared}));
    assert_eq!(default_contact_folder(&s), Ok(main));
    assert_eq!(run(&mut s, "people.new", json!({"first": "Ola"}))["folder"], json!(main));
}

#[test]
fn edit_delete_and_categorize_are_off_for_a_contact_that_can_only_be_read() {
    let (mut s, _, [_, friends, shared]) = session();
    for (folder, on) in [(shared, false), (friends, true)] {
        let id = s.mailbox.contacts.values().find(|c| c.folder == folder).unwrap().id;
        run(&mut s, "people.select", json!({"id": id}));
        for command in ["people.edit", "people.delete", "people.categorize"] {
            assert_eq!(s.enabled(command), on, "{command}");
        }
        // Favorite is this computer's own.
        assert!(s.enabled("people.favorite"));
    }
}
