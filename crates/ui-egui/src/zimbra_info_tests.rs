// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Zimbra accounts, the skeleton (Z1): File › Info says where one connects and why it isn't kept in
//! step, and its password is kept under a name of its own server's.

use mailcraft_engine::Session;
use mailcraft_model::{Account, AccountKind, Id, ZimbraServer};
use serde_json::json;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app() -> (MailApp, Id, Id) {
    let session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    let imap =
        a.execute("account.add", json!({"email": "ada@imap.example", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let server = ZimbraServer { url: "https://zimbra.example.org/".into(), ..Default::default() };
    let zimbra = Account { email: "ada@zimbra.example".into(), kind: AccountKind::Zimbra, zimbra: Some(server), ..Default::default() };
    let zimbra = a.session.mailbox.add_account(zimbra);
    (a, imap, zimbra)
}

#[test]
fn file_info_names_a_zimbra_accounts_server_and_why_it_waits() {
    let (mut a, _, id) = app();
    let card = crate::backstage::account_detail(&a, a.session.mailbox.account(id).unwrap());
    assert!(card.starts_with("Zimbra zimbra.example.org · not kept in step") && card.contains("MAILCRAFT_ZIMBRA=1"), "{card}");
    a.session.zimbra = true;
    let card = crate::backstage::account_detail(&a, a.session.mailbox.account(id).unwrap());
    assert_eq!(card, "Zimbra zimbra.example.org · not kept in step yet: this part is not built");
    a.session.mailbox.account_mut(id).unwrap().zimbra = Some(ZimbraServer { url: "http://elsewhere.example/".into(), ..Default::default() });
    let card = crate::backstage::account_detail(&a, a.session.mailbox.account(id).unwrap());
    assert_eq!(card, "Zimbra · no usable server address");
}

#[test]
fn a_zimbra_accounts_secret_is_kept_under_its_address_and_its_server() {
    let (a, imap, id) = app();
    let zimbra = a.session.mailbox.account(id).unwrap().clone();
    let key = MailApp::secret_key("password", &zimbra);
    assert_eq!(key, "password:ada@zimbra.example|zimbra=https://zimbra.example.org|user=");
    let mut other = zimbra.clone();
    other.zimbra.as_mut().unwrap().url = "https://other.example.org".into();
    let mut renamed = zimbra.clone();
    renamed.zimbra.as_mut().unwrap().username = "bob".into();
    let mut pinned = zimbra.clone();
    pinned.zimbra.as_mut().unwrap().trusted = "ab".repeat(32);
    for changed in [&other, &renamed, &pinned] {
        assert_ne!(MailApp::secret_key("password", changed), key);
    }
    assert_ne!(MailApp::secret_key("password", a.session.mailbox.account(imap).unwrap()), key);
}
