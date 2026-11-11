// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypting at Send (#30 part 7): what is encrypted is read back with the secret half of the
//! recipients' and the owner's keys (made here, from a seed: production code never opens a private
//! key), the signed part goes inside when the message was signed first, and what can't be encrypted
//! is held and never sent in the clear. Every name and message is invented.

use std::sync::Arc;

use mailcraft_model::digital_signature::SignatureFormat;
use mailcraft_model::encrypting::{EncryptTo, Encryption};
use mailcraft_model::signing::SigningKey;
use mailcraft_model::sync::{SendOutcome, SendUp};
use mailcraft_model::{Address, Body, Message, Original};
use pgp::composed::{EncryptionCaps, KeyType, Message as PgpMessage, SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey, SubkeyParamsBuilder};
use pgp::crypto::ecc_curve::ECCCurve;
use pgp::ser::Serialize;
use pgp::types::{KeyVersion, Password};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::encrypting::encrypt_with;
use crate::signing::{Refused, held_sends, sign_outbox_with, sign_sends_with};

const NOW: i64 = 4_000_000_000;
/// Inside the test certificates validity (2020 to 2060).
const SMIME_NOW: i64 = 2_000_000_000;
const SMIME_MAYA: &[u8] = include_bytes!("../../../crates/smime/testdata/maya.enc.der");
const SMIME_OWNER: &[u8] = include_bytes!("../../../crates/smime/testdata/owner.enc.der");
const FAKE_SIGNATURE: &str = "-----BEGIN PGP SIGNATURE-----\n\niQEzBAABCAAdFiEE\n=abcd\n-----END PGP SIGNATURE-----\n";

fn secret(user_id: &str, seed: u64) -> SignedSecretKey {
    let mut params = SecretKeyParamsBuilder::default();
    params.version(KeyVersion::V4).key_type(KeyType::Ed25519Legacy).can_certify(true).can_sign(true).primary_user_id(user_id.into());
    let mut sub = SubkeyParamsBuilder::default();
    sub.version(KeyVersion::V4).key_type(KeyType::ECDH(ECCCurve::Curve25519Legacy)).can_sign(false).can_encrypt(EncryptionCaps::All);
    params.subkeys(vec![sub.build().unwrap()]);
    params.build().unwrap().generate(StdRng::seed_from_u64(seed)).unwrap()
}

fn public(secret: &SignedSecretKey) -> Vec<u8> {
    SignedPublicKey::from(secret.clone()).to_bytes().unwrap()
}

fn seeded(seed: u64) -> impl FnMut(&mut [u8]) -> bool {
    let mut rng = StdRng::seed_from_u64(seed);
    move |buf| {
        rng.fill_bytes(buf);
        true
    }
}

fn message(id: u64) -> Message {
    Message {
        id,
        from: Address::new("Ada", "ada@mail.example"),
        to: vec![Address::new("Maya", "maya@lumen.example")],
        bcc: vec![Address::new("", "kit@lumen.example")],
        subject: "Rota".into(),
        date: 1_790_000_000,
        message_id: format!("rota.{id}@mail.example"),
        body: Body { text: "The vault code is 4417.\n".into(), html: None },
        ..Message::default()
    }
}

fn plan(format: SignatureFormat, recipients: Vec<(&str, Vec<u8>, bool)>) -> Encryption {
    Encryption {
        format,
        recipients: recipients
            .into_iter()
            .map(|(address, key, own)| EncryptTo { address: address.into(), fingerprint: "AB".repeat(20), key: Arc::new(key), own })
            .collect(),
        own_address: "ada@mail.example".into(),
    }
}

/// What the owner of `secret` reads out of the armoured message in a PGP/MIME body.
fn read(sent: &Message, secret: &SignedSecretKey) -> Option<Vec<u8>> {
    let body = sent.original_bytes()?;
    let text = String::from_utf8_lossy(body);
    let start = text.find("-----BEGIN PGP MESSAGE-----")?;
    let end = text.find("-----END PGP MESSAGE-----")? + "-----END PGP MESSAGE-----".len();
    let armoured = text.get(start..end)?.replace("\r\n", "\n");
    let (message, _) = PgpMessage::from_string(&armoured).ok()?;
    let mut decrypted = message.decrypt(&Password::empty(), secret).ok()?;
    decrypted.as_data_vec().ok()
}

fn no_own(_: SignatureFormat, _: &str, _: Option<&str>, _: i64) -> Result<Vec<u8>, String> {
    Err("none of your own keys could be found".into())
}

#[test]
fn an_openpgp_message_is_encrypted_to_the_recipients_and_the_owner() {
    let (maya, ada, kit) = (secret("Maya <maya@lumen.example>", 71), secret("Ada <ada@mail.example>", 72), secret("Kit <kit@lumen.example>", 73));
    let mut m = message(1);
    m.encrypt_with =
        Some(plan(SignatureFormat::OpenPgp, vec![("maya@lumen.example", public(&maya), false), ("kit@lumen.example", public(&kit), false)]));
    // The owner's own key isn't in the plan: found by the host, here handed in.
    let own = |format: SignatureFormat, address: &str, _: Option<&str>, _: i64| {
        assert_eq!((format, address), (SignatureFormat::OpenPgp, "ada@mail.example"));
        Ok(public(&ada))
    };
    encrypt_with(&mut m, NOW, &mut seeded(1), &own).unwrap();
    let entity = mailcraft_mime::signable(&message(1));
    for key in [&maya, &ada, &kit] {
        assert_eq!(read(&m, key).as_deref(), Some(entity.as_slice()));
    }
    assert!(!mailcraft_mime::encryption_missing(&m));
    // What goes out: the headers, and nothing of the text in the clear, the Bcc line included.
    let wire = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default());
    let text = String::from_utf8_lossy(&wire);
    assert!(text.contains("multipart/encrypted") && text.contains("Subject: Rota"), "{text}");
    assert!(!text.contains("4417"), "{text}");
}

#[test]
fn the_owners_own_key_in_the_plan_is_not_looked_for_again() {
    let (maya, ada) = (secret("Maya <maya@lumen.example>", 74), secret("Ada <ada@mail.example>", 75));
    let mut m = message(2);
    m.encrypt_with =
        Some(plan(SignatureFormat::OpenPgp, vec![("maya@lumen.example", public(&maya), false), ("ada@mail.example", public(&ada), true)]));
    encrypt_with(&mut m, NOW, &mut seeded(2), &no_own).unwrap();
    assert!(read(&m, &ada).is_some(), "Sent Items stays readable");
}

#[test]
fn without_the_owners_own_key_it_is_held_and_never_sent_in_the_clear() {
    let maya = secret("Maya <maya@lumen.example>", 76);
    let mut m = message(3);
    m.encrypt_with = Some(plan(SignatureFormat::OpenPgp, vec![("maya@lumen.example", public(&maya), false)]));
    let why = encrypt_with(&mut m, NOW, &mut seeded(3), &no_own).unwrap_err();
    assert!(why.contains("none of your own keys"), "{why}");
    assert!(m.original.is_none());
    assert!(mailcraft_mime::encryption_missing(&m), "nothing sends it");
    // As a run says it: taken out, held with why, in the words of encryption.
    let mut outbox = vec![m.clone(), message(4)];
    let unsigned = sign_outbox_with(&mut outbox, &|m| encrypt_with(m, NOW, &mut seeded(3), &no_own).map_err(|why| Refused { why, encrypting: true }));
    assert_eq!(outbox.iter().map(|m| m.id).collect::<Vec<_>>(), [4], "the one not to be encrypted goes as it is");
    assert_eq!((unsigned[0].message, unsigned[0].encrypting), (3, true));
    let mut sends = vec![SendUp { message: 3, draft: None, mime: Some(Box::new(m)), discard: String::new() }];
    let unsigned = sign_sends_with(&mut sends, &|m| encrypt_with(m, NOW, &mut seeded(3), &no_own).map_err(|why| Refused { why, encrypting: true }));
    assert!(sends.is_empty());
    let held = held_sends(unsigned);
    let SendOutcome::Held { why, lost } = &held[0].outcome else { panic!("held") };
    assert!(!lost && why.starts_with("It couldn't be Encrypted: none of your own keys"), "{why}");
}

#[test]
fn signed_first_then_encrypted_with_the_signed_part_inside() {
    let (maya, ada) = (secret("Maya <maya@lumen.example>", 77), secret("Ada <ada@mail.example>", 78));
    let mut m = message(5);
    m.sign_with = SigningKey::new(SignatureFormat::OpenPgp, &"AB".repeat(20), "Ada");
    m.encrypt_with =
        Some(plan(SignatureFormat::OpenPgp, vec![("maya@lumen.example", public(&maya), false), ("ada@mail.example", public(&ada), true)]));
    // What the signing step made (a stand-in signature: the order is what is looked at).
    let entity = mailcraft_mime::signable(&m);
    let signed = mailcraft_mime::multipart_signed(&entity, mailcraft_mime::SignatureKind::OpenPgp, "pgp-sha256", FAKE_SIGNATURE.as_bytes()).unwrap();
    m.original = Original::new(signed);
    encrypt_with(&mut m, NOW, &mut seeded(5), &no_own).unwrap();
    let inside = read(&m, &maya).expect("maya reads it");
    let parts = mailcraft_mime::signed_parts(&inside).expect("what is inside is the signed message");
    assert_eq!(parts.content, entity, "the signed part, byte for byte");
    assert!(parts.whole);
    // The message shows as encrypted, and not as missing its signature.
    assert!(!mailcraft_mime::encryption_missing(&m) && !mailcraft_mime::signature_missing(&m));
    // The outer message has no signature of its own to be mistaken for: it is the encrypted one.
    assert!(mailcraft_mime::is_encrypted(m.original_bytes().unwrap()));
}

#[test]
fn to_be_signed_and_not_signed_is_never_encrypted_alone() {
    let maya = secret("Maya <maya@lumen.example>", 79);
    let mut m = message(6);
    m.sign_with = SigningKey::new(SignatureFormat::OpenPgp, &"AB".repeat(20), "Ada");
    m.encrypt_with =
        Some(plan(SignatureFormat::OpenPgp, vec![("maya@lumen.example", public(&maya), false), ("ada@mail.example", public(&maya), true)]));
    let why = encrypt_with(&mut m, NOW, &mut seeded(6), &no_own).unwrap_err();
    assert!(why.contains("Digitally signed"), "{why}");
    assert!(mailcraft_mime::signature_missing(&m) && mailcraft_mime::encryption_missing(&m));
}

#[test]
fn encrypt_asked_for_and_nothing_decided_is_held() {
    let mut m = message(7);
    m.encrypt = Some(true);
    assert!(encrypt_with(&mut m, NOW, &mut seeded(7), &no_own).is_err());
    assert!(mailcraft_mime::encryption_missing(&m));
    // Not asked for: nothing to do, nothing changed.
    let mut plain = message(8);
    encrypt_with(&mut plain, NOW, &mut seeded(8), &no_own).unwrap();
    assert!(plain.original.is_none());
}

#[test]
fn a_source_of_random_numbers_that_fails_makes_nothing() {
    let maya = secret("Maya <maya@lumen.example>", 80);
    let mut m = message(9);
    m.encrypt_with =
        Some(plan(SignatureFormat::OpenPgp, vec![("maya@lumen.example", public(&maya), false), ("ada@mail.example", public(&maya), true)]));
    let err = encrypt_with(&mut m, NOW, &mut |_: &mut [u8]| false, &no_own).unwrap_err();
    assert!(err.contains("random"), "{err}");
    assert!(m.original.is_none() && mailcraft_mime::encryption_missing(&m));
}

#[test]
fn a_plan_that_cant_be_read_is_never_used() {
    let mut m = message(10);
    m.encrypt_with =
        Some(plan(SignatureFormat::OpenPgp, vec![("maya@lumen.example", b"not a key".to_vec(), false), ("ada@mail.example", b"x".to_vec(), true)]));
    assert!(encrypt_with(&mut m, NOW, &mut seeded(10), &no_own).is_err());
    m.encrypt_with = Some(plan(SignatureFormat::OpenPgp, vec![]));
    assert!(encrypt_with(&mut m, NOW, &mut seeded(10), &no_own).unwrap_err().contains("can't be read"));
    assert!(mailcraft_mime::encryption_missing(&m));
}

#[test]
fn an_smime_message_is_enveloped_data_in_the_form_mail_programs_read() {
    let mut m = message(11);
    m.encrypt_with = Some(plan(SignatureFormat::Smime, vec![("maya@lumen.example", SMIME_MAYA.to_vec(), false)]));
    let own = |format: SignatureFormat, _: &str, _: Option<&str>, _: i64| {
        assert_eq!(format, SignatureFormat::Smime);
        Ok(SMIME_OWNER.to_vec())
    };
    encrypt_with(&mut m, SMIME_NOW, &mut seeded(11), &own).unwrap();
    let body = m.original_bytes().unwrap();
    assert_eq!(mailcraft_mime::protection(body), Some(mailcraft_mime::Protection::Pkcs7));
    assert!(mailcraft_mime::is_encrypted(body) && !mailcraft_mime::encryption_missing(&m));
    let wire = mailcraft_mime::write(&m, &mailcraft_mime::WriteOptions::default());
    let text = String::from_utf8_lossy(&wire);
    assert!(text.contains("smime-type=enveloped-data") && !text.contains("4417"), "{text}");
    // Two recipients: Maya and the owner (each its own RecipientInfo).
    let der = mailcraft_charset::base64::STANDARD.decode(text.split("\r\n\r\n").nth(1).unwrap().replace("\r\n", "")).unwrap();
    assert!(der.windows(9).any(|w| w == [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x03]), "an EnvelopedData");
    // A certificate that isn't for encrypting stops it, and nothing is sent.
    let mut bad = message(12);
    bad.encrypt_with = Some(plan(SignatureFormat::Smime, vec![("maya@lumen.example", b"not a certificate".to_vec(), false)]));
    assert!(encrypt_with(&mut bad, SMIME_NOW, &mut seeded(12), &own).is_err());
    assert!(mailcraft_mime::encryption_missing(&bad));
}

#[test]
fn encrypting_again_makes_it_again_from_the_message_as_it_is() {
    let (maya, ada) = (secret("Maya <maya@lumen.example>", 81), secret("Ada <ada@mail.example>", 82));
    let mut m = message(13);
    m.encrypt_with =
        Some(plan(SignatureFormat::OpenPgp, vec![("maya@lumen.example", public(&maya), false), ("ada@mail.example", public(&ada), true)]));
    // Whatever bytes it held (an earlier send's, a signature by someone else) are let go of.
    m.original = Original::new(b"MIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nold\r\n".to_vec());
    encrypt_with(&mut m, SMIME_NOW, &mut seeded(13), &no_own).unwrap();
    assert_eq!(read(&m, &maya).as_deref(), Some(mailcraft_mime::signable(&message(13)).as_slice()));
}
