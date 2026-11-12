// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital signatures checked where a message is read (#30): against the trust anchors the host
//! hands in, kept for the session, given to an agent read only, and the signer's certificate
//! kept for the sender's address. RFC 9216's example certificates, signed by OpenSSL.

use mailcraft_model::digital_signature::{Verdict, Why};
use mailcraft_model::{Mailbox, Message};
use serde_json::json;

use crate::Session;

const CA: &[u8] = include_bytes!("../../smime/testdata/ca.rsa.der");
const DETACHED: &[u8] = include_bytes!("../../smime/testdata/detached.eml");
const OPAQUE: &[u8] = include_bytes!("../../smime/testdata/opaque.eml");
const NO_CERTS: &[u8] = include_bytes!("../../smime/testdata/nocerts.eml");
const OPAQUE_FILES: &[u8] = include_bytes!("../../smime/testdata/opaque-files.eml");

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
    // No anchors yet: valid, but the signer isn't verified, and the reason is that they haven't
    // been read yet (#30 review 2: 6); then, none could be read; never an issuer this computer
    // doesn't trust.
    assert_eq!(s.trust_anchors_read(), None);
    let d = s.digital_signature(ids[0]).unwrap();
    assert_eq!((d.verdict, d.why), (Verdict::NotVerified, Some(Why::AuthoritiesNotRead)));
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
        verified: false,
        source: Default::default(),
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

/// An opaque signed message is read as what it holds, its files too: the reading pane, a reply,
/// an agent and the list all have the signed text, and the `smime.p7m` it came as is told apart
/// as its Digital signature (#30 review 2: 1, 7).
#[test]
fn an_opaque_signed_message_is_read_as_what_it_holds() {
    let (mut s, ids) = session_with(&[OPAQUE_FILES]);
    s.set_trust_anchors([CA]);
    let id = ids[0];
    let m = s.mailbox.messages.get(&id).unwrap().clone();
    assert!(m.body.text.contains("The signed minutes are attached."), "{:?}", m.body.text);
    let names: Vec<&str> = m.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["smime.p7m", "minutes.pdf"]);
    assert!(m.attachments[1].data.starts_with(b"%PDF-1.4"));
    assert_eq!(s.digital_signature(id).unwrap().verdict, Verdict::Verified);
    assert_eq!(s.digital_signature_part(id), Some(0));
    let quoted = s.loaded_for_reply(id).unwrap();
    assert!(quoted.body.text.contains("The signed minutes are attached."));
    let agent = s.run("mail.get", &json!({"id": id})).unwrap();
    assert!(agent["body"].as_str().unwrap().contains("The signed minutes are attached."));
    // The signature file is the result's, not among the files, as in the window.
    let files: Vec<&str> = agent["attachments"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(files, ["minutes.pdf"]);
    assert_eq!(agent["digitalSignature"]["signaturePart"], 0);
    // The file inside opens as any other.
    assert!(s.content(id, crate::Part::Attachment(1)).unwrap().starts_with(b"%PDF-1.4"));
}

/// Only the very part that was checked is the signature: a file of the same type or name sent
/// along (a signed message forwarded as a file, a sender's own `smime.p7s`) is a file (#30
/// review 2: 2).
#[test]
fn only_the_part_that_was_checked_is_the_signature() {
    let p7m = String::from_utf8_lossy(OPAQUE).into_owned();
    let (_, p7m_body) = p7m.split_once("\r\n\r\n").unwrap();
    let detached = String::from_utf8_lossy(DETACHED).into_owned();
    let (head, body) = detached.split_once("\r\n\r\n").unwrap();
    let boundary = head.split("boundary=\"").nth(1).unwrap().split('"').next().unwrap();
    let close = format!("--{boundary}--");
    // Alice's detached signature as the signed part's own file, then two more files: the
    // forwarded opaque message, and a file the sender named `smime.p7s` (outside the signed part,
    // as a list's footer would be: the wrapper is what carries them).
    let wrapped = format!(
        "From: alice@smime.example\r\nTo: bob@smime.example\r\nSubject: Fwd\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\n{head}\r\n\r\n{body}\r\n--outer\r\nContent-Type: application/pkcs7-mime; smime-type=signed-data; name=\"smime.p7m\"\r\nContent-Disposition: attachment; filename=\"smime.p7m\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{p7m_body}\r\n--outer\r\nContent-Type: application/octet-stream; name=\"smime.p7s\"\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\nmy own notes\r\n--outer--\r\n"
    );
    assert!(wrapped.contains(&close));
    let (mut s, ids) = session_with(&[wrapped.as_bytes(), DETACHED]);
    s.set_trust_anchors([CA]);
    let m = s.mailbox.messages.get(&ids[0]).unwrap().clone();
    let names: Vec<&str> = m.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["smime.p7s", "smime.p7m", "smime.p7s"], "the forwarded file isn't opened");
    assert!(s.digital_signature(ids[0]).is_some());
    assert_eq!(s.digital_signature_part(ids[0]), Some(0));
    assert_eq!(s.digital_signature_part(ids[1]), s.mailbox.messages.get(&ids[1]).unwrap().attachments.iter().position(|a| a.name == "smime.p7s"));
}

/// The list's word on a message's seal holds only while what it rests on does: a new day, or
/// another key kept for the sender, and it is asked again (#30 review 2: 7).
#[test]
fn a_result_known_for_the_list_goes_stale_with_its_basis() {
    let (mut s, ids) = session_with(&[DETACHED]);
    s.set_trust_anchors([CA]);
    assert_eq!(s.digital_signature(ids[0]).unwrap().verdict, Verdict::Verified);
    assert!(s.digital_signature_checked(ids[0]));
    assert!(s.digital_signature_known(ids[0]).is_some());
    s.clock.now = Some(NOW + 86_400);
    assert!(!s.digital_signature_checked(ids[0]), "a new day");
    assert!(s.digital_signature_known(ids[0]).is_none());
    s.digital_signature(ids[0]);
    assert!(s.digital_signature_known(ids[0]).is_some());
    s.mailbox.public_keys.add("alice@smime.example", key_of(CA));
    assert!(s.digital_signature_known(ids[0]).is_none(), "another key kept");
}

/// A message signed in a part of a wrapper keeps the wrapper's text, after the signed part's, in
/// the order they came (#30 review 2: 8).
#[test]
fn an_opaque_part_of_a_wrapper_is_read_in_its_place() {
    let opaque = String::from_utf8_lossy(OPAQUE).into_owned();
    let (head, body) = opaque.split_once("\r\n\r\n").unwrap();
    let part_head: String =
        head.lines().filter(|l| l.starts_with("Content-") && !l.starts_with("Content-Disposition")).map(|l| format!("{l}\r\n")).collect();
    let wrapped = format!(
        "From: alice@smime.example\r\nTo: bob@smime.example\r\nSubject: Via the list\r\nContent-Type: multipart/mixed; boundary=outer\r\n\r\n--outer\r\n{part_head}\r\n{body}\r\n--outer\r\nContent-Type: text/plain\r\n\r\nYou are subscribed to minutes@lists.example.\r\n--outer--\r\n"
    );
    let (mut s, ids) = session_with(&[wrapped.as_bytes()]);
    s.set_trust_anchors([CA]);
    let text = s.mailbox.messages.get(&ids[0]).unwrap().body.text.clone();
    let signed = text.find("Hello Bob").unwrap();
    let footer = text.find("You are subscribed").unwrap();
    assert!(signed < footer, "{text:?}");
    let d = s.digital_signature(ids[0]).unwrap();
    assert_eq!((d.verdict, d.why), (Verdict::NotVerified, Some(Why::PartlySigned)));
}

/// The signature part is told by its very bytes: a file of S/MIME's type and the same size, but
/// other bytes, put before it is a file (#30 review 3).
#[test]
fn the_signature_part_is_told_by_its_bytes() {
    let (mut s, ids) = session_with(&[DETACHED]);
    s.set_trust_anchors([CA]);
    let m = s.mailbox.messages.get(&ids[0]).unwrap().clone();
    let real = m.attachments.iter().find(|a| a.name == "smime.p7s").unwrap().clone();
    let mut decoy = real.clone();
    let mut bytes = real.data.as_ref().clone();
    *bytes.last_mut().unwrap() ^= 0xff;
    decoy.data = std::sync::Arc::new(bytes);
    decoy.name = "notes.p7s".into();
    let mut two = Message::clone(&m);
    two.attachments = vec![decoy, real.clone()];
    let inbox = s.mailbox.inbox().unwrap();
    two.folder = inbox;
    let id = s.mailbox.add_message(two).unwrap();
    let m = s.mailbox.messages.get(&id).unwrap().clone();
    assert_eq!(s.signature_part(&m, &real.data), Some(1));
}

/// A row's result is asked for at every frame: the keys it rests on are hashed once for each
/// change of them, not at each asking (#30 review 3).
#[test]
fn the_keys_are_hashed_once_for_each_change() {
    use std::sync::atomic::Ordering;
    let (mut s, ids) = session_with(&[DETACHED]);
    s.set_trust_anchors([CA]);
    s.digital_signature(ids[0]).unwrap();
    let before = s.signatures.hashed.load(Ordering::Relaxed);
    for _ in 0..100 {
        assert!(s.digital_signature_known(ids[0]).is_some());
    }
    assert!(s.signatures.hashed.load(Ordering::Relaxed) <= before + 1, "{}", s.signatures.hashed.load(Ordering::Relaxed) - before);
    s.mailbox.public_keys.add("alice@smime.example", key_of(CA));
    assert!(s.digital_signature_known(ids[0]).is_none(), "another key kept is seen");
}

/// A session on an opaque signed message as a build before #30's read it: the file it came as,
/// and nothing else.
fn old_opaque() -> (Session, mailcraft_model::Id) {
    let (s, ids) = old_opaques(1);
    (s, ids[0])
}

/// The message an older build kept of an opaque signed message.
fn read_bare() -> Message {
    let mut m = mailcraft_mime::parse_fetched(OPAQUE_FILES);
    m.body = Default::default();
    m.attachments.truncate(1);
    m
}

/// [`old_opaque`] with `n` of them.
fn old_opaques(n: usize) -> (Session, Vec<mailcraft_model::Id>) {
    let mut mb = Mailbox::with_local_account("Bob", "bob@smime.example");
    let inbox = mb.inbox().unwrap();
    let ids = (0..n).map(|_| mb.add_message(Message { folder: inbox, ..read_bare() }).unwrap()).collect();
    let mut s = Session::new(mb);
    s.clock.now = Some(NOW);
    s.set_trust_anchors([CA]);
    (s, ids)
}

fn is_bare(s: &Session, id: mailcraft_model::Id) -> bool {
    s.mailbox.message(id).unwrap().body.text.trim().is_empty()
}

/// Before a command, no more of the bytes they came as are read again than a budget allows,
/// though always one message's: the rest come before the next (#30 review 5).
#[test]
fn old_opaque_messages_are_read_again_within_a_budget() {
    let (mut s, ids) = old_opaques(3);
    s.signatures.read_again_bytes = Some(1);
    s.run("mail.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(ids.iter().filter(|id| !is_bare(&s, **id)).count(), 1);
    s.run("mail.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(ids.iter().filter(|id| !is_bare(&s, **id)).count(), 2);
    s.run("mail.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(ids.iter().filter(|id| !is_bare(&s, **id)).count(), 3);
}

/// An undo that puts back a message as it was before it was read again has it read again at the
/// next command (#30 review 5): through `Session::undo` (the host's) and `edit.undo`.
#[test]
fn an_undo_that_puts_back_an_old_opaque_message_has_it_read_again() {
    let (mut s, ids) = old_opaques(2);
    s.signatures.read_again_bytes = Some(1);
    // The first command reads the last noted and flags the other, still bare; the second
    // reads that one before it runs.
    let bare = ids[0];
    s.run("mail.flag", &json!({"ids": [bare]})).unwrap();
    s.run("mail.flag", &json!({"ids": [ids[1]]})).unwrap();
    assert!(!is_bare(&s, bare));
    s.run("edit.undo", &json!({})).unwrap();
    assert!(!is_bare(&s, bare));
    // The host's own undo, outside any command.
    assert!(s.undo());
    assert!(is_bare(&s, bare), "put back as it was before the first command");
    s.run("mail.get", &json!({"id": bare})).unwrap();
    assert!(!is_bare(&s, bare));
}

/// Only the outermost command reads them again first: one run inside another that fails would
/// take what it read away with it (#30 review 7).
#[test]
fn a_command_inside_another_reads_nothing_again() {
    let (mut s, ids) = old_opaques(3);
    s.signatures.read_again_bytes = Some(1);
    let contact = s.run("people.new", &json!({"first": "Noor", "email": "noor@example.org"})).unwrap()["id"].as_u64().unwrap();
    assert_eq!(ids.iter().filter(|id| !is_bare(&s, **id)).count(), 1);
    // A meeting that can't be made: the command it runs inside fails, and so does it.
    assert!(s.run("people.meeting", &json!({"id": contact, "start": "junk"})).is_err());
    assert_eq!(ids.iter().filter(|id| !is_bare(&s, **id)).count(), 2);
    s.run("mail.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(ids.iter().filter(|id| !is_bare(&s, **id)).count(), 3, "none of them lost");
}

/// The messages a command changes that aren't as an older build read them are let go of before
/// the queue is looked at: a command over many messages costs nothing there (#30 review 6).
#[test]
fn messages_a_command_changes_ask_nothing_of_the_queue_unless_bare() {
    let (mut s, _) = old_opaques(1);
    let inbox = s.mailbox.inbox().unwrap();
    let plain = mailcraft_mime::parse(b"From: ada@example.org\r\nSubject: Plain\r\n\r\nHello.\r\n");
    let ids: Vec<_> = (0..50).map(|_| s.mailbox.add_message(Message { folder: inbox, ..plain.clone() }).unwrap()).collect();
    let asked = s.signatures.to_read_again.asked;
    s.run("mail.flag", &json!({"ids": ids})).unwrap();
    assert_eq!(s.signatures.to_read_again.asked, asked);
}

/// One an older build read without the bytes it came as has them come by Check Again through
/// `sync.apply`, and is read again at the next command (#30 review 5).
#[test]
fn an_old_opaque_message_whose_bytes_arrive_is_read_again() {
    use mailcraft_model::FolderKind;
    use mailcraft_model::sync::{FolderDelta, SyncResult};
    let mut s = Session::empty();
    s.clock.now = Some(NOW);
    s.set_trust_anchors([CA]);
    let servers = json!({"email": "bob@smime.example", "incoming": {"host": "imap.mail.example"}, "outgoing": {"host": "smtp.mail.example"}});
    let account = s.run("account.add", &servers).unwrap()["id"].as_u64().unwrap();
    let old = Message { uid: 1, original: None, ..read_bare() };
    let delta = |d: FolderDelta| FolderDelta { remote: "INBOX".into(), kind: FolderKind::Inbox, uid_validity: 7, uid_seen: 1, ..d };
    let new = SyncResult { account, folders: vec![delta(FolderDelta { new: vec![old], ..Default::default() })], ..Default::default() };
    s.run("sync.apply", &json!({"result": new})).unwrap();
    let id = s.mailbox.messages.values().find(|m| m.account == account).unwrap().id;
    assert!(is_bare(&s, id) && !s.mailbox.message(id).unwrap().has_original());
    s.run("mail.checkAgain", &json!({"id": id})).unwrap();
    let read = Message { uid: 1, ..mailcraft_mime::parse_fetched(OPAQUE_FILES) };
    let again = SyncResult { account, folders: vec![delta(FolderDelta { refetched: vec![read], ..Default::default() })], ..Default::default() };
    s.run("sync.apply", &json!({"result": again})).unwrap();
    assert!(s.mailbox.message(id).unwrap().has_original() && is_bare(&s, id));
    s.run("mail.get", &json!({"id": id})).unwrap();
    assert!(!is_bare(&s, id));
}

/// Opening the mailbox notes such a message, and the next command reads its text and files from
/// the bytes it came as before it runs, once, as a change of the mailbox (#30 review 3).
#[test]
fn an_opaque_message_read_before_is_read_again_once() {
    let (mut s, id) = old_opaque();
    let rev = s.rev();
    s.run("mail.get", &json!({"id": id})).unwrap();
    let m = s.mailbox.messages.get(&id).unwrap().clone();
    assert!(m.body.text.contains("The signed minutes are attached."), "{:?}", m.body.text);
    let names: Vec<&str> = m.attachments.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["smime.p7m", "minutes.pdf"]);
    assert!(s.dirty);
    assert!(s.rev() > rev, "what is worked out from the mailbox is due again");
    // Another command: nothing read twice.
    s.run("mail.get", &json!({"id": id})).unwrap();
    assert_eq!(s.mailbox.messages.get(&id).unwrap().attachments.len(), 2);
}

/// Checking such a message changes nothing of it: the commands that give the owner or an agent
/// its Digital signature only read (#30 review 4).
#[test]
fn checking_an_old_opaque_message_changes_nothing() {
    let (mut s, id) = old_opaque();
    // Not read again yet (as if the commands before had read others).
    s.signatures.to_read_again.clear();
    let before = s.mailbox.messages.get(&id).unwrap().clone();
    s.run("mail.digitalSignature", &json!({"id": id})).unwrap();
    s.run("mail.get", &json!({"id": id})).unwrap();
    s.agent = Some(Vec::new());
    s.run("mail.get", &json!({"id": id})).unwrap();
    s.run("mail.digitalSignature", &json!({"id": id})).unwrap();
    assert_eq!(s.digital_signature(id).unwrap().verdict, Verdict::Verified);
    assert!(*s.mailbox.messages.get(&id).unwrap() == before);
}
