// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! People's Address Book (#1262): the organisation's address list of an Exchange server account,
//! chosen in place of the contacts.

use serde_json::json;

use crate::cmd::people::{address_lists, shown_book};
use crate::{Module, Session};

const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

fn exchange(s: &mut Session, email: &str) -> u64 {
    s.run("account.add", &json!({"email": email, "kind": "exchange", "ews": {"url": URL}})).unwrap()["id"].as_u64().unwrap()
}

#[test]
fn people_shows_an_exchange_server_accounts_address_list_until_the_contacts_are_chosen() {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": "me@example.org", "kind": "imap"})).unwrap();
    // No address list to show without an Exchange server account.
    assert!(address_lists(&s.mailbox).is_empty());
    assert!(s.run("people.addressBook", &json!({})).is_err());
    let first = exchange(&mut s, "ada@example.org");
    let second = exchange(&mut s, "bea@example.org");
    assert_eq!(address_lists(&s.mailbox), [first, second]);

    assert_eq!(s.run("people.addressBook", &json!({})).unwrap(), json!({"account": first}));
    assert_eq!((s.view.module, shown_book(&s)), (Module::People, Some(first)));
    s.run("people.addressBook", &json!({"account": second})).unwrap();
    assert_eq!(shown_book(&s), Some(second));
    // Only an Exchange server account's, and nothing changes when one isn't.
    for bad in [json!({"account": 999}), json!({"account": "ada"}), json!({"account": -1})] {
        assert!(s.run("people.addressBook", &bad).is_err(), "{bad}");
        assert_eq!(shown_book(&s), Some(second));
    }
    s.run("view.peopleAccount", &json!({"value": "all"})).unwrap();
    assert_eq!(shown_book(&s), None);

    // An account that goes takes its address list along.
    s.run("people.addressBook", &json!({"account": second})).unwrap();
    s.run("account.remove", &json!({"id": second})).unwrap();
    assert_eq!(shown_book(&s), None);
}
