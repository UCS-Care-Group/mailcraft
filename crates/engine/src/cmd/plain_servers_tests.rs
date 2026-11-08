// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #836: no encryption is for a server on this computer alone (a mail bridge on 127.0.0.1), from
//! the window, the CLI and an agent alike, and an outgoing server may be a relay that takes mail
//! without a sign-in. Kept through a save and a load.

use mailcraft_model::{Account, AccountKind, Mailbox, Security, Server};
use serde_json::{Value, json};

use crate::Session;

fn session() -> Session {
    Session::new(Mailbox::with_local_account("Ada Lovelace", "ada@example.org"))
}

fn refusal(s: &mut Session, id: &str, params: Value) -> String {
    match s.run(id, &params) {
        Err(e) => e.to_string(),
        Ok(answer) => panic!("{id} {params} wasn't refused: {answer}"),
    }
}

/// The account `id` as the mailbox file has it after a save and a load.
fn reloaded(s: &Session, id: u64) -> Account {
    let file = crate::io::write_mailbox(&s.mailbox).unwrap();
    crate::io::read_mailbox(&file).unwrap().accounts.into_iter().find(|a| a.id == id).unwrap()
}

#[test]
fn no_encryption_is_taken_for_a_server_on_this_computer_and_refused_for_one_on_the_network() {
    let mut s = session();
    let bridge = json!({"email": "ada@proton.example", "incoming": {"host": "127.0.0.1", "port": 1143, "security": "none"},
        "outgoing": {"host": "localhost", "port": 1025, "security": "none"}});
    let id = s.run("account.add", &bridge).unwrap()["id"].as_u64().unwrap();
    let a = reloaded(&s, id);
    assert_eq!(a.incoming, Some(Server { host: "127.0.0.1".into(), port: 1143, security: Security::None }));
    assert_eq!(a.outgoing, Some(Server { host: "localhost".into(), port: 1025, security: Security::None }));

    for (key, host) in [("incoming", "mail.example.org"), ("outgoing", "192.0.2.25"), ("incoming", "localhost.example.org")] {
        let params = json!({"email": "bo@example.org", key: {"host": host, "security": "plain"}});
        let why = refusal(&mut s, "account.add", params);
        assert!(why.contains(&format!("`{key}.security`")) && why.contains("tls or starttls"), "{why}");
        let why = refusal(&mut s, "account.edit", json!({"id": id, key: {"host": host, "security": "none"}}));
        assert!(why.contains("this computer"), "{why}");
    }
    // Nothing of a refused edit was kept.
    assert_eq!(reloaded(&s, id).incoming.unwrap().host, "127.0.0.1");
    assert!(!s.mailbox.accounts.iter().any(|a| a.email == "bo@example.org"));
}

#[test]
fn a_file_that_already_has_one_on_the_network_can_still_be_edited_and_put_right() {
    let mut mb = Mailbox::with_local_account("Ada Lovelace", "ada@example.org");
    let old = Some(Server { host: "mail.example.org".into(), port: 143, security: Security::None });
    let id = mb.add_account(Account { email: "ada@old.example".into(), kind: AccountKind::Imap, incoming: old, ..Default::default() });
    let mut s = Session::new(mb);
    // What doesn't touch the server goes through; the server is refused only when it is set.
    s.run("account.edit", &json!({"id": id, "displayName": "Ada at home"})).unwrap();
    s.run("account.edit", &json!({"id": id, "incoming": {"host": "mail.example.org", "security": "starttls"}})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().incoming.as_ref().unwrap().security, Security::StartTls);
}

#[test]
fn a_relay_without_a_sign_in_is_a_choice_for_a_password_account_with_an_outgoing_server() {
    let mut s = session();
    let relay = json!({"email": "ada@relay.example", "incoming": {"host": "imap.relay.example"},
        "outgoing": {"host": "relay.example.org", "port": 25, "security": "starttls"}, "outgoingWithoutSignIn": true});
    let id = s.run("account.add", &relay).unwrap()["id"].as_u64().unwrap();
    assert!(reloaded(&s, id).outgoing_without_sign_in);
    let listed = s.run("account.list", &json!({})).unwrap();
    let shown = listed.as_array().or_else(|| listed["accounts"].as_array()).unwrap().iter().find(|a| a["id"] == id).unwrap().clone();
    assert_eq!(shown["outgoingWithoutSignIn"], true);

    // Turned off, and kept off.
    s.run("account.edit", &json!({"id": id, "outgoingWithoutSignIn": false})).unwrap();
    assert!(!reloaded(&s, id).outgoing_without_sign_in);
    assert!(refusal(&mut s, "account.edit", json!({"id": id, "outgoingWithoutSignIn": "yes"})).contains("true or false"));

    // Without an outgoing server there is nothing to choose; with a provider's sign-in a token always goes.
    s.run("account.edit", &json!({"id": id, "outgoingWithoutSignIn": true})).unwrap();
    s.run("account.edit", &json!({"id": id, "outgoing": null})).unwrap();
    assert!(!s.mailbox.account(id).unwrap().outgoing_without_sign_in);
    let cloud = json!({"email": "ada@cloud.example", "incoming": {"host": "imap.cloud.example"}, "outgoing": {"host": "smtp.cloud.example"},
        "auth": "oauth2", "outgoingWithoutSignIn": true});
    assert!(refusal(&mut s, "account.add", cloud).contains("password"));
    s.run("account.edit", &json!({"id": id, "outgoing": {"host": "smtp.relay.example"}, "outgoingWithoutSignIn": true})).unwrap();
    s.run("account.edit", &json!({"id": id, "auth": "oauth2"})).unwrap();
    assert!(!s.mailbox.account(id).unwrap().outgoing_without_sign_in);
}
