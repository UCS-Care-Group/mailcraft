// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Encrypted message as it is sent (#30 part 7): S/MIME's enveloped data and PGP/MIME's
//! `multipart/encrypted` read back as what they are, the entity that goes inside (the signed body
//! when the message was signed first), and a message to be Encrypted that isn't never counting as
//! one. Every name is invented.

use std::sync::Arc;

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::encrypting::{EncryptTo, Encryption};
use mailcraft_model::{Address, Body, Message, Original};
use proptest::prelude::*;

use crate::{
    LayoutError, Protection, SignatureKind, WriteOptions, encryption_missing, entity_to_encrypt, is_encrypted, multipart_encrypted, multipart_signed,
    protection, signable, signature_missing, smime_enveloped, write,
};

const ARMOURED: &str = "-----BEGIN PGP MESSAGE-----\n\nwcBMA0hQ\n=abcd\n-----END PGP MESSAGE-----\n";
const DER: &[u8] = &[0x30, 0x82, 0x01, 0x00, 0xAA, 0xBB, 0xCC];

fn message() -> Message {
    Message {
        from: Address::new("Ada", "ada@example.org"),
        to: vec![Address::new("Bob", "bob@example.org")],
        subject: "Plans".into(),
        message_id: "m1@example.org".into(),
        body: Body { text: "Hello Bob,\n\nSee you at ten.\n".into(), html: None },
        ..Message::default()
    }
}

fn plan(format: SignatureFormat) -> Encryption {
    Encryption {
        format,
        recipients: vec![EncryptTo { address: "bob@example.org".into(), fingerprint: "AB".repeat(20), key: Arc::new(vec![1, 2, 3]), own: false }],
        own_address: "ada@example.org".into(),
    }
}

#[test]
fn smime_enveloped_data_reads_back_as_an_encrypted_message() {
    let body = smime_enveloped(DER).unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("Content-Type: application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\""), "{text}");
    assert!(text.contains("Content-Transfer-Encoding: base64"));
    assert!(text.contains("filename=\"smime.p7m\""));
    assert!(body.ends_with(b"\r\n"));
    assert_eq!(protection(&body), Some(Protection::Pkcs7));
    assert!(is_encrypted(&body));
    // The bytes of the base64 are the DER.
    let (_, b64) = text.split_once("\r\n\r\n").unwrap();
    assert_eq!(crate::transfer::base64_decode(b64.as_bytes()), DER);
    assert_eq!(smime_enveloped(b""), Err(LayoutError::Empty));
}

#[test]
fn signed_data_is_not_an_encrypted_message() {
    let signed = b"MIME-Version: 1.0\r\nContent-Type: application/pkcs7-mime; smime-type=signed-data; name=\"smime.p7m\"\r\nContent-Transfer-Encoding: base64\r\n\r\nMAMCAQE=\r\n";
    assert_eq!(protection(signed), Some(Protection::Pkcs7));
    assert!(!is_encrypted(signed));
    assert!(!is_encrypted(b"Content-Type: text/plain\r\n\r\nhi\r\n"));
    assert!(!is_encrypted(b""));
}

#[test]
fn pgp_mime_encrypted_reads_back_as_an_encrypted_message() {
    let body = multipart_encrypted(ARMOURED.as_bytes()).unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("multipart/encrypted; protocol=\"application/pgp-encrypted\""), "{text}");
    assert!(text.contains("Version: 1"));
    assert!(text.contains("-----BEGIN PGP MESSAGE-----\r\n\r\nwcBMA0hQ\r\n=abcd\r\n-----END PGP MESSAGE-----"));
    assert_eq!(protection(&body), Some(Protection::Encrypted));
    assert!(is_encrypted(&body));
    // Lines are CRLF throughout.
    assert!(!text.replace("\r\n", "").contains('\n'));
}

#[test]
fn a_pgp_message_that_isnt_armoured_text_is_refused() {
    for bad in [
        "",
        "hello",
        "-----BEGIN PGP SIGNATURE-----\n-----END PGP SIGNATURE-----",
        "-----BEGIN PGP MESSAGE-----\nbad \u{e9}\n-----END PGP MESSAGE-----",
    ] {
        assert_eq!(multipart_encrypted(bad.as_bytes()), Err(LayoutError::NotArmoured), "{bad:?}");
    }
    assert_eq!(multipart_encrypted(&[0xFF, 0xFE]), Err(LayoutError::NotArmoured));
}

#[test]
fn the_boundary_occurs_in_neither_part() {
    let first = multipart_encrypted(ARMOURED.as_bytes()).unwrap();
    let boundary = String::from_utf8_lossy(&first)
        .lines()
        .find_map(|l| l.trim().strip_prefix("boundary=\"").map(|b| b.trim_end_matches('"').to_string()))
        .unwrap();
    // Armour that holds the first boundary: another one is chosen.
    let armour = format!("-----BEGIN PGP MESSAGE-----\n\n--{boundary}\n-----END PGP MESSAGE-----\n");
    let second = multipart_encrypted(armour.as_bytes()).unwrap();
    assert_eq!(protection(&second), Some(Protection::Encrypted));
    let text = String::from_utf8_lossy(&second);
    assert_eq!(text.matches(&format!("--{boundary}")).count(), 1, "only the armour has it");
}

#[test]
fn a_message_to_be_encrypted_that_is_not_never_counts_as_one() {
    let mut m = message();
    assert!(!encryption_missing(&m), "nothing asked");
    m.encrypt_with = Some(plan(SignatureFormat::Smime));
    assert!(encryption_missing(&m), "to be encrypted, and no encrypted body");
    // A clear or signed body isn't one.
    m.original = Original::new(b"MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nhello\r\n".to_vec());
    assert!(encryption_missing(&m));
    let signed = multipart_signed(&signable(&m), SignatureKind::Smime, "sha-256", DER).unwrap();
    m.original = Original::new(signed);
    assert!(encryption_missing(&m));
    // S/MIME's signed data isn't.
    m.original = Original::new(
        b"MIME-Version: 1.0\r\nContent-Type: application/pkcs7-mime; smime-type=signed-data\r\nContent-Transfer-Encoding: base64\r\n\r\nMAMCAQE=\r\n"
            .to_vec(),
    );
    assert!(encryption_missing(&m));
    m.original = Original::new(smime_enveloped(DER).unwrap());
    assert!(!encryption_missing(&m));
    m.original = Original::new(multipart_encrypted(ARMOURED.as_bytes()).unwrap());
    assert!(!encryption_missing(&m));
    // A draft's body is never its original: nothing to send either way.
    m.draft = true;
    assert!(encryption_missing(&m));
}

#[test]
fn encrypt_asked_for_and_nothing_decided_is_never_sent_in_the_clear() {
    let mut m = message();
    m.encrypt = Some(true);
    assert!(encryption_missing(&m));
    m.encrypt = Some(false);
    assert!(!encryption_missing(&m));
    m.encrypt = None;
    assert!(!encryption_missing(&m));
    // (A draft isn't sent.)
    m.encrypt = Some(true);
    m.draft = true;
    assert!(!encryption_missing(&m));
}

#[test]
fn signed_then_encrypted_has_the_signed_body_inside_and_counts_as_signed() {
    let mut m = message();
    m.sign_with = mailcraft_model::signing::SigningKey::new(SignatureFormat::Smime, &"AB".repeat(32), "Ada");
    m.encrypt_with = Some(plan(SignatureFormat::Smime));
    // Signed, not yet encrypted: the signature is there, the encryption isn't.
    let entity = signable(&m);
    m.original = Original::new(multipart_signed(&entity, SignatureKind::Smime, "sha-256", DER).unwrap());
    assert!(!signature_missing(&m));
    assert!(encryption_missing(&m));
    // What goes inside is the signed body, without the outer MIME-Version, and it still is signed.
    let inside = entity_to_encrypt(&m);
    let text = String::from_utf8_lossy(&inside);
    assert!(text.starts_with("Content-Type: multipart/signed; protocol=\"application/pkcs7-signature\""), "{text}");
    assert!(!text.contains("MIME-Version"));
    assert!(!inside.ends_with(b"\r\n\r\n"));
    let parts = crate::signed_parts(&inside).unwrap();
    assert_eq!(parts.content, entity);
    assert!(parts.whole);
    // Encrypted: the encrypted body is what shows, and the message isn't missing either.
    m.original = Original::new(smime_enveloped(DER).unwrap());
    assert!(!signature_missing(&m));
    assert!(!encryption_missing(&m));
    // Asked to be signed and encrypted, and only encrypted (the signature skipped): not good.
    let mut skipped = message();
    skipped.sign_with = m.sign_with.clone();
    skipped.encrypt_with = Some(plan(SignatureFormat::Smime));
    skipped.original = Original::new(smime_enveloped(DER).unwrap());
    assert!(!signature_missing(&skipped), "(the host signs first and stops when it can't: the order is its to keep)");
}

#[test]
fn what_is_encrypted_without_a_signature_is_the_signable_entity() {
    let m = message();
    assert_eq!(entity_to_encrypt(&m), signable(&m));
}

#[test]
fn an_encrypted_message_is_written_with_its_headers_and_the_encrypted_body_only() {
    for (body, expected) in
        [(smime_enveloped(DER).unwrap(), "application/pkcs7-mime"), (multipart_encrypted(ARMOURED.as_bytes()).unwrap(), "multipart/encrypted")]
    {
        let mut m = message();
        m.bcc = vec![Address::new("", "carol@example.org")];
        m.encrypt_with = Some(plan(SignatureFormat::Smime));
        m.original = Original::new(body);
        let sent = write(&m, &WriteOptions::default());
        let text = String::from_utf8_lossy(&sent);
        assert!(text.contains("Subject: Plans"), "{text}");
        assert!(text.contains(expected), "{text}");
        // None of the plain text, in any part.
        assert!(!text.contains("See you at ten"), "{text}");
        assert!(!crate::body_not_encrypted(&m));
        // A Bcc recipient with no message of their own leaves it missing: one for all is never sent.
        assert!(encryption_missing(&m));
        m.bcc.clear();
        assert!(!encryption_missing(&m));
    }
}

proptest! {
    /// Whatever bytes, nothing panics, and an armoured message that wasn't made is refused.
    #[test]
    fn hostile_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = smime_enveloped(&bytes);
        let _ = multipart_encrypted(&bytes);
        let _ = is_encrypted(&bytes);
        let mut m = message();
        m.encrypt_with = Some(plan(SignatureFormat::OpenPgp));
        m.original = Original::new(bytes);
        let _ = entity_to_encrypt(&m);
        let _ = encryption_missing(&m);
        let _ = signature_missing(&m);
    }
}
