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
fn forgetting_the_trusted_device_takes_the_kept_token_and_the_sessions_sign_in() {
    let (mut a, imap, id) = app();
    let account = a.session.mailbox.account(id).unwrap().clone();
    // As a sign-in that brought a trusted-device token left it: kept under the account's own
    // name (the token and the device id, a line each), and this session's sign-in token.
    a.secret_cache.insert(MailApp::secret_key("trust", &account), "tok-1\ndevice-1".into());
    a.zimbra_sessions.insert(id, ("sess-1".into(), mailcraft_time::now() as u64 + 3_600));
    assert!(a.zimbra_trusted(id).is_some());

    let said = a.execute("ui.forgetZimbraTrust", json!({"id": id})).unwrap();
    assert_eq!(said, json!({"id": id, "forgotten": true}));
    assert!(a.zimbra_trusted(id).is_none());
    assert!(!a.zimbra_sessions.contains_key(&id));
    // The password stays: only the trust was the owner's to take back.
    a.passwords.insert(id, "kept".into());
    assert_eq!(a.passwords.get(&id).map(String::as_str), Some("kept"));

    // Once forgotten there is nothing to forget; a second ask answers so.
    assert_eq!(a.execute("ui.forgetZimbraTrust", json!({"id": id})).unwrap()["forgotten"], json!(false));

    // Only a Zimbra account's, and never an id that is none.
    assert!(a.execute("ui.forgetZimbraTrust", json!({"id": imap})).is_err());
    assert!(a.execute("ui.forgetZimbraTrust", json!({"id": 987_654})).is_err());
    for junk in [json!(null), json!({}), json!({"id": "x"}), json!({"id": -1})] {
        assert!(a.execute("ui.forgetZimbraTrust", junk).is_err());
    }
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
