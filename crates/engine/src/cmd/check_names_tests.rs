// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Check Names with the people an Exchange server account's address list has (#1262): what the
//! engine asks a host to look up, and what it does with what comes back.

use serde_json::{Value, json};

use super::*;
use crate::cmd::ews_mail::ews_resolve_request;

const ME: &str = "ada@example.org";
const URL: &str = "https://exchange.example.org/EWS/Exchange.asmx";

/// An Exchange server account with a message being written, and one contact, Maya Lopez.
fn session() -> Session {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": ME, "kind": "exchange", "ews": {"url": URL}})).unwrap();
    s.run("people.new", &json!({"first": "Maya", "last": "Lopez", "email": "maya@partner.example", "company": "Partner Co"})).unwrap();
    s.run("mail.new", &json!({})).unwrap();
    s
}

fn to(s: &mut Session, line: &str) {
    s.run("compose.set", &json!({"to": line})).unwrap();
}

fn check(s: &mut Session, params: Value) -> Value {
    s.run("compose.checkNames", &params).unwrap()
}

fn person(name: &str, email: &str) -> Value {
    json!({"name": name, "email": email, "kind": "mailbox", "department": "Engines"})
}

#[test]
fn the_names_nobody_matched_are_what_the_address_list_is_asked_for() {
    let mut s = session();
    to(&mut s, "maya; grace;  Grace ; bob@example.org; ");
    let answer = check(&mut s, json!({}));
    assert_eq!(answer["unresolved"], json!(["grace", "Grace"]));
    assert_eq!((answer["directory"].clone(), answer["choices"].clone()), (json!(true), json!([])));
    // The contact was put in; the address stayed as it was.
    assert_eq!(s.compose.as_ref().unwrap().to, "Maya Lopez <maya@partner.example>; grace; Grace; bob@example.org");

    let request = ews_resolve_request(&s, &["grace".into(), "Grace".into(), " a\r\nb ".into(), "".into()]).unwrap();
    assert_eq!(request.texts, ["grace", "a b"]);
    assert_eq!((request.email.as_str(), request.url.as_str()), (ME, URL));
    // The request's log line names nobody.
    let line = format!("{request:?}");
    assert!(!line.contains("grace") && !line.contains(URL) && !line.contains(ME), "{line}");
    let many: Vec<String> = (0..30).map(|i| format!("name{i}")).collect();
    assert_eq!(ews_resolve_request(&s, &many).unwrap().texts.len(), mailcraft_model::sync::MAX_RESOLVE_TEXTS);
}

#[test]
fn one_person_found_is_put_in_and_several_are_answered_as_choices() {
    let mut s = session();
    to(&mut s, "grace; ada; nobody");
    let found = json!({
        "Grace": [person("Grace Hopper", "grace@example.org")],
        "ada": [person("Ada Byron", "ada.byron@example.org"), person("Ada Lovelace", "ada.lovelace@example.org"), person("Ada again", "ADA.BYRON@example.org")],
        "nobody": [],
    });
    let answer = check(&mut s, json!({"found": found}));
    assert_eq!(s.compose.as_ref().unwrap().to, "Grace Hopper <grace@example.org>; ada; nobody");
    assert_eq!(answer["unresolved"], json!(["ada", "nobody"]));
    let choices = answer["choices"].as_array().unwrap();
    let [ada] = choices.as_slice() else { panic!("{choices:?}") };
    assert_eq!(ada["text"], "ada");
    // Each address once, with what tells them apart and where each was found.
    assert_eq!(
        ada["people"],
        json!([
            {"name": "Ada Byron", "email": "ada.byron@example.org", "from": "directory", "kind": "mailbox", "department": "Engines"},
            {"name": "Ada Lovelace", "email": "ada.lovelace@example.org", "from": "directory", "kind": "mailbox", "department": "Engines"},
        ])
    );

    // The owner picks one: that one is put in, though others match too.
    check(&mut s, json!({"pick": {"ada": {"name": "Ada Lovelace", "email": "ada.lovelace@example.org"}}}));
    assert_eq!(s.compose.as_ref().unwrap().to, "Grace Hopper <grace@example.org>; Ada Lovelace <ada.lovelace@example.org>; nobody");
}

#[test]
fn a_contact_and_somebody_else_in_the_address_list_are_both_offered() {
    let mut s = session();
    to(&mut s, "maya");
    // The same address in both is one person, and the contact stays put in at once.
    check(&mut s, json!({"found": {"maya": [person("Maya Lopez", "MAYA@partner.example")]}}));
    assert_eq!(s.compose.as_ref().unwrap().to, "Maya Lopez <maya@partner.example>");

    to(&mut s, "maya");
    let answer = check(&mut s, json!({"found": {"maya": [person("Maya Stone", "maya.stone@example.org")]}}));
    assert_eq!(s.compose.as_ref().unwrap().to, "maya");
    let from: Vec<&str> = answer["choices"][0]["people"].as_array().unwrap().iter().map(|p| p["from"].as_str().unwrap()).collect();
    assert_eq!(from, ["contacts", "directory"]);
    assert_eq!(answer["choices"][0]["people"][0]["company"], "Partner Co");
}

#[test]
fn what_comes_back_is_hostile_and_is_read_so() {
    let mut s = session();
    to(&mut s, "grace");
    for found in [
        // Addresses mail can't go to, and one that would add a header or a recipient.
        json!({"grace": [person("A", "not-an-address"), person("B", "a@b@example.org"), person("C", "x@example.org\r\nBcc: y@attacker.example"), person("D", "x@example.org, y@attacker.example")]}),
        // Not people at all.
        json!({"grace": [1, "x", null, {"email": 5}]}),
    ] {
        let answer = check(&mut s, json!({"found": found}));
        assert_eq!((s.compose.as_ref().unwrap().to.as_str(), answer["choices"].clone()), ("grace", json!([])));
    }
    // A name of many lines is one line when it is put in.
    check(&mut s, json!({"found": {"grace": [person("Grace\r\nBcc: y@attacker.example", "grace@example.org")]}}));
    assert_eq!(s.compose.as_ref().unwrap().to, "\"Grace Bcc: y@attacker.example\" <grace@example.org>");
    // Shapes that aren't the documented ones are refused, and nothing changes.
    to(&mut s, "grace");
    for bad in [json!({"found": []}), json!({"found": {"grace": "x"}}), json!({"pick": {"grace": {"email": "nope"}}}), json!({"pick": 3})] {
        assert!(s.run("compose.checkNames", &bad).is_err(), "{bad}");
        assert_eq!(s.compose.as_ref().unwrap().to, "grace");
    }
    // Many people for a name are read up to the cap.
    let many: Vec<Value> = (0..500).map(|i| person("Sam", &format!("sam{i}@example.org"))).collect();
    let answer = check(&mut s, json!({"found": {"grace": many}}));
    assert_eq!(answer["choices"][0]["people"].as_array().unwrap().len(), mailcraft_model::sync::MAX_RESOLVE_PEOPLE);
}

#[test]
fn an_account_that_is_no_exchange_server_account_has_no_address_list_to_ask() {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": ME, "name": "Ada", "kind": "imap"})).unwrap();
    s.run("mail.new", &json!({"to": "grace"})).unwrap();
    assert_eq!(check(&mut s, json!({}))["directory"], json!(false));
    assert!(ews_resolve_request(&s, &["grace".into()]).is_err());
}
