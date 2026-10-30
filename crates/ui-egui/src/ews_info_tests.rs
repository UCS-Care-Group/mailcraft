// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exchange server accounts, the skeleton (#252): File › Info says why one isn't kept in step, and
//! no Send/Receive of the window takes it in.

use mailcraft_engine::Session;
use mailcraft_model::{Account, AccountKind, EwsServer, Id};
use serde_json::json;

use crate::{MailApp, Services};

const NOW: i64 = 1_791_993_600;

fn app(ews: bool) -> (MailApp, Id, Id) {
    let mut session = Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0);
    session.ews = ews;
    let mut a = MailApp::new(session, Services::default());
    a.autosave = false;
    let imap =
        a.execute("account.add", json!({"email": "ada@imap.example", "incoming": {"host": "imap.example.org"}})).unwrap()["id"].as_u64().unwrap();
    let server = EwsServer { url: "https://mail.example.org:444/EWS/Exchange.asmx".into(), ..Default::default() };
    let exchange = Account { email: "ada@exchange.example".into(), kind: AccountKind::Exchange, ews: Some(server), ..Default::default() };
    let exchange = a.session.mailbox.add_account(exchange);
    (a, imap, exchange)
}

#[test]
fn file_info_says_why_an_exchange_account_is_not_kept_in_step() {
    let (a, _, id) = app(false);
    let card = crate::backstage::account_detail(&a, a.session.mailbox.account(id).unwrap());
    assert!(card.starts_with("Exchange server mail.example.org") && card.contains("MAILCRAFT_EWS=1"), "{card}");
    assert!(!card.contains("Exchange.asmx") && !card.contains(":444"), "{card}");
    let (a, _, id) = app(true);
    let card = crate::backstage::account_detail(&a, a.session.mailbox.account(id).unwrap());
    assert_eq!(card, "Exchange server mail.example.org · not kept in step yet: this part is not built");
}

#[test]
fn an_exchange_account_with_no_usable_address_says_so() {
    let (mut a, _, id) = app(false);
    a.session.mailbox.account_mut(id).unwrap().ews = Some(EwsServer { url: "http://elsewhere.example/".into(), ..Default::default() });
    let card = crate::backstage::account_detail(&a, a.session.mailbox.account(id).unwrap());
    assert_eq!(card, "Exchange server · no usable server address");
}

/// The name a password is kept under names the address and the whole EWS address, so that an
/// account that names another server (a mailbox from a file, a changed setting) is not given it.
#[test]
fn an_exchange_accounts_secret_is_kept_under_its_address_and_its_servers_address() {
    let (a, imap, id) = app(true);
    let exchange = a.session.mailbox.account(id).unwrap();
    let key = MailApp::secret_key("password", exchange);
    assert_eq!(key, "password:ada@exchange.example|ews=https://mail.example.org:444/EWS/Exchange.asmx");
    // Another server, or another address, is another key; the user name and the trusted
    // certificate are no part of it; and it is not an IMAP account's.
    let other = |change: &dyn Fn(&mut Account)| {
        let mut account = exchange.clone();
        change(&mut account);
        MailApp::secret_key("password", &account)
    };
    assert_ne!(other(&|x| x.ews.as_mut().unwrap().url = "https://elsewhere.example/EWS/Exchange.asmx".into()), key);
    assert_ne!(other(&|x| x.email = "bo@exchange.example".into()), key);
    assert_eq!(other(&|x| x.ews.as_mut().unwrap().username = "CORP\\ada".into()), key);
    assert_eq!(other(&|x| x.ews.as_mut().unwrap().trusted = "ab".repeat(32)), key);
    assert_ne!(MailApp::secret_key("refresh", exchange), key);
    assert!(!MailApp::secret_key("password", a.session.mailbox.account(imap).unwrap()).contains("ews="));
    // One that has no server settings gets a key that no server's is.
    assert_eq!(other(&|x| x.ews = None), "password:ada@exchange.example|ews=none");
}

#[test]
fn no_send_receive_of_the_window_takes_an_exchange_account_in() {
    for on in [false, true] {
        let (a, imap, _) = app(on);
        assert_eq!(a.server_accounts(), vec![imap], "switch {on}");
    }
}
