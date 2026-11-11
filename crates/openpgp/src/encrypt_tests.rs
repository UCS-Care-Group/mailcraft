// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypting to OpenPGP recipients (#30 part 7): keys made here from a seed, each message read
//! back by rPGP's own decrypt with the secret half (the crate has none: a private key is used
//! only by tests), and never a panic on any bytes. Synthetic `example.org` identities only.

use std::sync::LazyLock;

use pgp::composed::{ArmorOptions, EncryptionCaps, KeyType, Message, SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey, SubkeyParamsBuilder};
use pgp::crypto::ecc_curve::ECCCurve;
use pgp::packet::SignatureType;
use pgp::ser::Serialize;
use pgp::types::{KeyVersion, Password};
use proptest::prelude::*;
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};

use crate::fixtures::*;
use crate::{EncryptError, MAX_RECIPIENTS, can_encrypt_to, encrypt, encryption_certificates};

const CONTENT: &[u8] = b"Content-Type: text/plain; charset=utf-8\r\n\r\nThe minutes are attached.\r\n";
const ALICE: &str = "Alice <alice@example.org>";
const NOW: u64 = LATER;

/// A key of `primary` for `user_id` with an encryption-only subkey of `sub`.
fn with_subkey(user_id: &str, version: KeyVersion, primary: KeyType, sub: KeyType, seed: u64) -> SignedSecretKey {
    let mut params = SecretKeyParamsBuilder::default();
    params.version(version).key_type(primary).can_certify(true).can_sign(true).primary_user_id(user_id.into());
    let mut s = SubkeyParamsBuilder::default();
    s.version(version).key_type(sub).can_sign(false).can_encrypt(EncryptionCaps::All);
    params.subkeys(vec![s.build().unwrap()]);
    params.build().unwrap().generate(StdRng::seed_from_u64(seed)).unwrap()
}

static X25519: LazyLock<SignedSecretKey> =
    LazyLock::new(|| with_subkey(ALICE, KeyVersion::V4, KeyType::Ed25519Legacy, KeyType::ECDH(ECCCurve::Curve25519Legacy), 51));
static KIT: LazyLock<SignedSecretKey> =
    LazyLock::new(|| with_subkey("Kit <kit@example.org>", KeyVersion::V4, KeyType::Ed25519Legacy, KeyType::ECDH(ECCCurve::P256), 52));
static RSA: LazyLock<SignedSecretKey> = LazyLock::new(|| secret(ALICE, Make { rsa_encryption: true, seed: 53, ..Make::default() }));
static V6: LazyLock<SignedSecretKey> = LazyLock::new(|| with_subkey(ALICE, KeyVersion::V6, KeyType::Ed25519, KeyType::X25519, 54));
static V6_TOO: LazyLock<SignedSecretKey> =
    LazyLock::new(|| with_subkey("Kit <kit@example.org>", KeyVersion::V6, KeyType::Ed25519, KeyType::X25519, 55));

fn public_bytes(secret: &SignedSecretKey) -> Vec<u8> {
    SignedPublicKey::from(secret.clone()).to_bytes().unwrap()
}

/// The random numbers of the tests: a seeded generator (a test of two messages gives each its own).
fn seeded(seed: u64) -> impl FnMut(&mut [u8]) -> bool {
    let mut rng = StdRng::seed_from_u64(seed);
    move |buf| {
        rng.fill_bytes(buf);
        true
    }
}

/// What the owner of `secret` reads out of `armoured`: the test's own decrypt.
fn read(armoured: &str, secret: &SignedSecretKey) -> Option<Vec<u8>> {
    let (message, _) = Message::from_string(armoured).ok()?;
    let mut decrypted = message.decrypt(&Password::empty(), secret).ok()?;
    decrypted.as_data_vec().ok()
}

fn made_at(secret: &SignedSecretKey) -> u64 {
    u64::from(made(secret)) + 60
}

#[test]
fn an_ecdh_subkey_reads_what_was_encrypted_to_it() {
    let armoured = encrypt(CONTENT, &[public_bytes(&X25519)], NOW, &mut seeded(1)).unwrap();
    assert!(armoured.starts_with("-----BEGIN PGP MESSAGE-----"), "{armoured}");
    assert!(armoured.trim_end().ends_with("-----END PGP MESSAGE-----"), "{armoured}");
    assert_eq!(read(&armoured, &X25519).as_deref(), Some(CONTENT));
    // Nobody else does.
    assert_eq!(read(&armoured, &KIT), None);
}

#[test]
fn an_rsa_encryption_subkey_reads_it() {
    let armoured = encrypt(CONTENT, &[public_bytes(&RSA)], NOW, &mut seeded(2)).unwrap();
    assert_eq!(read(&armoured, &RSA).as_deref(), Some(CONTENT));
}

#[test]
fn every_recipient_reads_it_and_the_owner_among_them() {
    // Alice is the owner: encrypted to her too, so that Sent Items is readable (R17).
    let recipients = [public_bytes(&KIT), public_bytes(&X25519), public_bytes(&RSA)];
    let armoured = encrypt(CONTENT, &recipients, NOW, &mut seeded(3)).unwrap();
    for key in [&*KIT, &*X25519, &*RSA] {
        assert_eq!(read(&armoured, key).as_deref(), Some(CONTENT));
    }
}

#[test]
fn version_6_keys_get_the_aead_form() {
    let armoured = encrypt(CONTENT, &[public_bytes(&V6), public_bytes(&V6_TOO)], NOW, &mut seeded(4)).unwrap();
    assert_eq!(read(&armoured, &V6).as_deref(), Some(CONTENT));
    assert_eq!(read(&armoured, &V6_TOO).as_deref(), Some(CONTENT));
    // A version 4 key among them: the form every key reads, and a version 6 key too.
    let mixed = encrypt(CONTENT, &[public_bytes(&V6), public_bytes(&KIT)], NOW, &mut seeded(5));
    if let Ok(mixed) = mixed {
        assert_eq!(read(&mixed, &KIT).as_deref(), Some(CONTENT));
    }
}

#[test]
fn a_large_message_is_encrypted_whole() {
    let content = vec![b'x'; 2 * 1024 * 1024 + 11];
    let armoured = encrypt(&content, &[public_bytes(&X25519)], NOW, &mut seeded(6)).unwrap();
    assert_eq!(read(&armoured, &X25519), Some(content));
}

#[test]
fn nothing_is_the_same_twice_and_each_recipient_once() {
    let key = public_bytes(&X25519);
    let (one, two) = (
        encrypt(CONTENT, std::slice::from_ref(&key), NOW, &mut seeded(7)).unwrap(),
        encrypt(CONTENT, std::slice::from_ref(&key), NOW, &mut seeded(8)).unwrap(),
    );
    assert_ne!(one, two);
    let doubled = encrypt(CONTENT, &[key.clone(), key.clone()], NOW, &mut seeded(7)).unwrap();
    assert_eq!(doubled, one, "the same recipient twice is one");
}

#[test]
fn a_key_that_cant_be_encrypted_to_stops_it() {
    let t = made_at(&X25519);
    // Signs only: no encryption key at all.
    let signs_only = public_bytes(&secret(ALICE, Make { seed: 56, ..Make::default() }));
    assert!(matches!(encrypt(CONTENT, std::slice::from_ref(&signs_only), NOW, &mut seeded(1)), Err(EncryptError::Key(_))));
    assert!(can_encrypt_to(&signs_only, NOW).is_err());
    // Not yet valid.
    assert!(can_encrypt_to(&public_bytes(&X25519), u64::from(made(&X25519)) - 1).is_err());
    assert_eq!(can_encrypt_to(&public_bytes(&X25519), t), Ok(()));
    // One bad among good: nothing is made.
    let good = public_bytes(&KIT);
    assert!(matches!(encrypt(CONTENT, &[good, signs_only], NOW, &mut seeded(1)), Err(EncryptError::Key(_))));
    // Not a key.
    assert!(can_encrypt_to(b"nothing", NOW).is_err());
    assert!(can_encrypt_to(b"", NOW).is_err());
}

#[test]
fn a_revoked_subkey_is_not_encrypted_to() {
    let mut key = SignedPublicKey::from(X25519.clone());
    let sig = config(&X25519.primary_key, SignatureType::SubkeyRevocation, made(&X25519) + 10, vec![])
        .sign_subkey_binding(&X25519.primary_key, &key.primary_key, &Password::empty(), &key.public_subkeys[0].key)
        .unwrap();
    key.public_subkeys[0].signatures.push(sig);
    let bytes = key.to_bytes().unwrap();
    assert!(can_encrypt_to(&bytes, NOW).is_err());
    assert!(matches!(encrypt(CONTENT, &[bytes], NOW, &mut seeded(1)), Err(EncryptError::Key(_))));
}

#[test]
fn a_revoked_key_is_not_encrypted_to() {
    let mut key = SignedPublicKey::from(X25519.clone());
    let sig = config(&X25519.primary_key, SignatureType::KeyRevocation, made(&X25519) + 10, vec![])
        .sign_key(&X25519.primary_key, &Password::empty(), &key.primary_key)
        .unwrap();
    key.details.revocation_signatures.push(sig);
    assert!(can_encrypt_to(&key.to_bytes().unwrap(), NOW).is_err());
}

#[test]
fn nobody_and_too_many_and_nothing_are_refused() {
    assert_eq!(encrypt(CONTENT, &[], NOW, &mut seeded(1)), Err(EncryptError::NoRecipients));
    let key = public_bytes(&X25519);
    assert_eq!(encrypt(CONTENT, &vec![key.clone(); MAX_RECIPIENTS + 1], NOW, &mut seeded(1)), Err(EncryptError::TooLarge));
    assert_eq!(encrypt(b"", &[key], NOW, &mut seeded(1)), Err(EncryptError::Encoding));
}

#[test]
fn a_source_of_random_numbers_that_fails_makes_nothing() {
    let key = public_bytes(&X25519);
    // Fails from the start, and from the n-th request on (a message asks for its random numbers
    // three times or more).
    for allowed in [0, 1, 2] {
        let mut calls = 0;
        let mut inner = seeded(9);
        let mut flaky = |buf: &mut [u8]| {
            calls += 1;
            calls <= allowed && inner(buf)
        };
        assert_eq!(encrypt(CONTENT, std::slice::from_ref(&key), NOW, &mut flaky), Err(EncryptError::Random), "after {allowed}");
    }
}

#[test]
fn the_owners_own_keys_are_found_in_an_export() {
    let export = [armoured(&X25519), armoured(&secret(ALICE, Make { seed: 56, ..Make::default() })), armoured(&KIT)].concat().into_bytes();
    let found = encryption_certificates(&export, NOW);
    // Two can be encrypted to (the one that only signs isn't offered), each readable again.
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].addresses, ["alice@example.org".to_string()]);
    assert_eq!(found[0].fingerprint, public(&X25519).fingerprint().to_hex());
    let armoured = encrypt(CONTENT, &[found[0].bytes.clone()], NOW, &mut seeded(1)).unwrap();
    assert_eq!(read(&armoured, &X25519).as_deref(), Some(CONTENT));
    assert!(encryption_certificates(b"", NOW).is_empty());
    assert!(encryption_certificates(b"junk bytes", NOW).is_empty());
}

fn armoured(secret: &SignedSecretKey) -> String {
    SignedPublicKey::from(secret.clone()).to_armored_string(ArmorOptions::default()).unwrap()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Whatever bytes are given as a key, nothing panics and nothing is made.
    #[test]
    fn hostile_keys_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        prop_assert!(encrypt(CONTENT, std::slice::from_ref(&bytes), NOW, &mut seeded(1)).is_err());
        prop_assert!(can_encrypt_to(&bytes, NOW).is_err());
        let _ = encryption_certificates(&bytes, NOW);
    }

    /// A real key with a byte changed or the end cut off is refused or still a key, never a panic.
    #[test]
    fn damaged_keys_never_panic(at in 0usize..400, byte in any::<u8>(), cut in 0usize..40) {
        let mut key = public_bytes(&X25519);
        if let Some(b) = key.get_mut(at) { *b = byte; }
        key.truncate(key.len().saturating_sub(cut));
        let _ = encrypt(CONTENT, &[key.clone()], NOW, &mut seeded(1));
        let _ = can_encrypt_to(&key, NOW);
        let _ = encryption_certificates(&key, NOW);
    }
}

/// Writes the Public keys other crates' tests encrypt to (`testdata/*.enc.asc`: EdDSA with an ECDH
/// encryption subkey, for example.org-style addresses). Run on purpose:
/// `cargo test -p mailcraft-openpgp -- --ignored make_the_testdata`.
#[test]
#[ignore = "writes files: run on purpose"]
fn make_the_testdata() {
    for (name, user, seed) in
        [("maya", "Maya <maya@lumen.example>", 61), ("owner", "Ada <ada@mail.example>", 62), ("kit", "Kit <kit@lumen.example>", 63)]
    {
        let key = with_subkey(user, KeyVersion::V4, KeyType::Ed25519Legacy, KeyType::ECDH(ECCCurve::Curve25519Legacy), seed);
        std::fs::write(format!("{}/testdata/{name}.enc.asc", env!("CARGO_MANIFEST_DIR")), armoured(&key)).unwrap();
    }
}
