// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #836: an outgoing server may be a relay that takes mail without a sign-in, from the window, the
//! CLI and an agent alike. Kept through a save and a load.

use mailcraft_model::{Account, Mailbox};
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

#[test]
fn a_relay_tick_goes_with_the_outgoing_host_it_was_set_for() {
    let mut s = session();
    let relay = json!({"email": "ada@relay.example", "incoming": {"host": "imap.relay.example"},
        "outgoing": {"host": "relay.example.org", "port": 25, "security": "starttls"}, "outgoingWithoutSignIn": true});
    let id = s.run("account.add", &relay).unwrap()["id"].as_u64().unwrap();
    // The same host again (another port, another way of writing it) keeps it.
    s.run("account.edit", &json!({"id": id, "outgoing": {"host": " Relay.Example.org ", "port": 587, "security": "starttls"}})).unwrap();
    assert!(reloaded(&s, id).outgoing_without_sign_in);
    // Another host starts without it.
    s.run("account.edit", &json!({"id": id, "outgoing": {"host": "smtp.relay.example"}})).unwrap();
    assert!(!reloaded(&s, id).outgoing_without_sign_in);
    // Unless the same call says it is a relay too.
    s.run("account.edit", &json!({"id": id, "outgoingWithoutSignIn": true})).unwrap();
    s.run("account.edit", &json!({"id": id, "outgoing": {"host": "other.relay.example"}, "outgoingWithoutSignIn": true})).unwrap();
    assert!(reloaded(&s, id).outgoing_without_sign_in);
}
