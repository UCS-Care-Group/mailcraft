// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A contact an Exchange server keeps: its address book is writable, so it is changed here and
//! sent back; one in a read-only book is refused before anything is changed.

use serde_json::json;

use mailcraft_model::remote::{ContactsDownload, RemoteAddressBook, RemoteContact};
use mailcraft_model::{Labeled, Rights};

use crate::Session;
use crate::cmd::remote::apply_contacts;
use crate::sample;

fn session() -> (Session, u64) {
    let now = 1_800_000_000;
    let mut s = Session::at(sample::mailbox(now, 0), now, 0);
    let account = s
        .run("account.add", &json!({"email": "ada@example.org", "kind": "exchange", "ews": {"url": "https://mail.example.org/EWS/Exchange.asmx"}}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    (s, account)
}

fn download(account: u64, rights: Rights) -> ContactsDownload {
    ContactsDownload {
        account,
        email: "ada@example.org".into(),
        complete: true,
        books: vec![RemoteAddressBook { id: "book-1".into(), name: "Contacts".into(), rights }],
        contacts: vec![RemoteContact {
            id: "remote-1".into(),
            book: "Contacts".into(),
            book_id: "book-1".into(),
            display_name: "Remote Ada".into(),
            emails: vec![Labeled::new("", "ada@example.org")],
            ..Default::default()
        }],
    }
}

#[test]
fn a_contact_in_a_writable_exchange_book_is_edited_and_sent_back() {
    let (mut s, account) = session();
    apply_contacts(&mut s, &download(account, Rights::Edit)).unwrap();
    let id = s.mailbox.contacts.values().find(|c| c.display_name == "Remote Ada").map(|c| c.id).unwrap();
    assert_eq!(s.mailbox.contact_books.first().map(|b| b.rights), Some(Rights::Edit));
    s.run("people.edit", &json!({"id": id, "company": "Analytical Engines"})).unwrap_or_else(|e| panic!("{e:?}"));
    let change = s.mailbox.contact_changes.iter().find(|c| c.contact == id);
    assert!(change.is_some(), "the edit waits to go to the server");
    assert_eq!(s.mailbox.contact(id).unwrap().company, "Analytical Engines");
}

#[test]
fn a_contact_in_a_read_only_book_is_refused_and_nothing_changes() {
    let (mut s, account) = session();
    apply_contacts(&mut s, &download(account, Rights::Read)).unwrap();
    let id = s.mailbox.contacts.values().find(|c| c.display_name == "Remote Ada").map(|c| c.id).unwrap();
    assert_eq!(s.mailbox.contact(id).unwrap().book_id, "book-1");
    assert_eq!(s.mailbox.contact_books.first().map(|b| b.rights), Some(Rights::Read));
    assert!(s.run("people.edit", &json!({"id": id, "company": "Analytical Engines"})).is_err());
    assert_eq!(s.mailbox.contact(id).unwrap().company, "");
    assert!(s.mailbox.contact_changes.is_empty());
    // And a new contact cannot be put in it.
    assert!(s.run("people.new", &json!({"displayName": "Bo", "addressBook": "book-1"})).is_err());
}

#[test]
fn a_new_contact_goes_in_a_writable_book_and_is_sent_back() {
    let (mut s, account) = session();
    apply_contacts(&mut s, &download(account, Rights::Edit)).unwrap();
    let made = s.run("people.new", &json!({"displayName": "Bo", "addressBook": "book-1"})).unwrap();
    let _id = made["id"].as_u64().unwrap();
    let request = s.run("sync.ewsDirectoryRequest", &json!({"account": account})).unwrap();
    let ops = request["contactChanges"]["ops"].as_array().cloned().unwrap_or_default();
    assert!(ops.iter().any(|op| op["kind"] == json!("create")), "the new contact is in the request: {request}");
}
