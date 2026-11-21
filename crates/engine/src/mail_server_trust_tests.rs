// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The one certificate an owner trusts for an IMAP or SMTP server (#1413): what `account.add` and
//! `account.edit` take of it, that it goes to the host with the account, and that the mailbox file
//! keeps it.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::Session;

/// A fingerprint, as setup has it: 64 lower-case hexadecimal digits.
fn fingerprint() -> String {
    "ab".repeat(32)
}

fn bridge(trusted: Value) -> Value {
    json!({
        "email": "ada@example.org",
        "incoming": {"host": "127.0.0.1", "port": 1143, "security": "starttls", "trusted": trusted},
        "outgoing": {"host": "127.0.0.1", "port": 1025, "security": "starttls"},
    })
}

fn said(result: Result<Value, crate::CmdError>) -> String {
    result.expect_err("should have been refused").to_string()
}

#[test]
fn a_trusted_certificate_is_kept_with_its_server_alone() {
    let mut s = Session::empty();
    // Any case is taken; it is kept as setup writes it.
    let id = s.run("account.add", &bridge(json!(fingerprint().to_ascii_uppercase()))).unwrap()["id"].as_u64().unwrap();
    let a = s.mailbox.account(id).unwrap();
    assert_eq!(a.incoming.as_ref().map(|i| i.trusted.as_str()), Some(fingerprint().as_str()));
    // The outgoing server trusts nothing it wasn't given: each server is asked about on its own.
    assert_eq!(a.outgoing.as_ref().map(|o| o.trusted.as_str()), Some(""));
    // It goes to the host with the account, as the rest of the server's settings do.
    let ask = s.run("sync.request", &json!({"account": id})).unwrap();
    assert_eq!(ask["account"]["incoming"]["trusted"], json!(fingerprint()));
    assert!(ask["account"]["outgoing"].get("trusted").is_none(), "{}", ask["account"]["outgoing"]);
}

#[test]
fn what_is_no_fingerprint_is_refused_and_changes_nothing() {
    let mut s = Session::empty();
    let before = s.mailbox.accounts.clone();
    for bad in [json!("not-a-fingerprint"), json!("ab".repeat(31)), json!(5), json!(["ab"]), json!(format!("{}zz", "ab".repeat(31)))] {
        let why = said(s.run("account.add", &bridge(bad.clone())));
        assert!(why.contains("incoming.trusted"), "{bad}: {why}");
    }
    assert_eq!(s.mailbox.accounts, before);
    // Empty is none.
    let id = s.run("account.add", &bridge(json!(""))).unwrap()["id"].as_u64().unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().incoming.as_ref().map(|i| i.trusted.as_str()), Some(""));
}

#[test]
fn an_edit_that_names_the_server_again_keeps_or_drops_the_trust_as_it_says() {
    let mut s = Session::empty();
    let id = s.run("account.add", &bridge(json!(fingerprint()))).unwrap()["id"].as_u64().unwrap();
    // An edit that doesn't touch the servers leaves the trust where it was.
    s.run("account.edit", &json!({"id": id, "displayName": "Ada"})).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().incoming.as_ref().map(|i| i.trusted.clone()), Some(fingerprint()));
    // The server given again, with it: kept. Without it: a server that trusts only what the system does.
    let mut again = bridge(json!(fingerprint()));
    again["id"] = json!(id);
    s.run("account.edit", &again).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().incoming.as_ref().map(|i| i.trusted.clone()), Some(fingerprint()));
    again["incoming"].as_object_mut().unwrap().remove("trusted");
    s.run("account.edit", &again).unwrap();
    assert_eq!(s.mailbox.account(id).unwrap().incoming.as_ref().map(|i| i.trusted.clone()), Some(String::new()));
}

#[test]
fn the_mailbox_file_keeps_the_trust_and_a_server_without_one_is_written_as_before() {
    let dir: PathBuf = std::env::temp_dir().join(format!("mailcraft-trust-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mailbox.mailcraft");
    let mut s = Session::empty();
    let id = s.run("account.add", &bridge(json!(fingerprint()))).unwrap()["id"].as_u64().unwrap();
    crate::store::save_single(&path, &s.mailbox).unwrap();
    let text = String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned();
    // Once, for the incoming server: a server with none writes no `trusted` at all.
    assert_eq!(text.matches(r#""trusted""#).count(), 1, "{text}");
    let opened = crate::store::open(&path).unwrap();
    assert_eq!(opened.mailbox.account(id).unwrap().incoming, s.mailbox.account(id).unwrap().incoming);
    let _ = std::fs::remove_dir_all(&dir);
}
