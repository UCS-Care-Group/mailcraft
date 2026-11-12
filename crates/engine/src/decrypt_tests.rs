// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypted messages opened where they are read (#30 part 6): what they open to is held in
//! memory only, never in the mailbox file, a search, an undo step or what an agent reads. The host
//! (the key holders) is played by the tests, which hand back the plaintext; the cryptography is
//! tested in `mailcraft-smime` and `mailcraft-openpgp`, and with a real key holder in the app.

use mailcraft_model::digital_signature::{SignatureFormat, Verdict};
use mailcraft_model::{Id, Mailbox};
use serde_json::json;
use zeroize::Zeroizing;

use crate::decrypt::{Decryption, Failure, MAX_KEPT, Plain};
use crate::{Part, Session};

const CA: &[u8] = include_bytes!("../../smime/testdata/ca.rsa.der");
const DETACHED: &[u8] = include_bytes!("../../smime/testdata/detached.eml");

/// 2026-11-09.
const NOW: i64 = 1_794_182_400;
const SECRET: &str = "the vault code is 4417";

/// An S/MIME message whose CMS is EnvelopedData (nothing inside it is read by the engine).
fn smime(from: &str, subject: &str) -> Vec<u8> {
    format!(
        "From: {from}\r\nTo: bob@smime.example\r\nSubject: {subject}\r\nMIME-Version: 1.0\r\nContent-Type: application/pkcs7-mime; smime-type=enveloped-data; name=smime.p7m\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=smime.p7m\r\n\r\nMA8GCSqGSIb3DQEHA6ACMAA=\r\n"
    )
    .into_bytes()
}

/// A PGP/MIME message (the packets inside aren't read by the engine either).
fn pgp_mime(from: &str) -> Vec<u8> {
    format!(
        "From: {from}\r\nTo: bob@smime.example\r\nSubject: Minutes\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=\"=-e1\"\r\n\r\n--=-e1\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n--=-e1\r\nContent-Type: application/octet-stream; name=\"encrypted.asc\"\r\n\r\n-----BEGIN PGP MESSAGE-----\r\n\r\nwcDMA3wvqk35PDeyAQv/\r\n-----END PGP MESSAGE-----\r\n--=-e1--\r\n"
    )
    .into_bytes()
}

fn session_with(messages: &[&[u8]]) -> (Session, Vec<Id>) {
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

fn plain(text: &str) -> Plain {
    Plain {
        data: Zeroizing::new(format!("Content-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n").into_bytes()),
        inline: false,
        signatures: Vec::new(),
        authenticated: true,
    }
}

/// Open message `id` as the host would, with `plain`.
fn open(s: &mut Session, id: Id, plain: Plain) {
    let job = s.begin_decrypt(id).expect("a job");
    assert_eq!(job.id, id);
    s.finish_decrypt(id, Ok(plain));
}

#[test]
fn an_encrypted_message_is_told_by_its_bytes_and_not_by_its_name() {
    let (mut s, ids) = session_with(&[
        &smime("alice@smime.example", "Vault"),
        &pgp_mime("alice@example.org"),
        b"From: a@example.org\r\nSubject: Hi\r\n\r\nHello.\r\n",
    ]);
    assert_eq!(s.encryption_of(ids[0]), Some(SignatureFormat::Smime));
    assert_eq!(s.encryption_of(ids[1]), Some(SignatureFormat::OpenPgp));
    assert_eq!(s.encryption_of(ids[2]), None);
    assert_eq!(s.decryption(ids[2]), Decryption::NotEncrypted);
    assert!(s.begin_decrypt(ids[2]).is_none());
    // A Digitally signed message is not an Encrypted one.
    let (mut s, ids) = session_with(&[DETACHED]);
    assert_eq!(s.encryption_of(ids[0]), None);
}

#[test]
fn what_an_encrypted_message_opens_to_is_in_memory_only() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let id = ids[0];
    assert_eq!(s.decryption(id), Decryption::Waiting(SignatureFormat::Smime));
    let steps = s.history.len();
    let job = s.begin_decrypt(id).unwrap();
    assert_eq!(job.format, SignatureFormat::Smime);
    assert!(job.original.starts_with(b"From: alice@smime.example"));
    assert_eq!(s.decryption(id), Decryption::Opening(SignatureFormat::Smime));
    // Asked of the host once.
    assert!(s.begin_decrypt(id).is_none());
    s.finish_decrypt(id, Ok(plain(SECRET)));
    assert_eq!(s.decryption(id), Decryption::Opened(SignatureFormat::Smime));
    // The window's copy has it, with the stored message's header, and is a view.
    let shown = s.decrypted_message(id).unwrap();
    assert!(shown.body.text.contains(SECRET));
    assert_eq!(shown.subject, "Vault");
    assert!(shown.original.is_none());
    assert!(s.is_decrypted_view(&shown));
    assert!(!s.is_decrypted_view(s.mailbox.message(id).unwrap()));
    // The stored message is as it was: the encrypted original, no text.
    let stored = s.mailbox.message(id).unwrap();
    assert!(!stored.body.text.contains(SECRET));
    assert!(stored.has_original());
    // Nothing of it is saved, searched, undone, or read by a command.
    let saved = String::from_utf8_lossy(&crate::io::write_mailbox(&s.mailbox).unwrap()).into_owned();
    assert!(!saved.contains(SECRET), "the mailbox file holds the plaintext");
    assert_eq!(s.history.len(), steps);
    for command in ["mail.get", "mail.viewSource", "mail.digitalSignature"] {
        let out = s.run(command, &json!({"id": id})).map(|v| v.to_string()).unwrap_or_default();
        assert!(!out.contains(SECRET), "{command} gave the plaintext");
    }
    let found = s.run("mail.list", &json!({"query": "vault code"})).unwrap().to_string();
    assert!(!found.contains("Vault") || !found.contains(SECRET));
    assert!(s.run("mail.list", &json!({"query": "4417"})).unwrap()["items"].as_array().is_none_or(Vec::is_empty), "a search found the plaintext");
    assert!(s.content(id, Part::Text).map(|t| !String::from_utf8_lossy(&t).contains(SECRET)).unwrap_or(true));
    for i in 0..stored_attachments(&s, id) {
        let bytes = s.content(id, Part::Attachment(i)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(SECRET));
    }
}

fn stored_attachments(s: &Session, id: Id) -> usize {
    s.mailbox.message(id).map_or(0, |m| m.attachments.len())
}

#[test]
fn an_agent_is_told_the_message_is_encrypted_and_never_given_what_it_holds() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault"), b"From: a@example.org\r\nSubject: Hi\r\n\r\nHello.\r\n"]);
    let before = s.run("mail.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(before["encrypted"], true);
    assert_eq!(before["encryptedFormat"], "smime");
    assert_eq!(s.run("mail.get", &json!({"id": ids[1]})).unwrap()["encrypted"], false);
    open(&mut s, ids[0], plain(SECRET));
    let after = s.run("mail.get", &json!({"id": ids[0]})).unwrap();
    assert_eq!(after["encrypted"], true);
    assert!(!after.to_string().contains(SECRET));
    assert_eq!(after["body"], before["body"]);
    // An agent can't ask for a message to be opened (it could bring up a PIN prompt).
    assert_eq!(crate::agent::access("mail.decryptAgain"), Some(crate::agent::Access::Never));
}

#[test]
fn a_digital_signature_inside_the_encryption_is_checked_like_any_other() {
    // Signed, then encrypted: what is inside is a Digitally signed message from the sender.
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Signed minutes")]);
    s.set_trust_anchors([CA]);
    let id = ids[0];
    assert!(s.digital_signature(id).is_none());
    let inside = Plain { data: Zeroizing::new(DETACHED.to_vec()), inline: false, signatures: Vec::new(), authenticated: true };
    open(&mut s, id, inside);
    let d = s.digital_signature(id).expect("the signature inside");
    assert_eq!(d.verdict, Verdict::Verified, "{d:?}");
    assert_eq!(s.digital_signature_known(id).map(|d| d.verdict), Some(Verdict::Verified));
    let agent = s.run("mail.digitalSignature", &json!({"id": id})).unwrap();
    assert_eq!(agent["digitalSignature"]["result"], "verified");
    // The signature file isn't one of the files shown; the text is.
    let shown = s.decrypted_message(id).unwrap();
    assert!(shown.body.text.contains("Hello Bob"));
    assert!(shown.attachments.iter().all(|a| !a.mime.contains("pkcs7-signature")));
    // From another sender than the certificate's: not verified.
    let (mut s, ids) = session_with(&[&smime("mallory@example.org", "Signed minutes")]);
    s.set_trust_anchors([CA]);
    open(&mut s, ids[0], Plain { data: Zeroizing::new(DETACHED.to_vec()), inline: false, signatures: Vec::new(), authenticated: true });
    assert_eq!(s.digital_signature(ids[0]).unwrap().verdict, Verdict::NotVerified);
}

#[test]
fn signature_packets_that_came_inside_a_pgp_message_are_checked_and_junk_is_unchecked() {
    let (mut s, ids) = session_with(&[&pgp_mime("alice@example.org")]);
    let id = ids[0];
    let mut p = plain(SECRET);
    p.signatures = vec![0xC2, 0x03, 4, 0, 1];
    open(&mut s, id, p);
    let d = s.digital_signature(id).unwrap();
    assert_eq!(d.verdict, Verdict::Unchecked);
    assert!(s.decrypted_message(id).unwrap().body.text.contains(SECRET));
}

#[test]
fn a_failure_is_kept_until_it_is_asked_again_and_a_pin_retries_what_waited_for_it() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault"), &smime("alice@smime.example", "Vault 2")]);
    let (a, b) = (ids[0], ids[1]);
    s.begin_decrypt(a).unwrap();
    s.finish_decrypt(a, Err(Failure::NoKey));
    assert_eq!(s.decryption(a), Decryption::Failed(SignatureFormat::Smime, Failure::NoKey));
    assert!(s.begin_decrypt(a).is_none(), "not asked of the host again on every frame");
    s.run("mail.decryptAgain", &json!({"id": a})).unwrap();
    assert_eq!(s.decryption(a), Decryption::Waiting(SignatureFormat::Smime));
    // A smartcard that needs its PIN: asked again once the PIN is given.
    s.begin_decrypt(b).unwrap();
    s.finish_decrypt(b, Err(Failure::PinNeeded("Card".into())));
    s.begin_decrypt(a).unwrap();
    s.finish_decrypt(a, Err(Failure::Cancelled));
    s.retry_decryptions_waiting_for_pin();
    assert_eq!(s.decryption(b), Decryption::Waiting(SignatureFormat::Smime));
    assert_eq!(s.decryption(a), Decryption::Failed(SignatureFormat::Smime, Failure::Cancelled));
    // Not an Encrypted message: refused.
    let (mut s, ids) = session_with(&[b"From: a@example.org\r\nSubject: Hi\r\n\r\nHello.\r\n"]);
    assert!(s.run("mail.decryptAgain", &json!({"id": ids[0]})).is_err());
}

#[test]
fn a_result_for_bytes_the_message_no_longer_has_is_let_go_of() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let id = ids[0];
    s.begin_decrypt(id).unwrap();
    // The bytes it came as went (a part was removed): what the host opened is no longer it.
    s.mailbox.message_mut(id).unwrap().original = None;
    s.finish_decrypt(id, Ok(plain(SECRET)));
    assert!(s.decrypted_message(id).is_none());
    // A message that is gone too.
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    s.begin_decrypt(ids[0]).unwrap();
    s.mailbox.messages.remove(&ids[0]);
    s.finish_decrypt(ids[0], Ok(plain(SECRET)));
    assert!(s.decrypted_message(ids[0]).is_none());
    // A result nobody asked for is let go of.
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    s.finish_decrypt(ids[0], Ok(plain(SECRET)));
    assert!(s.decrypted_message(ids[0]).is_none());
}

#[test]
fn only_a_few_opened_messages_are_kept_and_the_oldest_goes_first() {
    let messages: Vec<Vec<u8>> = (0..=MAX_KEPT).map(|i| smime("alice@smime.example", &format!("Vault {i}"))).collect();
    let refs: Vec<&[u8]> = messages.iter().map(Vec::as_slice).collect();
    let (mut s, ids) = session_with(&refs);
    for id in &ids {
        open(&mut s, *id, plain(SECRET));
    }
    assert!(s.decrypted_message(ids[0]).is_none(), "the oldest was let go of");
    assert_eq!(s.decryption(ids[0]), Decryption::Waiting(SignatureFormat::Smime));
    assert!(s.decrypted_message(*ids.last().unwrap()).is_some());
    assert_eq!(ids.iter().filter(|id| s.decrypted_message(**id).is_some()).count(), MAX_KEPT);
}

#[test]
fn a_mailbox_taken_in_forgets_what_was_opened() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    open(&mut s, ids[0], plain(SECRET));
    assert!(s.decrypted_message(ids[0]).is_some());
    s.decryptions.forget_messages();
    assert!(s.decrypted_message(ids[0]).is_none());
}

/// An inline OpenPGP message in the text of a mail: the packets are those of an encrypted message
/// (a key packet for nobody in particular and a data packet of noise): the engine only tells it
/// by its shape, the keys are the host's.
fn inline_pgp(before: &str, after: &str, charset: &str) -> Vec<u8> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut packets = vec![0xC1, 0x0D, 3, 1, 2, 3, 4, 5, 6, 7, 8, 1, 0x00, 0x08, 0xFF];
    packets.extend_from_slice(&[0xD2, 41, 1]);
    packets.extend((0..40u8).map(|i| i.wrapping_mul(7)));
    let mut text = String::new();
    for chunk in packets.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            text.push(if i <= chunk.len() { char::from(TABLE[((n >> (18 - 6 * i)) & 63) as usize]) } else { '=' });
        }
    }
    format!(
        "From: alice@example.org\r\nTo: bob@smime.example\r\nSubject: Hi\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset={charset}\r\n\r\n{before}\r\n-----BEGIN PGP MESSAGE-----\r\n\r\n{text}\r\n-----END PGP MESSAGE-----\r\n{after}\r\n"
    )
    .into_bytes()
}

fn inline_plain(text: &[u8]) -> Plain {
    Plain { data: Zeroizing::new(text.to_vec()), inline: true, signatures: Vec::new(), authenticated: true }
}

#[test]
fn only_the_encrypted_block_of_an_inline_message_is_shown_and_the_text_around_it_is_said_not_to_be() {
    // Somebody pastes a block they were sent into their own mail, with their own words around it.
    let mail = inline_pgp("Please pay invoice 42 to IBAN XX00.", "Regards, Bob", "utf-8");
    let (mut s, ids) = session_with(&[&mail]);
    assert_eq!(s.encryption_of(ids[0]), Some(SignatureFormat::OpenPgp));
    open(&mut s, ids[0], inline_plain(b"Secret words."));
    let shown = s.decrypted_message(ids[0]).unwrap();
    assert!(shown.body.text.contains("Secret words."));
    assert!(!shown.body.text.contains("IBAN"), "the sender's own text is not shown as part of what was encrypted");
    assert!(!shown.body.text.contains("Regards"));
    assert!(s.decrypted_outside(ids[0]).text);
    // Nothing around the block: nothing to say.
    let (mut s, ids) = session_with(&[&inline_pgp("", "", "utf-8")]);
    open(&mut s, ids[0], inline_plain(b"Secret words."));
    assert!(!s.decrypted_outside(ids[0]).any());
}

#[test]
fn an_inline_message_is_read_in_the_charset_its_mail_names() {
    let mail = inline_pgp("", "", "iso-8859-1");
    let (mut s, ids) = session_with(&[&mail]);
    open(&mut s, ids[0], inline_plain(b"caf\xe9 au lait"));
    assert!(s.decrypted_message(ids[0]).unwrap().body.text.contains("café au lait"));
    // The header is read as the header says, never more.
    use crate::decrypt::outer_charset;
    assert_eq!(outer_charset(b"Content-Type: text/plain;\r\n charset=\"ISO-8859-2\"\r\n\r\nx").as_deref(), Some("ISO-8859-2"));
    assert_eq!(outer_charset(b"Subject: charset=x\r\n\r\nContent-Type: text/plain; charset=y").as_deref(), None);
    assert_eq!(outer_charset(b"Content-Type: text/plain; charset=\"u\x00\"\r\n\r\n"), None);
    assert_eq!(outer_charset(b""), None);
}

#[test]
fn the_encrypted_part_is_told_by_its_place_and_never_by_its_name() {
    use crate::decrypt::{outside_files, outside_of};
    let part = |name: &str, mime: &str| mailcraft_model::Attachment { name: name.into(), mime: mime.into(), ..Default::default() };
    let mut m = mailcraft_model::Message::default();
    assert!(!outside_of(&m, false).any());
    // As mutt and KMail write it (`msg.asc`), as some senders do (no name), as most do (`encrypted.asc`).
    for name in ["msg.asc", "", "encrypted.asc", "whatever.bin"] {
        m.attachments = vec![part("", "application/pgp-encrypted"), part(name, "application/octet-stream")];
        assert!(!outside_of(&m, false).any(), "{name:?}");
        assert!(outside_files(&m, false).is_empty());
    }
    // S/MIME's one part.
    m.attachments = vec![part("smime.p7m", "application/pkcs7-mime")];
    assert!(!outside_of(&m, false).any());
    // A file sent in the clear beside it, and a footer of text: said, and the file kept for the list.
    m.attachments =
        vec![part("", "application/pgp-encrypted"), part("msg.asc", "application/octet-stream"), part("price list.pdf", "application/pdf")];
    m.body.text = "Sent through the list".into();
    let outside = outside_of(&m, false);
    assert!(outside.text);
    assert_eq!(outside.files, 1);
    assert_eq!(outside_files(&m, false).first().map(|a| a.name.as_str()), Some("price list.pdf"));
    // A second octet-stream part isn't the encrypted one.
    m.attachments.push(part("extra.bin", "application/octet-stream"));
    assert_eq!(outside_of(&m, false).files, 2);
}

#[test]
fn files_beside_an_encrypted_part_are_listed_first_and_the_view_says_how_many() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let id = ids[0];
    let mut extra = mailcraft_model::Attachment { name: "price list.pdf".into(), mime: "application/pdf".into(), ..Default::default() };
    extra.data = std::sync::Arc::new(b"%PDF".to_vec());
    std::sync::Arc::make_mut(s.mailbox.messages.get_mut(&id).unwrap()).attachments.push(extra);
    open(&mut s, id, plain(SECRET));
    assert_eq!(s.decrypted_outside(id).files, 1);
    let shown = s.decrypted_message(id).unwrap();
    assert_eq!(shown.attachments.first().map(|a| a.name.as_str()), Some("price list.pdf"));
}

#[test]
fn what_the_window_kept_of_every_opened_message_goes_when_another_mailbox_is_opened() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let id = ids[0];
    open(&mut s, id, plain(SECRET));
    let view = s.decrypted_message(id).unwrap();
    assert!(!s.decrypted_view_gone(&view));
    s.decryptions.forget_messages();
    assert!(s.decrypted_view_gone(&view), "the layout of a mailbox that was closed is kept");
    s.layouts_dropped();
    assert!(!s.decrypted_view_gone(&view));
}

#[test]
fn an_opened_message_whose_bytes_were_replaced_is_not_shown_as_opened() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let id = ids[0];
    open(&mut s, id, plain(SECRET));
    assert!(s.decrypted_message(id).is_some());
    let mut again = mailcraft_mime::parse_fetched(&smime("alice@smime.example", "Vault, sent again"));
    again.id = id;
    again.folder = s.mailbox.message(id).unwrap().folder;
    *s.mailbox.messages.get_mut(&id).unwrap() = std::sync::Arc::new(again);
    assert!(s.decrypted_message(id).is_none(), "the old plaintext under the new headers");
    assert_eq!(s.decrypted_authenticated(id), None);
    assert_eq!(s.decryption(id), Decryption::Waiting(SignatureFormat::Smime));
}

#[test]
fn an_inline_block_in_a_part_of_a_multipart_is_read_in_that_parts_charset() {
    use crate::decrypt::outer_charset;
    let top = String::from_utf8(inline_pgp("", "", "utf-8")).unwrap();
    let block = &top[top.find("-----BEGIN PGP MESSAGE-----").unwrap()..];
    let mail = format!(
        "From: alice@example.org\r\nTo: bob@smime.example\r\nSubject: Hi\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"b1\"\r\n\r\n--b1\r\nContent-Type: text/plain; charset=iso-8859-1\r\n\r\n{block}--b1--\r\n"
    );
    assert_eq!(outer_charset(mail.as_bytes()).as_deref(), Some("iso-8859-1"));
    let (mut s, ids) = session_with(&[mail.as_bytes()]);
    if s.encryption_of(ids[0]).is_some() {
        open(&mut s, ids[0], inline_plain(b"caf\xe9 au lait"));
        assert!(s.decrypted_message(ids[0]).unwrap().body.text.contains("café au lait"));
    }
    // A header that isn't the text part's is not believed.
    let other = "From: a@example.org\r\nContent-Type: application/pdf; charset=bogus\r\n\r\n-----BEGIN PGP MESSAGE-----\r\n";
    assert_eq!(outer_charset(other.as_bytes()), None);
}

#[test]
fn what_an_opened_message_shows_of_the_mailbox_stays_as_the_mailbox_has_it() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let id = ids[0];
    open(&mut s, id, plain(SECRET));
    assert_eq!(s.decrypted_message(id).unwrap().flag, mailcraft_model::FlagStatus::None);
    // Flagged, or put in a category, after it was opened: the view says so.
    s.run("mail.flag", &json!({"id": id})).unwrap();
    assert_eq!(s.decrypted_message(id).unwrap().flag, mailcraft_model::FlagStatus::Flagged);
    assert!(s.decrypted_message(id).unwrap().body.text.contains(SECRET));
    assert!(s.is_decrypted_view(&s.decrypted_message(id).unwrap()));
    // The same view while nothing changes (the window doesn't lay it out again).
    let a = s.decrypted_message(id).unwrap();
    let b = s.decrypted_message(id).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
}

#[test]
fn a_failure_is_for_the_bytes_it_was_for() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let id = ids[0];
    s.begin_decrypt(id).unwrap();
    s.finish_decrypt(id, Err(Failure::Failed));
    assert_eq!(s.decryption(id), Decryption::Failed(SignatureFormat::Smime, Failure::Failed));
    // The bytes came again (Check Again, a resync): asked again, not still "damaged".
    let mut again = mailcraft_mime::parse_fetched(&smime("alice@smime.example", "Vault, resent"));
    again.id = id;
    again.folder = s.mailbox.message(id).unwrap().folder;
    *s.mailbox.messages.get_mut(&id).unwrap() = std::sync::Arc::new(again);
    assert_eq!(s.decryption(id), Decryption::Waiting(SignatureFormat::Smime));
    assert!(s.begin_decrypt(id).is_some());
}

#[test]
fn what_the_window_kept_of_an_opened_message_can_be_told_to_go() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let id = ids[0];
    let before = s.decryptions_let_go();
    open(&mut s, id, plain(SECRET));
    let view = s.decrypted_message(id).unwrap();
    assert!(!s.decrypted_view_gone(&view));
    assert_eq!(s.decryptions_let_go(), before);
    s.forget_decryption(id);
    assert!(s.decryptions_let_go() > before);
    // The view the window kept is now one of nothing: it drops it.
    assert!(s.decrypted_view_gone(&view));
    // The stored message is never taken for one.
    assert!(!s.decrypted_view_gone(s.mailbox.message(id).unwrap()));
}

#[test]
fn an_encryption_that_cant_detect_changes_says_so_to_the_window() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault"), &smime("alice@smime.example", "Vault 2")]);
    let mut cbc = plain(SECRET);
    cbc.authenticated = false;
    open(&mut s, ids[0], cbc);
    open(&mut s, ids[1], plain(SECRET));
    assert_eq!(s.decrypted_authenticated(ids[0]), Some(false));
    assert_eq!(s.decrypted_authenticated(ids[1]), Some(true));
    assert_eq!(s.decrypted_authenticated(999), None);
}

#[test]
fn a_messages_with_hostile_plaintext_is_read_with_the_usual_caps_and_never_panics() {
    let (mut s, ids) = session_with(&[&smime("alice@smime.example", "Vault")]);
    let nasty =
        "Content-Type: multipart/mixed; boundary=x\r\n\r\n".to_string() + &"--x\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n".repeat(2_000);
    open(&mut s, ids[0], Plain { data: Zeroizing::new(nasty.into_bytes()), inline: false, signatures: vec![0xFF; 10], authenticated: true });
    let junk = Plain { data: Zeroizing::new(vec![0xFF; 5_000]), inline: false, signatures: Vec::new(), authenticated: true };
    let (mut s2, ids2) = session_with(&[&pgp_mime("alice@example.org")]);
    open(&mut s2, ids2[0], junk);
    assert!(s2.decrypted_message(ids2[0]).is_some());
}
