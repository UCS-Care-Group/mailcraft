// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One address book of an account in People (#1275).

use mailcraft_model::remote::{ContactsDownload, RemoteAddressBook, RemoteContact};
use mailcraft_model::{AccountChoice, Id, Labeled, Rights};
use serde_json::{Value, json};

use super::{shown_book, sorted};
use crate::Session;
use crate::cmd::remote::merge_contacts;

const NOW: i64 = 1_790_000_000;

fn run(s: &mut Session, id: &str, params: Value) -> Value {
    s.run(id, &params).unwrap_or_else(|e| panic!("{id} {params}: {e}"))
}

fn contact(id: &str, name: &str, book: &RemoteAddressBook) -> RemoteContact {
    RemoteContact {
        id: id.into(),
        display_name: name.into(),
        emails: vec![Labeled::new("work", format!("{id}@example.org"))],
        book: book.name.clone(),
        book_id: book.id.clone(),
        ..Default::default()
    }
}

fn id_of(s: &Session, name: &str) -> Id {
    s.mailbox.contacts.values().find(|c| c.display_name == name).unwrap().id
}

/// The sample with an account whose server lists two address books, one of them read-only, of one
/// contact each, and a second account with a book of the same id.
fn session() -> (Session, Id, Id) {
    let mut s = Session::at(crate::sample::mailbox(NOW, 0), NOW, 0);
    let mut add = |email: &str| {
        run(&mut s, "account.add", json!({"email": email, "incoming": {"host": "outlook.example.org"}, "auth": "oauth2"}))["id"].as_u64().unwrap()
    };
    let (work, home) = (add("work@example.org"), add("home@example.org"));
    let own = RemoteAddressBook { id: "own".into(), name: "Contacts".into(), rights: Rights::Edit };
    let shared = RemoteAddressBook { id: "pf-external".into(), name: "External contacts".into(), rights: Rights::Read };
    let download = ContactsDownload {
        account: work,
        email: "work@example.org".into(),
        complete: true,
        books: vec![own.clone(), shared.clone()],
        contacts: vec![contact("w1", "Wren Own", &own), contact("w2", "Pell External", &shared)],
    };
    merge_contacts(&mut s.mailbox, &download).unwrap();
    let elsewhere = RemoteAddressBook { id: "own".into(), name: "Contacts".into(), rights: Rights::Edit };
    let download = ContactsDownload {
        account: home,
        email: "home@example.org".into(),
        complete: true,
        books: vec![elsewhere.clone()],
        contacts: vec![contact("h1", "Hollis Home", &elsewhere)],
    };
    merge_contacts(&mut s.mailbox, &download).unwrap();
    (s, work, home)
}

#[test]
fn one_address_book_of_the_account_shown_is_shown_alone() {
    let (mut s, work, home) = session();
    let (own, external) = (id_of(&s, "Wren Own"), id_of(&s, "Pell External"));
    // A book is one account's: with every account's contacts in the list there is none to choose.
    assert!(s.run("view.peopleBook", &json!({"value": "own"})).is_err());
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    let mut both = sorted(&s, "");
    both.sort_unstable();
    let mut expected = vec![own, external];
    expected.sort_unstable();
    assert_eq!(both, expected);

    run(&mut s, "view.peopleBook", json!({"value": "pf-external"}));
    assert_eq!((shown_book(&s), sorted(&s, "")), (Some("pf-external"), vec![external]));
    let books = run(&mut s, "people.addressBooks", json!({}));
    let shown: Vec<&str> = books["books"].as_array().unwrap().iter().filter(|b| b["shown"] == true).map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(shown, ["pf-external"]);
    // The list's answer for agents says the same, and another book can be asked for by name.
    let listed = run(&mut s, "people.list", json!({}));
    assert_eq!((listed["book"].clone(), listed["contacts"].as_array().unwrap().len()), (json!("pf-external"), 1));
    let listed = run(&mut s, "people.list", json!({"account": work, "book": "own"}));
    assert_eq!(listed["contacts"][0]["id"], json!(own));
    assert!(s.run("people.list", &json!({"account": "all", "book": "own"})).is_err(), "a book needs its account");

    // A book the account doesn't have is refused, the other account's of the same id too.
    assert!(s.run("view.peopleBook", &json!({"value": "nowhere"})).is_err());
    assert_eq!(shown_book(&s), Some("pf-external"));
    // "" is all of the account's books again.
    run(&mut s, "view.peopleBook", json!({"value": ""}));
    assert_eq!((shown_book(&s), sorted(&s, "").len()), (None, 2));

    // Choosing another account shows all of its contacts.
    run(&mut s, "view.peopleBook", json!({"value": "pf-external"}));
    run(&mut s, "view.peopleAccount", json!({"value": home}));
    assert_eq!((shown_book(&s), s.view.people_book.as_str()), (None, ""));
    // And choosing the same account again keeps the book.
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    run(&mut s, "view.peopleBook", json!({"value": "pf-external"}));
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    assert_eq!(shown_book(&s), Some("pf-external"));
}

#[test]
fn a_contact_of_another_book_selected_shows_the_accounts_whole_list() {
    let (mut s, work, _) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    run(&mut s, "view.peopleBook", json!({"value": "pf-external"}));
    let own = id_of(&s, "Wren Own");
    super::reveal(&mut s, own);
    assert_eq!((shown_book(&s), s.view.people_account), (None, AccountChoice::Account(work)));
    assert!(sorted(&s, "").contains(&own));
}

#[test]
fn a_selected_contact_outside_the_book_chosen_is_no_longer_selected() {
    let (mut s, work, _) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    let own = id_of(&s, "Wren Own");
    s.sel.contact = Some(own);
    run(&mut s, "view.peopleBook", json!({"value": "pf-external"}));
    assert_eq!(s.sel.contact, None);
}

#[test]
fn a_book_the_server_no_longer_lists_shows_all_of_the_accounts_contacts() {
    let (mut s, work, _) = session();
    run(&mut s, "view.peopleAccount", json!({"value": work}));
    run(&mut s, "view.peopleBook", json!({"value": "pf-external"}));
    s.mailbox.contact_books.retain(|book| book.id != "pf-external");
    assert_eq!(shown_book(&s), None);
    assert!(sorted(&s, "").contains(&id_of(&s, "Wren Own")));
}
