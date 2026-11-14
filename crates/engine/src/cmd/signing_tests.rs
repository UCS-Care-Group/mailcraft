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

/// Security review of #1036: a message held because it couldn't be signed is said among the
/// run's problems, not only in passing: the run's answer can't say all is well without it.
#[test]
fn a_message_held_unsigned_is_among_the_runs_problems() {
    let (mut s, a) = imap();
    key(&mut s, a, true);
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Rota", "account": a})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let unsigned = vec![Unsigned { message: id, why: "the smartcard needs its PIN".into() }];
    let email = s.mailbox.account(a).unwrap().email.clone();
    let answer = s.run("sync.apply", &json!({"result": SyncResult { account: a, email, unsigned, ..Default::default() }})).unwrap();
    let errors = answer["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e.as_str().is_some_and(|e| e.contains("Rota") && e.contains("PIN"))), "{answer}");
}

/// Security review of #1036: a message to be signed whose account has no key that can be used
/// (none, or one that can't be read from the file) is never handed out unsigned; it carries a
/// key nothing signs with, the run holds it with why, and Send Again doesn't send it unsigned
/// either.
#[test]
fn a_message_to_be_signed_without_a_usable_key_is_never_handed_out_unsigned() {
    use mailcraft_model::digital_signature::SignatureFormat;
    use mailcraft_model::signing::{AccountSigning, SigningKey};
    let (mut s, a) = imap();
    s.run("mail.new", &json!({"to": "maya@lumen.example", "subject": "Unreadable key", "account": a})).unwrap();
    let id = s.run("compose.send", &json!({})).unwrap()["id"].as_u64().unwrap();
    let unreadable = SigningKey { format: SignatureFormat::Smime, fingerprint: "not a fingerprint".into(), label: "Ada".into() };
    for (signing, sign) in [
        // Always, with a key the file has spoiled.
        (AccountSigning { key: Some(unreadable.clone()), always: true }, None),
        // Always, and no key at all (a file written by hand).
        (AccountSigning { key: None, always: true }, None),
        // Sign on for the message, and no key.
        (AccountSigning::default(), Some(true)),
    ] {
        s.mailbox.account_mut(a).unwrap().signing = signing.clone();
        s.mailbox.message_mut(id).unwrap().sign = sign;
        let m = sync_request(&s, a).unwrap().outbox.into_iter().find(|m| m.id == id).unwrap();
        let key = m.sign_with.clone().expect("never handed out without a key to sign with");
        assert!(key.checked().is_none(), "{signing:?}: nothing can sign with it");
        assert!(mailcraft_mime_model::signature_missing(&m), "{signing:?}: every sending path refuses it");
    }
    // The host says it couldn't sign it: held with a notice.
    let unsigned = vec![Unsigned { message: id, why: crate::cmd::signing::NO_USABLE_KEY.into() }];
    let email = s.mailbox.account(a).unwrap().email.clone();
    s.run("sync.apply", &json!({"result": SyncResult { account: a, email: email.clone(), unsigned: unsigned.clone(), ..Default::default() }}))
        .unwrap();
    assert!(s.mailbox.message(id).unwrap().held.contains("Digital Signatures"), "{}", s.mailbox.message(id).unwrap().held);
    assert!(sync_request(&s, a).unwrap().outbox.iter().all(|m| m.id != id), "held");
    // Send Again (#704): it goes out to be signed again, still not unsigned.
    s.run("mail.sendAgain", &json!({"id": id})).unwrap();
    let m = sync_request(&s, a).unwrap().outbox.into_iter().find(|m| m.id == id).unwrap();
    assert!(m.sign_with.is_some_and(|k| k.checked().is_none()), "Send Again doesn't send it unsigned");
}
