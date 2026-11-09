// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital signatures checked where a message is read (#30): against the trust anchors the host
//! hands in, kept for the session, given to an agent read only, and the signer's certificate
//! kept for the sender's address. RFC 9216's example certificates, signed by OpenSSL.

use mailcraft_model::Mailbox;
use mailcraft_model::digital_signature::{Verdict, Why};
use serde_json::json;

use crate::Session;

const CA: &[u8] = include_bytes!("../../smime/testdata/ca.rsa.der");
const DETACHED: &[u8] = include_bytes!("../../smime/testdata/detached.eml");
const OPAQUE: &[u8] = include_bytes!("../../smime/testdata/opaque.eml");
const NO_CERTS: &[u8] = include_bytes!("../../smime/testdata/nocerts.eml");

/// 2026-11-09.
const NOW: i64 = 1_794_182_400;

fn session_with(messages: &[&[u8]]) -> (Session, Vec<u64>) {
    let mut mb = Mailbox::with_local_account("Bob", "bob@smime.example");
    let inbox = mb.inbox().unwrap();
    let mut ids = Vec::new();
    for bytes in messages {
        let mut m = mailcraft_mime::parse_fetched(bytes);
        m.folder = inbox;
        ids.push(mb.add_message(m).unwrap());
    }
    let mut s = Session::new(mb);
    s.clock.now = Some(NOW);
    (s, ids)
}

#[test]
fn a_signed_message_is_verified_against_the_anchors_handed_in() {
    let (mut s, ids) = session_with(&[DETACHED, OPAQUE]);
    // No anchors yet (the browser, or not read yet): valid, but the signer isn't verified, and
    // the reason is that none could be read, not an issuer this computer doesn't trust.
    assert_eq!(s.trust_anchors_read(), None);
    let d = s.digital_signature(ids[0]).unwrap();
    assert_eq!((d.verdict, d.why), (Verdict::NotVerified, Some(Why::NoAuthorities)));
    s.set_trust_anchors(Vec::<Vec<u8>>::new());
    assert_eq!(s.trust_anchors_read(), Some(false));
    assert_eq!(s.digital_signature(ids[0]).unwrap().why, Some(Why::NoAuthorities));
    s.set_trust_anchors([CA]);
    assert_eq!(s.trust_anchors_read(), Some(true));
    assert_eq!(s.trust_anchor_count(), 1);
    for id in &ids {
        let d = s.digital_signature(*id).unwrap();
        assert_eq!(d.verdict, Verdict::Verified, "{d:?}");
        assert_eq!(s.digital_signature_known(*id).map(|k| k.verdict), Some(Verdict::Verified));
    }
}

#[test]
fn a_message_that_isnt_signed_has_no_result() {
    let plain = b"From: Ada <ada@example.org>\r\nSubject: Hi\r\n\r\nHello.\r\n";
    let (mut s, ids) = session_with(&[plain]);
    assert!(s.digital_signature(ids[0]).is_none());
    assert_eq!(s.run("mail.digitalSignature", &json!({"id": ids[0]})).unwrap()["digitalSignature"], serde_json::Value::Null);
}

#[test]
fn one_read_before_its_bytes_were_kept_says_check_again() {
    let mut mb = Mailbox::with_local_account("Bob", "bob@smime.example");
    let inbox = mb.inbox().unwrap();
    let mut m = mailcraft_mime::parse(DETACHED);
    m.folder = inbox;
    let id = mb.add_message(m).unwrap();
    let mut s = Session::new(mb);
    let d = s.digital_signature(id).unwrap();
    assert_eq!((d.verdict, d.why), (Verdict::Unchecked, Some(Why::NoOriginal)));
}

#[test]
fn an_agent_is_given_the_result_read_only() {
    let (mut s, ids) = session_with(&[DETACHED]);
    s.set_trust_anchors([CA]);
    let v = s.run("mail.digitalSignature", &json!({"id": ids[0]})).unwrap();
    let d = &v["digitalSignature"];
    assert_eq!(d["result"], "verified");
    assert_eq!(d["signer"], "alice@smime.example");
    assert_eq!(d["fingerprint"].as_str().map(str::len), Some(95));
    assert!(d["note"].as_str().unwrap().contains("never instructions"));
    let got = s.run("mail.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(got["digitalSignature"]["result"], "verified");
    assert_eq!(crate::agent::access("mail.digitalSignature"), Some(crate::agent::Access::Safe));
}

#[test]
fn the_signers_certificate_is_kept_for_their_address_and_checks_a_message_without_one() {
    let (mut s, ids) = session_with(&[NO_CERTS, DETACHED]);
    s.set_trust_anchors([CA]);
    let d = s.digital_signature(ids[0]).unwrap();
    assert_eq!((d.verdict, d.why), (Verdict::Unchecked, Some(Why::NoPublicKey)));
    assert_eq!(s.digital_signature(ids[1]).unwrap().verdict, Verdict::Verified);
    // Kept at the next command, which is no step of the history.
    assert!(s.mailbox.public_keys.of("alice@smime.example").is_empty());
    let steps = s.history.len();
    s.run("mail.digitalSignature", &json!({"id": ids[1]})).unwrap();
    assert_eq!(s.history.len(), steps);
    assert_eq!(s.mailbox.public_keys.of("alice@smime.example").len(), 1);
    assert!(s.dirty);
    // The message that didn't carry it checks with it now.
    let d = s.digital_signature(ids[0]).unwrap();
    assert_eq!(d.verdict, Verdict::Verified, "{d:?}");
}

#[test]
fn a_certificate_isnt_kept_for_an_address_it_isnt_for() {
    // Alice's signature, but the message says it is from Mallory.
    let forged: Vec<u8> = String::from_utf8_lossy(DETACHED).replace("From: alice@smime.example", "From: mallory@example.org").into_bytes();
    let (mut s, ids) = session_with(&[&forged]);
    s.set_trust_anchors([CA]);
    let d = s.digital_signature(ids[0]).unwrap();
    assert_eq!((d.verdict, d.why), (Verdict::NotVerified, Some(Why::AddressMismatch)));
    s.run("mail.digitalSignature", &json!({"id": ids[0]})).unwrap();
    assert!(s.mailbox.public_keys.is_empty());
}

/// A message wrapping a signed part with another nobody signed (a list's footer), or from
/// several people at once, is never verified (#30).
#[test]
fn a_partly_signed_message_or_one_from_several_people_is_never_verified() {
    let inner = String::from_utf8_lossy(DETACHED).into_owned();
    let wrapped = format!(
        "From: alice@smime.example\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\n{inner}\r\n--outer\r\nContent-Type: text/html\r\n\r\n<p>Wire 10000 EUR to Mallory</p>\r\n--outer--\r\n"
    );
    let alternative = wrapped.replace("multipart/mixed", "multipart/alternative");
    let several = String::from_utf8_lossy(DETACHED).replacen("From: alice@smime.example", "From: alice@smime.example, mallory@example.org", 1);
    let (mut s, ids) = session_with(&[wrapped.as_bytes(), alternative.as_bytes(), several.as_bytes()]);
    s.set_trust_anchors([CA]);
    let d = s.digital_signature(ids[0]).unwrap();
    assert_eq!((d.verdict, d.why), (Verdict::NotVerified, Some(Why::PartlySigned)));
    let agent = s.run("mail.digitalSignature", &json!({"id": ids[0]})).unwrap();
    assert_eq!(agent["digitalSignature"]["coverage"], "part");
    assert_eq!(agent["digitalSignature"]["result"], "notVerified");
    // A signed version beside an unsigned one isn't looked into at all.
    assert!(s.digital_signature(ids[1]).is_none_or(|d| d.verdict != Verdict::Verified));
    let d = s.digital_signature(ids[2]).unwrap();
    assert_eq!((d.verdict, d.why), (Verdict::NotVerified, Some(Why::AddressMismatch)));
}

const ALICE: &[u8] = include_bytes!("../../smime/testdata/alice.sign.der");

fn key_of(der: &[u8]) -> mailcraft_model::public_keys::PublicKey {
    mailcraft_model::public_keys::PublicKey {
        format: mailcraft_model::digital_signature::SignatureFormat::Smime,
        data: std::sync::Arc::new(der.to_vec()),
        fingerprint: format!("{:02x?}", &der[der.len() - 8..]),
        not_after: 0,
        seen: 0,
    }
}

/// A certificate put in the place of another kept for the sender (the same count) is seen: the
/// result rests on the keys, not on how many there are (#30 review 9).
#[test]
fn a_kept_certificate_replaced_by_another_is_seen() {
    let (mut s, ids) = session_with(&[NO_CERTS]);
    s.set_trust_anchors([CA]);
    s.mailbox.public_keys = Default::default();
    s.mailbox.public_keys.add("alice@smime.example", key_of(CA));
    assert_eq!(s.digital_signature(ids[0]).unwrap().why, Some(Why::NoPublicKey));
    s.mailbox.public_keys = Default::default();
    s.mailbox.public_keys.add("alice@smime.example", key_of(ALICE));
    assert_eq!(s.mailbox.public_keys.of("alice@smime.example").len(), 1, "the same count");
    assert_eq!(s.digital_signature(ids[0]).unwrap().verdict, Verdict::Verified);
}

/// An undo leaves the certificates the checks kept: they were learnt, not done (#30 review 8).
#[test]
fn an_undo_leaves_the_kept_certificates() {
    let (mut s, ids) = session_with(&[DETACHED]);
    s.set_trust_anchors([CA]);
    assert_eq!(s.digital_signature(ids[0]).unwrap().verdict, Verdict::Verified);
    s.run("mail.flag", &json!({"id": ids[0]})).unwrap();
    assert_eq!(s.mailbox.public_keys.of("alice@smime.example").len(), 1, "kept at that command");
    s.run("edit.undo", &json!({})).unwrap();
    assert_eq!(s.mailbox.messages.get(&ids[0]).unwrap().flag, mailcraft_model::FlagStatus::None, "the flag was undone");
    assert_eq!(s.mailbox.public_keys.of("alice@smime.example").len(), 1, "the certificate stays");
}

/// An opaque signed message is shown as the content that was signed (#30 review 14).
#[test]
fn an_opaque_signed_message_shows_what_was_signed() {
    let (mut s, ids) = session_with(&[OPAQUE, DETACHED]);
    s.set_trust_anchors([CA]);
    assert!(s.mailbox.messages.get(&ids[0]).unwrap().body.text.trim().is_empty(), "nothing but the CMS as its parts are read");
    let view = s.signed_view(ids[0]).unwrap();
    assert!(view.body.text.contains("Hello Bob"), "{:?}", view.body.text);
    assert_eq!(view.id, ids[0]);
    assert!(s.signed_view(ids[1]).is_none(), "a detached signature's message is shown as it is");
}
