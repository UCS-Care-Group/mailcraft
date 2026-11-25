// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening and removing another person's address book by address, for an Exchange server account.

use serde_json::json;

use super::*;

const ME: &str = "ada@example.org";
const CY: &str = "cy@example.org";

fn session() -> (Session, Id) {
    let mut s = Session::empty();
    let added =
        s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": "https://exchange.example.org/EWS/Exchange.asmx"}})).unwrap();
    (s, added["id"].as_u64().unwrap())
}

#[test]
fn an_address_book_is_opened_and_removed() {
    let (mut s, account) = session();
    let opened = open(&mut s, &json!({"account": account, "address": CY})).unwrap();
    assert_eq!(opened["address"], CY);
    assert_eq!(s.mailbox.account(account).unwrap().opened_address_books.len(), 1);
    // (A second open of the same address changes nothing.)
    assert!(open(&mut s, &json!({"account": account, "address": CY})).is_err());
    let removed = remove(&mut s, &json!({"account": account, "address": CY})).unwrap();
    assert_eq!(removed["removed"], CY);
    assert!(s.mailbox.account(account).unwrap().opened_address_books.is_empty());
}

#[test]
fn an_account_that_is_not_an_exchange_server_refuses() {
    let mut s = Session::empty();
    let account = s.run("account.add", &json!({"email": ME})).unwrap()["id"].as_u64().unwrap();
    assert!(open(&mut s, &json!({"account": account, "address": CY})).is_err());
}
