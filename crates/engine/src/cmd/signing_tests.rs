// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #30 part 5: Sign in a message being written, an account's key and Always, and a message to
//! be signed that its host couldn't sign held in the Outbox.

use mailcraft_model::signing::Unsigned;
use mailcraft_model::sync::SyncResult;
use mailcraft_model::{FolderKind, Id};
use serde_json::json;

use crate::Session;
use crate::cmd::account::sync_request;

const FP: &str = "abababababababababababababababababababababababababababababababab";

fn imap() -> (Session, Id) {
    let mut s = Session::empty();
    let a = s.run("account.add", &json!({"email": "ada@mail.example", "incoming": {"host": "imap.mail.example"}})).unwrap()["id"].as_u64().unwrap();
    (s, a)
}

fn key(s: &mut Session, a: Id, always: bool) {
    s.run("account.setSigning", &json!({"account": a, "key": {"format": "smime", "fingerprint": FP, "label": "Ada"}, "always": always})).unwrap();
}

#[test]
fn a_message_sent_with_sign_on_carries_the_accounts_key() {
    let (mut s, a) = imap();
    key(&mut s, a, false);
    assert_eq!(s.mailbox.account(a).unwrap().signing.key.as_ref().unwrap().fingerprint, FP.to_ascii_uppercase());
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Signed", "account": a})).unwrap();
    let on = s.run("compose.sign", &json!({})).unwrap();
    assert_eq!(on["sign"], json!(true), "toggled on from the account's default, off");
    assert_eq!(on["key"], json!("Ada"));
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let m = sync_request(&s, a).unwrap().outbox.into_iter().find(|m| m.id == id).unwrap();
    assert_eq!(m.sign_with.unwrap().fingerprint, FP.to_ascii_uppercase());
}

#[test]
fn always_signs_and_sign_off_in_the_message_doesnt() {
    let (mut s, a) = imap();
    key(&mut s, a, true);
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Unsigned", "account": a})).unwrap();
    assert_eq!(s.run("compose.sign", &json!({"value": false})).unwrap()["sign"], json!(false));
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(s.mailbox.message(id).unwrap().sign_with.is_none());
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Signed", "account": a})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    assert!(s.mailbox.message(id).unwrap().sign_with.is_some(), "Always");
}

#[test]
fn sign_without_a_key_refuses_the_send_and_keeps_the_message() {
    let (mut s, a) = imap();
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "No key", "account": a})).unwrap();
    s.run("compose.sign", &json!({"value": true})).unwrap();
    let e = s.run("compose.send", &json!({})).unwrap_err().to_string();
    assert!(e.contains("choose the certificate"), "{e}");
    assert!(s.compose.is_some(), "the message is still being written");
}

#[test]
fn bad_keys_and_values_are_refused() {
    let (mut s, a) = imap();
    assert!(s.run("account.setSigning", &json!({"account": a, "key": {"format": "smime", "fingerprint": "not hex"}})).is_err());
    assert!(s.run("account.setSigning", &json!({"account": a, "key": {"format": "x509", "fingerprint": FP}})).is_err());
    assert!(s.run("account.setSigning", &json!({"account": a, "key": 7})).is_err());
    assert!(s.mailbox.account(a).unwrap().signing.key.is_none());
    s.run("mail.new", &json!({"account": a})).unwrap();
    assert!(s.run("compose.sign", &json!({"value": "yes"})).is_err());
}

#[test]
fn a_message_that_couldnt_be_signed_is_held_with_why() {
    let (mut s, a) = imap();
    key(&mut s, a, true);
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Rota", "account": a})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let unsigned = vec![Unsigned { message: id, why: "the smartcard needs its PIN".into() }];
    let email = s.mailbox.account(a).unwrap().email.clone();
    s.run("sync.apply", &json!({"result": SyncResult { account: a, email, unsigned, ..Default::default() }})).unwrap();
    let m = s.mailbox.message(id).unwrap();
    assert_eq!(Some(m.folder), s.mailbox.folder_of_kind(a, FolderKind::Outbox));
    assert!(m.held.contains("PIN"), "{}", m.held);
    assert!(sync_request(&s, a).unwrap().outbox.iter().all(|m| m.id != id), "not sent again by itself");
}

#[test]
fn always_signs_what_goes_by_another_way_than_send() {
    let (mut s, a) = imap();
    key(&mut s, a, true);
    // Sent with Sign off, then put back in the Outbox and sent with Send Again (a meeting's
    // answer goes otherwise than with Send alike): Always signs it.
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Again", "account": a})).unwrap();
    s.run("compose.sign", &json!({"value": false})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    s.mailbox.message_mut(id).unwrap().sign = None;
    let m = sync_request(&s, a).unwrap().outbox.into_iter().find(|m| m.id == id).unwrap();
    assert!(m.sign_with.is_some(), "the account's Always");
    s.mailbox.message_mut(id).unwrap().sign = Some(false);
    let m = sync_request(&s, a).unwrap().outbox.into_iter().find(|m| m.id == id).unwrap();
    assert!(m.sign_with.is_none(), "Sign off for that message");
}

#[test]
fn always_needs_a_key() {
    let (mut s, a) = imap();
    assert!(s.run("account.setSigning", &json!({"account": a, "always": true})).is_err());
    key(&mut s, a, true);
    assert!(s.run("account.setSigning", &json!({"account": a, "key": null})).is_err(), "not while Always is on");
    s.run("account.setSigning", &json!({"account": a, "key": null, "always": false})).unwrap();
}
