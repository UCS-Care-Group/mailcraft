// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Account Settings › Other addresses you send from (#797): shown for an IMAP account only,
//! saved through `account.setSenderAddresses`, and an address that isn't one keeps the page open.
use mailcraft_engine::Session;

use super::senders::Row;
use super::*;
use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

/// A window with an IMAP account and a Microsoft account; their ids.
fn app() -> (MailApp, Id, Id) {
    let mut a = MailApp::new(Session::at(mailcraft_model::Mailbox::default(), NOW, 0), Services::default());
    (a.autosave, a.auto_sync) = (false, false);
    let add = |a: &mut MailApp, params: Value| a.session.run("account.add", &params).unwrap()["id"].as_u64().unwrap();
    let imap = add(
        &mut a,
        json!({"email": "jorgen@company.example", "incoming": {"host": "imap.company.example"}, "outgoing": {"host": "smtp.company.example"}}),
    );
    let graph = add(&mut a, json!({"email": "jorgen@contoso.example", "kind": "microsoft"}));
    (a, imap, graph)
}

fn row(email: &str, name: &str) -> Row {
    Row { email: email.into(), name: name.into(), signature: String::new() }
}

#[test]
fn an_imap_accounts_other_addresses_are_shown_and_saved() {
    let (mut a, imap, graph) = app();
    assert!(AccountSetup::open(&a, &json!({"id": graph})).unwrap().senders.is_none());
    let mut w = AccountSetup::open(&a, &json!({"id": imap})).unwrap();
    assert_eq!(w.senders.as_ref().map(|x| x.rows.len()), Some(0));
    // Nothing changed: nothing is run for them.
    assert_eq!(w.senders.as_ref().unwrap().command(imap), None);
    // Two added, and a row left empty, which is no address.
    w.senders.as_mut().unwrap().rows = vec![row(" support@product.example ", "Product Support"), row("", ""), row("jorgen@product.example", "")];
    assert!(w.act(&mut a, Action::Go));
    let kept: Vec<(String, String)> =
        a.session.mailbox.account(imap).unwrap().sender_addresses.iter().map(|x| (x.email.clone(), x.display_name.clone())).collect();
    assert_eq!(kept, [("support@product.example".to_string(), "Product Support".to_string()), ("jorgen@product.example".to_string(), String::new())]);
    // The page opens again with them.
    let w = AccountSetup::open(&a, &json!({"id": imap})).unwrap();
    assert_eq!(w.senders.as_ref().map(|x| x.rows.len()), Some(2));
}

#[test]
fn an_address_that_isnt_one_keeps_the_page_open_and_saves_nothing() {
    let (mut a, imap, _) = app();
    let mut w = AccountSetup::open(&a, &json!({"id": imap})).unwrap();
    w.senders.as_mut().unwrap().rows = vec![row("support at product", "")];
    assert!(!w.act(&mut a, Action::Go));
    assert!(w.error.contains("support at product"), "{}", w.error);
    assert!(a.session.mailbox.account(imap).unwrap().sender_addresses.is_empty());
    // Cancel leaves them as they were.
    assert!(w.act(&mut a, Action::Back));
    assert!(a.session.mailbox.account(imap).unwrap().sender_addresses.is_empty());
}
