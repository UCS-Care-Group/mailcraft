// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1017: an account has a name of the owner's own in the folder pane. `account.edit` gives it
//! one, an empty one is the address again, and a name that is the address follows it.

use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::Session;

fn add(s: &mut Session, params: Value) -> Id {
    s.run("account.add", &params).unwrap()["id"].as_u64().unwrap()
}

fn name(s: &Session, id: Id) -> String {
    s.mailbox.account(id).unwrap().name.clone()
}

fn listed_name(s: &mut Session, id: Id) -> Value {
    let list = s.run("account.list", &json!({})).unwrap();
    list["accounts"].as_array().unwrap().iter().find(|a| a["id"] == json!(id)).unwrap()["name"].clone()
}

/// A new account goes by its address; a name given to it is the one the folder pane and
/// `account.list` show, and the rest of the account stays as it was.
#[test]
fn an_account_takes_the_name_it_is_given() {
    let mut s = Session::empty();
    let id = add(&mut s, json!({"email": "noor@example.org", "displayName": "Noor Haddad"}));
    assert_eq!(name(&s, id), "noor@example.org");
    assert!(s.mailbox.account(id).unwrap().named_by_address());

    s.run("account.edit", &json!({"id": id, "name": "  Work \r\n"})).unwrap();
    let a = s.mailbox.account(id).unwrap();
    assert_eq!((a.name.as_str(), a.email.as_str(), a.display_name.as_str()), ("Work", "noor@example.org", "Noor Haddad"));
    assert!(!a.named_by_address());
    assert_eq!(listed_name(&mut s, id), json!("Work"));

    // A name is one line of at most 254 characters.
    s.run("account.edit", &json!({"id": id, "name": "x".repeat(400)})).unwrap();
    assert_eq!(name(&s, id).chars().count(), 254);
}

/// An empty name, or one of spaces, gives the account its address back; one without an address is
/// a Mailbox.
#[test]
fn an_empty_name_is_the_address_again() {
    let mut s = Session::empty();
    let id = add(&mut s, json!({"email": "noor@example.org"}));
    for empty in ["", "   "] {
        s.run("account.edit", &json!({"id": id, "name": "Home"})).unwrap();
        s.run("account.edit", &json!({"id": id, "name": empty})).unwrap();
        assert_eq!(name(&s, id), "noor@example.org");
    }
    let nameless = mailcraft_model::Account { name: "Mailbox".into(), ..Default::default() };
    assert_eq!(nameless.name_by_default(), "Mailbox");
    assert!(nameless.named_by_address());
}

/// While an account goes by its address, it follows the address to a new one; a name of the
/// owner's own stays, and so does one given with the new address.
#[test]
fn a_name_that_is_the_address_follows_it() {
    let mut s = Session::empty();
    let plain = add(&mut s, json!({"email": "noor@example.org"}));
    s.run("account.edit", &json!({"id": plain, "email": "noor.haddad@example.org"})).unwrap();
    assert_eq!(name(&s, plain), "noor.haddad@example.org");

    let named = add(&mut s, json!({"email": "ada@example.org"}));
    s.run("account.edit", &json!({"id": named, "name": "Work"})).unwrap();
    s.run("account.edit", &json!({"id": named, "email": "ada.lovelace@example.org"})).unwrap();
    assert_eq!(name(&s, named), "Work");

    // A name that isn't a string is no name given.
    s.run("account.edit", &json!({"id": plain, "email": "noor.h@example.org", "name": null})).unwrap();
    assert_eq!(name(&s, plain), "noor.h@example.org");

    let both = add(&mut s, json!({"email": "lin@example.org"}));
    s.run("account.edit", &json!({"id": both, "email": "lin.wei@example.org", "name": "Club"})).unwrap();
    assert_eq!(name(&s, both), "Club");
}
