// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Address books a server lists, and putting a new contact into one of them.

use mailcraft_model::remote::{ContactOutcome, ContactsUploaded, EventResult};
use mailcraft_model::{ContactBook, Rights};
use serde_json::json;

use crate::Session;
use crate::cmd::remote;

const NOW: i64 = 1_791_993_600;
const BOOK: &str = "https://mail.example.org/carddav/ada@example.org/contacts/personal/";

fn session() -> (Session, u64) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let account = s.mailbox.accounts.first().map(|account| account.id).unwrap_or(0);
    s.mailbox.contact_books.push(ContactBook { account, id: BOOK.into(), name: "My contacts".into(), rights: Rights::Edit });
    (s, account)
}

#[test]
fn a_new_contact_can_go_into_the_address_book_chosen() {
    let (mut s, account) = session();
    let listed = s.run("people.addressBooks", &json!({})).unwrap();
    assert_eq!(listed["books"][0]["id"], BOOK);
    assert_eq!(listed["books"][0]["rights"], "edit");

    let made = s.run("people.new", &json!({"first": "Ada", "last": "Lovelace", "addressBook": BOOK})).unwrap();
    let id = made["id"].as_u64().unwrap();
    let contact = s.mailbox.contact(id).unwrap();
    assert_eq!((contact.book.as_str(), contact.book_id.as_str(), contact.origin.account), ("My contacts", BOOK, account));
    assert!(!contact.uid.is_empty());
    assert!(!contact.origin.is_local(), "it is for the server, not only here");

    // It waits for the next CardDAV run, which writes the card under its own UID.
    let upload = remote::contact_upload(&s, account).expect("a contact to send");
    assert_eq!((upload.ops.len(), upload.ops[0].book.as_str(), upload.ops[0].contact), (1, BOOK, id));
    assert!(upload.ops[0].card.contains(&format!("UID:{}\r\n", upload.ops[0].uid)), "{}", upload.ops[0].card);
    let uploaded = ContactsUploaded {
        account,
        email: upload.email.clone(),
        outcomes: vec![ContactOutcome {
            op: upload.ops[0].clone(),
            result: EventResult::Done,
            remote: format!("{BOOK}{}.vcf", upload.ops[0].uid),
            status: 201,
            ..Default::default()
        }],
    };
    remote::apply_contact_outcomes(&mut s, &uploaded).unwrap();
    let contact = s.mailbox.contact(id).unwrap();
    assert_eq!(contact.origin.id, format!("{BOOK}{}.vcf", upload.ops[0].uid));
    assert!(remote::contact_upload(&s, account).is_none(), "sent once");
}

#[test]
fn a_contact_without_a_book_stays_on_this_computer() {
    let (mut s, _) = session();
    let made = s.run("people.new", &json!({"first": "Grace", "last": "Hopper"})).unwrap();
    let contact = s.mailbox.contact(made["id"].as_u64().unwrap()).unwrap();
    assert!(contact.origin.is_local() && contact.book.is_empty() && contact.book_id.is_empty());
    assert!(!contact.uid.is_empty(), "a new card has an identity if it is ever exported");
}

#[test]
fn a_read_only_address_book_is_not_offered_a_new_contact() {
    let (mut s, account) = session();
    s.mailbox.contact_books.push(ContactBook {
        account,
        id: "https://mail.example.org/carddav/ada@example.org/contacts/~book-9/".into(),
        name: "Team".into(),
        rights: Rights::Read,
    });
    let why = s
        .run("people.new", &json!({"first": "Ada", "addressBook": "https://mail.example.org/carddav/ada@example.org/contacts/~book-9/"}))
        .unwrap_err()
        .to_string();
    assert!(why.contains("can't be written"), "{why}");
}
