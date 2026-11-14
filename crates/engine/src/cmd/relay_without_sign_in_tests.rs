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

#[test]
fn the_question_for_an_agents_account_edit_names_the_tick() {
    let mut s = session();
    let relay = json!({"email": "ada@relay.example", "incoming": {"host": "imap.relay.example"},
        "outgoing": {"host": "relay.example.org", "port": 25, "security": "starttls"}});
    let id = s.run("account.add", &relay).unwrap()["id"].as_u64().unwrap();
    let ask = |s: &Session, params: Value| crate::agent::describe(s, "account.edit", &params);
    let set = ask(&s, json!({"id": id, "outgoingWithoutSignIn": true}));
    assert!(set.contains("the outgoing server needs no sign-in"), "{set}");
    // Said once, and what it means for the password.
    assert!(set.contains("never sent"), "{set}");
    // Already set, nothing changes: nothing to name.
    s.run("account.edit", &json!({"id": id, "outgoingWithoutSignIn": true})).unwrap();
    assert!(!ask(&s, json!({"id": id, "outgoingWithoutSignIn": true})).contains("sign-in"));
    assert!(!ask(&s, json!({"id": id, "displayName": "Ada"})).contains("sign-in"));
    // Taken off again.
    let off = ask(&s, json!({"id": id, "outgoingWithoutSignIn": false}));
    assert!(off.contains("signs in to the outgoing server again"), "{off}");
    // Another host clears it, and the question says so.
    let moved = ask(&s, json!({"id": id, "outgoing": {"host": "smtp.other.example"}}));
    assert!(moved.contains("signs in to the outgoing server again") && moved.contains("smtp.other.example"), "{moved}");
}
