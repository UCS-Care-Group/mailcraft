// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening OpenPGP Encrypted messages (#30 part 6): keys and messages made here from seeds, a
//! stand-in for gpg-agent that does the one private-key step (the RSA decryption, or the key
//! agreement of ECDH and X25519) with the secret half, and everything else done by the crate.

use std::sync::LazyLock;

use pgp::composed::{
    ArmorOptions, EncryptionCaps, KeyType, MessageBuilder, SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey, SubkeyParamsBuilder,
};
use pgp::crypto::aead::{AeadAlgorithm, ChunkSize};
use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::ser::Serialize;
use pgp::types::{CompressionAlgorithm, KeyVersion, Password, PlainSecretParams};
use proptest::prelude::*;
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::decrypt::read;
use crate::{AgentKey, DecryptError, DecryptionKey, EncryptedMessage, Esk, Plaintext, Recipient, decryption_keys, encrypted_message, session_key};

const ALICE: &str = "Alice <alice@example.org>";
const TEXT: &[u8] = b"Content-Type: text/plain; charset=utf-8\r\n\r\nThe board meets on Friday.\r\n";

/// A certificate for alice@example.org: an Ed25519 primary key that certifies and signs, and one
/// encryption subkey of `sub`.
fn make(sub: KeyType, version: KeyVersion, seed: u64) -> SignedSecretKey {
    let primary = if version == KeyVersion::V6 { KeyType::Ed25519 } else { KeyType::Ed25519Legacy };
    let mut params = SecretKeyParamsBuilder::default();
    params.version(version).key_type(primary).can_certify(true).can_sign(true).primary_user_id(ALICE.into());
    let mut subkey = SubkeyParamsBuilder::default();
    subkey.version(version).key_type(sub).can_sign(false).can_encrypt(EncryptionCaps::All);
    params.subkeys(vec![subkey.build().unwrap()]);
    params.build().unwrap().generate(StdRng::seed_from_u64(seed)).unwrap()
}

static RSA: LazyLock<SignedSecretKey> = LazyLock::new(|| make(KeyType::Rsa(2048), KeyVersion::V4, 31));
static CV25519: LazyLock<SignedSecretKey> =
    LazyLock::new(|| make(KeyType::ECDH(pgp::crypto::ecc_curve::ECCCurve::Curve25519Legacy), KeyVersion::V4, 32));
static X25519: LazyLock<SignedSecretKey> = LazyLock::new(|| make(KeyType::X25519, KeyVersion::V6, 33));
static OTHER: LazyLock<SignedSecretKey> =
    LazyLock::new(|| make(KeyType::ECDH(pgp::crypto::ecc_curve::ECCCurve::Curve25519Legacy), KeyVersion::V4, 34));

fn export(secret: &SignedSecretKey) -> Vec<u8> {
    SignedPublicKey::from(secret.clone()).to_bytes().unwrap()
}

/// How a test message is made.
#[derive(Clone, Copy, Default)]
struct Build {
    v2: bool,
    compress: bool,
    sign: bool,
    anonymous: bool,
}

/// The binary packets of `data` encrypted to `secret`'s subkey.
fn encrypt_binary(secret: &SignedSecretKey, data: &[u8], how: Build) -> Vec<u8> {
    let mut rng = StdRng::seed_from_u64(77);
    let key = secret.secret_subkeys.first().unwrap().public_key();
    let signer = secret.primary_key.clone();
    macro_rules! finish {
        ($builder:expr) => {{
            let mut b = $builder;
            if how.compress {
                b.compression(CompressionAlgorithm::ZLIB);
            }
            if how.sign {
                b.sign(&signer, Password::empty(), HashAlgorithm::Sha256);
            }
            if how.anonymous {
                b.encrypt_to_key_anonymous(&mut rng, &key).unwrap();
            } else {
                b.encrypt_to_key(&mut rng, &key).unwrap();
            }
            b.to_vec(&mut rng).unwrap()
        }};
    }
    if how.v2 {
        finish!(MessageBuilder::from_bytes("", data.to_vec()).seipd_v2(
            &mut StdRng::seed_from_u64(78),
            SymmetricKeyAlgorithm::AES256,
            AeadAlgorithm::Ocb,
            ChunkSize::C4MiB
        ))
    } else {
        finish!(MessageBuilder::from_bytes("", data.to_vec()).seipd_v1(&mut StdRng::seed_from_u64(78), SymmetricKeyAlgorithm::AES128))
    }
}

fn armoured(binary: &[u8]) -> String {
    let mut out = String::from("-----BEGIN PGP MESSAGE-----\r\n\r\n");
    out.push_str(&armour_body(binary));
    out.push_str("-----END PGP MESSAGE-----\r\n");
    out
}

/// The base64 of `binary` in lines of 64, and its CRC-24 line (RFC 9580 §6.1).
fn armour_body(binary: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in binary.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                text.push(char::from(TABLE[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                text.push('=');
            }
        }
    }
    let mut out = String::new();
    for line in text.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).unwrap());
        out.push_str("\r\n");
    }
    out
}

fn pgp_mime(armour: &str) -> Vec<u8> {
    format!(
        "From: Alice <alice@example.org>\r\nTo: bob@example.org\r\nSubject: Minutes\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted;\r\n protocol=\"application/pgp-encrypted\"; boundary=\"=-e1\"\r\n\r\nThis is an OpenPGP/MIME encrypted message.\r\n--=-e1\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n--=-e1\r\nContent-Type: application/octet-stream; name=\"encrypted.asc\"\r\nContent-Disposition: inline; filename=\"encrypted.asc\"\r\n\r\n{armour}\r\n--=-e1--\r\n"
    )
    .into_bytes()
}

/// What gpg-agent would answer for `recipient` with `secret`'s encryption subkey: the value and
/// its `PADDING` status. For RSA the decrypted message (or the PKCS #1 frame, as 2.4 gives it);
/// for ECDH and X25519 the shared secret (`0x40 || u`).
fn agent(secret: &SignedSecretKey, recipient: &Recipient, framed: bool) -> (Vec<u8>, Option<u32>) {
    let sub = &secret.secret_subkeys.first().unwrap().key;
    match &recipient.esk {
        Esk::Rsa { m } => {
            let value = sub
                .unlock(&Password::empty(), |_, plain| {
                    let PlainSecretParams::RSA(key) = plain else { panic!("not RSA") };
                    let mpi = pgp::types::Mpi::from_slice(m);
                    use pgp::crypto::Decryptor;
                    Ok(key.decrypt(&mpi)?.to_vec())
                })
                .unwrap()
                .unwrap();
            if framed {
                // A frame, as the agent leaves it: 02, random non-zero bytes, 00, the message.
                let mut frame = vec![2u8];
                frame.extend((0..40).map(|i| 1 + (i as u8 % 200)));
                frame.push(0);
                frame.extend_from_slice(&value);
                (frame, None)
            } else {
                (value, Some(0))
            }
        }
        Esk::Ecdh { point, .. } => {
            let eph: [u8; 32] = point.get(1..33).unwrap().try_into().unwrap();
            let secret_bytes = sub
                .unlock(&Password::empty(), |_, plain| {
                    let PlainSecretParams::ECDH(pgp::crypto::ecdh::SecretKey::Curve25519Legacy(k)) = plain else { panic!("not Curve25519") };
                    Ok(*k.as_bytes())
                })
                .unwrap()
                .unwrap();
            (shared(secret_bytes, eph), None)
        }
        Esk::X25519 { ephemeral, .. } => {
            let secret_bytes = sub
                .unlock(&Password::empty(), |_, plain| {
                    let PlainSecretParams::X25519(k) = plain else { panic!("not X25519") };
                    Ok(*k.as_bytes())
                })
                .unwrap()
                .unwrap();
            (shared(secret_bytes, *ephemeral), None)
        }
        Esk::Other => panic!("not a recipient here"),
    }
}

fn shared(secret: [u8; 32], eph: [u8; 32]) -> Vec<u8> {
    let secret = x25519_dalek::StaticSecret::from(secret);
    let mut out = vec![0x40];
    out.extend_from_slice(secret.diffie_hellman(&x25519_dalek::PublicKey::from(eph)).as_bytes());
    out
}

/// Opens `message` for `secret` as the program would: its recipients against the owner's keys,
/// the agent asked, the session key made and the data decrypted.
fn open_with(message: &EncryptedMessage, secret: &SignedSecretKey, framed: bool) -> Result<Plaintext, DecryptError> {
    let keys = decryption_keys(&export(secret));
    assert!(!keys.is_empty());
    for recipient in message.recipients() {
        for key in keys.iter().filter(|k| k.is_recipient(recipient)) {
            let (value, padding) = agent(secret, recipient, framed);
            if let Some(session) = session_key(recipient, key, &value, padding) {
                return message.decrypt(&session);
            }
        }
    }
    Err(DecryptError::Failed)
}

fn open(bytes: &[u8], secret: &SignedSecretKey) -> Result<Plaintext, DecryptError> {
    open_with(&encrypted_message(bytes).expect("an encrypted message"), secret, false)
}

// ---- tests ----

#[test]
fn rsa_pgp_mime_is_opened_with_the_message_and_with_the_frame_as_gpg_agent_gives_them() {
    let binary = encrypt_binary(&RSA, TEXT, Build::default());
    let message = pgp_mime(&armoured(&binary));
    let found = encrypted_message(&message).unwrap();
    assert!(!found.inline);
    assert!(matches!(found.recipients().first().map(|r| &r.esk), Some(Esk::Rsa { .. })));
    for framed in [false, true] {
        let plain = open_with(&found, &RSA, framed).unwrap();
        assert_eq!(plain.data.as_slice(), TEXT);
        assert!(plain.signatures.is_empty());
    }
}

#[test]
fn ecdh_on_curve25519_is_opened_from_the_shared_secret_the_agent_gives() {
    let message = pgp_mime(&armoured(&encrypt_binary(&CV25519, TEXT, Build::default())));
    assert_eq!(open(&message, &CV25519).unwrap().data.as_slice(), TEXT);
}

#[test]
fn x25519_with_a_version_2_data_packet_is_opened() {
    let message = pgp_mime(&armoured(&encrypt_binary(&X25519, TEXT, Build { v2: true, ..Build::default() })));
    let found = encrypted_message(&message).unwrap();
    assert!(found.recipients().first().is_some_and(|r| r.v6 && matches!(r.esk, Esk::X25519 { .. })));
    assert_eq!(open_with(&found, &X25519, false).unwrap().data.as_slice(), TEXT);
}

#[test]
fn inline_pgp_in_the_text_is_opened() {
    let body = format!(
        "From: Alice <alice@example.org>\r\nSubject: Hi\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello Bob.\r\n\r\n{}",
        armoured(&encrypt_binary(&CV25519, b"Secret words.", Build::default()))
    );
    let found = encrypted_message(body.as_bytes()).unwrap();
    assert!(found.inline);
    assert_eq!(open_with(&found, &CV25519, false).unwrap().data.as_slice(), b"Secret words.");
}

#[test]
fn compressed_and_signed_data_is_opened_and_its_signature_packets_come_out() {
    let message = pgp_mime(&armoured(&encrypt_binary(&X25519, TEXT, Build { v2: true, compress: true, sign: true, ..Build::default() })));
    let plain = open(&message, &X25519).unwrap();
    assert_eq!(plain.data.as_slice(), TEXT);
    assert!(!plain.signatures.is_empty());
    // The packets are a signature (tag 2) made by the key that signed.
    assert_eq!(plain.signatures.first().map(|b| b & 0x3f), Some(2));
}

#[test]
fn an_anonymous_recipient_is_tried_with_the_owners_keys() {
    let binary = encrypt_binary(&CV25519, TEXT, Build { anonymous: true, ..Build::default() });
    let found = read(binary, false).unwrap();
    let r = found.recipients().first().unwrap();
    assert!(r.anonymous());
    assert_eq!(open_with(&found, &CV25519, false).unwrap().data.as_slice(), TEXT);
}

#[test]
fn a_message_for_another_key_is_not_opened_and_the_failure_is_the_same_as_any() {
    let message = pgp_mime(&armoured(&encrypt_binary(&OTHER, TEXT, Build::default())));
    // Not a recipient: no key of ours is asked.
    let found = encrypted_message(&message).unwrap();
    let keys = decryption_keys(&export(&CV25519));
    assert!(found.recipients().iter().all(|r| keys.iter().all(|k| !k.is_recipient(r))));
    assert_eq!(open_with(&found, &CV25519, false).err(), Some(DecryptError::Failed));
    // A wrong session key (the other key's value given for this recipient): one failure too.
    let right = encrypted_message(&pgp_mime(&armoured(&encrypt_binary(&CV25519, TEXT, Build::default())))).unwrap();
    let recipient = right.recipients().first().unwrap();
    let key = &keys[0];
    let (mut value, padding) = agent(&CV25519, recipient, false);
    value[5] ^= 1;
    assert!(session_key(recipient, key, &value, padding).is_none());
}

#[test]
fn changed_ciphertext_never_opens() {
    for (secret, v2) in [(&CV25519, false), (&X25519, true), (&RSA, false)] {
        let binary = encrypt_binary(secret, TEXT, Build { v2, ..Build::default() });
        let ok = read(binary.clone(), false).unwrap();
        assert_eq!(open_with(&ok, secret, false).unwrap().data.as_slice(), TEXT);
        // A byte changed in the encrypted data (the end of the packets): the integrity check fails.
        for back in [1usize, 5, 20, 33] {
            let mut changed = binary.clone();
            let at = changed.len() - back;
            changed[at] ^= 0x10;
            let found = read(changed, false).unwrap();
            assert_eq!(open_with(&found, secret, false).err(), Some(DecryptError::Failed), "{back}");
        }
    }
}

#[test]
fn a_compression_bomb_is_stopped_at_the_cap() {
    // More than the cap of zeros, compressed to a few hundred kilobytes.
    let zeros = vec![0u8; crate::MAX_PLAINTEXT + 1];
    let binary = encrypt_binary(&X25519, &zeros, Build { v2: true, compress: true, ..Build::default() });
    assert!(binary.len() < 1 << 20, "{}", binary.len());
    let found = read(binary, false).unwrap();
    assert_eq!(open_with(&found, &X25519, false).err(), Some(DecryptError::Failed));
}

#[test]
fn what_is_not_an_encrypted_message_is_none() {
    let signed = b"From: a@example.org\r\nContent-Type: text/plain\r\n\r\nHello.\r\n";
    assert!(encrypted_message(signed).is_none());
    // A PGP/MIME message with the wrong protocol, or with one part, or with junk for data.
    let wrong = "From: a@example.org\r\nContent-Type: multipart/encrypted; protocol=\"application/other\"; boundary=b\r\n\r\n--b\r\n\r\nx\r\n--b\r\n\r\ny\r\n--b--\r\n";
    assert!(encrypted_message(wrong.as_bytes()).is_none());
    let junk = pgp_mime("-----BEGIN PGP MESSAGE-----\r\n\r\nAAAA\r\n-----END PGP MESSAGE-----");
    assert!(encrypted_message(&junk).is_none());
}

#[test]
fn the_owners_keys_are_read_from_an_export_and_matched_to_the_agents_by_their_material() {
    let keys: Vec<DecryptionKey> = decryption_keys(&export(&RSA));
    assert!(keys.iter().any(DecryptionKey::is_rsa));
    let sub = keys.iter().find(|k| k.is_rsa()).unwrap();
    assert_eq!(sub.addresses(), ["alice@example.org".to_string()]);
    let rsa_sub = keys.iter().find(|k| k.is_rsa()).unwrap();
    assert!(!rsa_sub.matches(&AgentKey::Rsa { n: &[1, 2, 3], e: &[1, 0, 1] }));
    // A Curve25519 key matches an agent key with the 0x40 or without.
    let cv = decryption_keys(&export(&CV25519));
    let key = cv.iter().find(|k| k.is_x25519_class()).unwrap();
    assert!(!key.is_rsa());
    assert!(!key.matches(&AgentKey::Ecc { curve: "NIST P-256", q: &[4; 65] }));
    // Armoured and binary exports read alike; junk reads as none.
    let armoured_export = SignedPublicKey::from(CV25519.clone()).to_armored_string(ArmorOptions::default()).unwrap();
    assert_eq!(decryption_keys(armoured_export.as_bytes()).len(), cv.len());
    assert!(decryption_keys(b"junk").is_empty());
    assert!(decryption_keys(&[]).is_empty());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn no_bytes_make_reading_or_opening_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = encrypted_message(&bytes);
        let _ = decryption_keys(&bytes);
        if let Some(found) = read(bytes.clone(), false) {
            let session = crate::decrypt::SessionKey::for_test(bytes.clone());
            let _ = found.decrypt(&session);
        }
        let armoured = format!("-----BEGIN PGP MESSAGE-----\r\n\r\n{}\r\n-----END PGP MESSAGE-----\r\n", String::from_utf8_lossy(&bytes));
        let _ = encrypted_message(&pgp_mime(&armoured));
    }

    #[test]
    fn a_valid_message_with_bytes_changed_never_panics(at in 0usize..400, with in any::<u8>(), cut in 0usize..16) {
        let mut binary = encrypt_binary(&X25519, TEXT, Build { v2: true, ..Build::default() });
        if let Some(b) = binary.get_mut(at) {
            *b = with;
        }
        binary.truncate(binary.len().saturating_sub(cut));
        if let Some(found) = read(binary, false) {
            let _ = open_with_ignoring_missing(&found);
        }
    }

    #[test]
    fn session_keys_from_any_agent_value_never_panic(value in proptest::collection::vec(any::<u8>(), 0..256), padding in proptest::option::of(0u32..3)) {
        let found = read(encrypt_binary(&CV25519, TEXT, Build::default()), false).unwrap();
        let rsa_found = read(encrypt_binary(&RSA, TEXT, Build::default()), false).unwrap();
        let x_found = read(encrypt_binary(&X25519, TEXT, Build { v2: true, ..Build::default() }), false).unwrap();
        for (found, secret) in [(&found, &*CV25519), (&rsa_found, &*RSA), (&x_found, &*X25519)] {
            let keys = decryption_keys(&export(secret));
            for recipient in found.recipients() {
                for key in keys.iter().filter(|k| k.is_recipient(recipient)) {
                    let _ = session_key(recipient, key, &value, padding);
                }
            }
        }
    }
}

/// Opens `found` with the X25519 test key, whatever has become of it.
fn open_with_ignoring_missing(found: &EncryptedMessage) -> Option<Plaintext> {
    let keys = decryption_keys(&export(&X25519));
    for recipient in found.recipients() {
        for key in keys.iter().filter(|k| k.is_recipient(recipient) && matches!(recipient.esk, Esk::X25519 { .. })) {
            let (value, padding) = agent(&X25519, recipient, false);
            if let Some(session) = session_key(recipient, key, &value, padding) {
                return found.decrypt(&session).ok();
            }
        }
    }
    None
}

#[test]
fn the_ecdh_padding_must_be_all_of_it() {
    use crate::decrypt::unpad;
    assert_eq!(unpad(&[1, 2, 3, 4, 5, 3, 3, 3]), Some(&[1u8, 2, 3, 4, 5][..]));
    assert_eq!(unpad(&[1, 2, 3, 4, 5, 6, 7, 1]), Some(&[1u8, 2, 3, 4, 5, 6, 7][..]));
    // Only the last byte right.
    assert_eq!(unpad(&[1, 2, 3, 4, 5, 9, 9, 3]), None);
    assert_eq!(unpad(&[1, 2, 3, 4, 5, 2, 3, 3]), None);
    // No padding, too much, none left of the message.
    assert_eq!(unpad(&[1, 2, 3, 0]), None);
    assert_eq!(unpad(&[1, 2, 3, 9]), None);
    assert_eq!(unpad(&[2, 2]), None);
    assert_eq!(unpad(&[]), None);
}

#[test]
fn the_agents_padding_status_is_zero_or_absent_and_nothing_else() {
    let message = pgp_mime(&armoured(&encrypt_binary(&RSA, TEXT, Build::default())));
    let found = encrypted_message(&message).unwrap();
    let recipient = found.recipients().first().unwrap();
    let key = decryption_keys(&export(&RSA)).into_iter().find(DecryptionKey::is_rsa).unwrap();
    let (value, _) = agent(&RSA, recipient, false);
    // Padding already removed (`PADDING 0`): the message as it is.
    assert!(session_key(recipient, &key, &value, Some(0)).is_some());
    // Any other status isn't understood: nothing is made of the value.
    for status in [1u32, 2, 255] {
        assert!(session_key(recipient, &key, &value, Some(status)).is_none(), "{status}");
    }
    // No status: the value is a frame, and the bare message isn't one.
    assert!(session_key(recipient, &key, &value, None).is_none());
}

/// A packet in the new format (RFC 9580 §4.2.1): its tag, then a one-, two- or five-octet length.
fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0xC0 | tag];
    match body.len() {
        n if n < 192 => out.push(n as u8),
        n if n < 8384 => out.extend_from_slice(&[(((n - 192) >> 8) + 192) as u8, ((n - 192) & 0xFF) as u8]),
        n => {
            out.push(255);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
    }
    out.extend_from_slice(body);
    out
}

/// The leading public-key encrypted session key packets (tag 1) of `binary`, as bytes.
fn pkesk_packets(binary: &[u8]) -> Vec<u8> {
    let mut at = 0;
    while binary.get(at).is_some_and(|b| b & 0xC0 == 0xC0 && b & 0x3F == 1) {
        let (len, head) = match binary[at + 1] {
            n if n < 192 => (n as usize, 2),
            n if n < 224 => ((((n as usize) - 192) << 8) + binary[at + 2] as usize + 192, 3),
            255 => (u32::from_be_bytes(binary[at + 2..at + 6].try_into().unwrap()) as usize, 6),
            _ => panic!("partial length"),
        };
        at += head + len;
    }
    binary[..at].to_vec()
}

#[test]
fn old_symmetrically_encrypted_data_without_a_check_is_refused() {
    // The old data packet (tag 9) has no integrity check at all; a session key for it must never open it.
    let real = encrypt_binary(&CV25519, TEXT, Build::default());
    let mut old = pkesk_packets(&real);
    assert!(!old.is_empty());
    old.extend(packet(9, &[0x55; 64]));
    if let Some(found) = read(old, false) {
        assert_eq!(open_with(&found, &CV25519, false).err(), Some(DecryptError::Failed));
    }
}

#[test]
fn a_message_with_a_password_packet_and_no_key_packet_is_not_opened_with_a_key() {
    let real = encrypt_binary(&CV25519, TEXT, Build::default());
    let data = &real[pkesk_packets(&real).len()..];
    // A version 4 SKESK with AES-128 and a simple S2K (SHA-256), then the data packet.
    let mut only = packet(3, &[4, 7, 0, 8]);
    only.extend_from_slice(data);
    if let Some(found) = read(only, false) {
        assert!(found.recipients().is_empty());
        assert_eq!(open_with(&found, &CV25519, false).err(), Some(DecryptError::Failed));
    }
}

#[test]
fn a_data_packet_whose_check_was_taken_off_or_changed_is_refused() {
    let binary = encrypt_binary(&CV25519, TEXT, Build::default());
    // The last 22 octets of a version 1 data packet are the check (a packet of 20 octets of hash).
    let mut cut = binary.clone();
    cut.truncate(binary.len() - 22);
    if let Some(found) = read(cut, false) {
        assert_eq!(open_with(&found, &CV25519, false).err(), Some(DecryptError::Failed));
    }
    for back in 1..=22usize {
        let mut changed = binary.clone();
        let at = changed.len() - back;
        changed[at] ^= 0x01;
        let found = read(changed, false).unwrap();
        assert_eq!(open_with(&found, &CV25519, false).err(), Some(DecryptError::Failed), "{back}");
    }
}

#[test]
fn a_version_6_key_says_so() {
    // (ECDH on one isn't opened: its KDF names a 32-octet fingerprint, and the program says so.)
    assert!(decryption_keys(&export(&X25519)).iter().all(DecryptionKey::is_v6));
    assert!(decryption_keys(&export(&CV25519)).iter().all(|k| !k.is_v6()));
}

#[test]
fn what_is_read_out_of_an_unencrypted_message_never_panics() {
    // A literal data packet (tag 11): format 'b', an empty file name, a date, then the data.
    let mut body = vec![b'b', 0, 0, 0, 0, 0];
    body.extend_from_slice(b"Hello.");
    let plain = crate::read_opened(&packet(11, &body)).unwrap();
    assert_eq!(plain.data.as_slice(), b"Hello.");
    assert!(crate::read_opened(b"junk").is_none());
    assert!(crate::read_opened(&[]).is_none());
}
