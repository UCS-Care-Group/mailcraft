// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Add Account and Account Settings for a Zimbra account: a tile only while Zimbra accounts are
//! being tested, no mail servers anywhere, the server's address checked before anything is added,
//! and a change of it saved as `account.edit` takes it.

use mailcraft_engine::Session;
use serde_json::json;

use super::*;
use crate::Services;

const NOW: i64 = 1_791_993_600;
const ME: &str = "ada@example.org";

fn app(zimbra: bool) -> MailApp {
    let mut session = Session::at(mailcraft_model::Mailbox::with_local_account("", ""), NOW, 0);
    session.zimbra = zimbra;
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    a
}

#[test]
fn the_tile_is_there_only_while_zimbra_accounts_are_tested() {
    assert!(!Kind::tiles(false).contains(&Kind::Zimbra));
    assert!(Kind::tiles(true).contains(&Kind::Zimbra));
    assert!(!Kind::Zimbra.title().contains("IMAP") && Kind::Zimbra.detail().contains("8.8"));
}

#[test]
fn the_zimbra_page_adds_an_account_without_mail_servers() {
    let mut a = app(true);
    let mut w = AccountSetup::open(&a, &json!({})).unwrap();
    w.email = ME.into();
    w.pick(&mut a, Kind::Zimbra);
    assert_eq!(w.page, Page::Zimbra);
    // The domain's mail host is the guess, which the owner may change.
    assert_eq!(w.zimbra.url, "https://mail.example.org");
    for (typed, ok) in
        [("http://mail.example.org", false), ("https://mail.example.org/?zauthtoken=x", false), ("", false), ("mail.example.org", true)]
    {
        w.zimbra.url = typed.into();
        assert_eq!(w.zimbra_problem().is_none(), ok, "{typed}");
    }
    let (command, params) = w.zimbra_command().unwrap();
    assert_eq!(command, "account.add");
    assert_eq!(params["kind"], "zimbra");
    assert_eq!(params["zimbra"]["url"], "https://mail.example.org");
    assert!(params.get("incoming").is_none() && params.get("outgoing").is_none());
    w.password = Secret("not-a-real-password".into());
    w.zimbra_connect(&mut a);
    let account = a.session.mailbox.accounts.iter().find(|x| x.email == ME).unwrap();
    assert_eq!(account.kind, AccountKind::Zimbra);
    assert!(account.incoming.is_none() && account.outgoing.is_none());
}

#[test]
fn account_settings_of_a_zimbra_account_change_its_server_and_nothing_of_imap() {
    let mut a = app(true);
    let id = a.execute("account.add", json!({"email": ME, "kind": "zimbra", "zimbra": {"url": "https://mail.example.org"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
    assert!(w.zimbra_account && w.zimbra.url == "https://mail.example.org");
    assert_eq!(w.edit_command(), None);
    w.zimbra.url = "https://zimbra.example.org".into();
    w.username = "ada".into();
    let (_, params) = w.edit_command().unwrap();
    assert_eq!(params, json!({"id": id, "zimbra": {"url": "https://zimbra.example.org", "username": "ada"}}));
    assert!(w.act(&mut a, Action::Go));
    let server = a.session.mailbox.account(id).unwrap().zimbra.clone().unwrap();
    assert_eq!((server.url.as_str(), server.username.as_str()), ("https://zimbra.example.org", "ada"));
    // An address that can't be used is not saved.
    let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
    w.zimbra.url = "http://plain.example.org".into();
    assert_eq!(w.edit_command(), None);
}
