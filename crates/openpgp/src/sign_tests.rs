// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Making Digital signatures with a key holder that plays gpg-agent: keys made here from a seed,
//! the holder signing the digest it is handed with the secret half, and every signature read
//! back by the crate's own [`crate::verify`].

use std::sync::LazyLock;

use pgp::composed::{EncryptionCaps, KeyType, SecretKeyParamsBuilder, SignedPublicKey, SignedSecretKey, SubkeyParamsBuilder};
use pgp::crypto::ecc_curve::ECCCurve;
use pgp::packet::{KeyFlags, SignatureType, SubpacketData};
use pgp::ser::Serialize;
use pgp::types::{Duration, KeyDetails, KeyVersion, Password, SignatureBytes, SigningKey as _};
use proptest::prelude::*;
use rand::SeedableRng;
use rand::rngs::StdRng;

use crate::fixtures::*;
use crate::{AgentKey, Fingerprint, HashAlgorithm, KeyRing, Outcome, SignError, SignatureValue, SigningKey, sign_detached, signing_keys};

const ALICE: &str = "Alice <alice@example.org>";

/// RSA keys are slow to make: one, for every test that wants one.
static RSA: LazyLock<SignedSecretKey> = LazyLock::new(|| secret(ALICE, Make { rsa: Some(2048), seed: 21, ..Make::default() }));
/// EdDSA (legacy) with a signing subkey.
static ED: LazyLock<SignedSecretKey> = LazyLock::new(|| secret(ALICE, Make { subkey: true, seed: 22, ..Make::default() }));
/// ECDSA on NIST P-256.
static P256: LazyLock<SignedSecretKey> = LazyLock::new(|| made_with(KeyType::ECDSA(ECCCurve::P256), KeyVersion::V4, None, 23));

/// A key of `key_type` for alice@example.org, with `subkey` (encryption only) when given.
fn made_with(key_type: KeyType, version: KeyVersion, subkey: Option<KeyType>, seed: u64) -> SignedSecretKey {
    let mut params = SecretKeyParamsBuilder::default();
    params.version(version).key_type(key_type).can_certify(true).can_sign(true).primary_user_id(ALICE.into());
    if let Some(sub_type) = subkey {
        let mut sub = SubkeyParamsBuilder::default();
        sub.version(version).key_type(sub_type).can_sign(false).can_encrypt(EncryptionCaps::All);
        params.subkeys(vec![sub.build().unwrap()]);
    }
    params.build().unwrap().generate(StdRng::seed_from_u64(seed)).unwrap()
}

fn key_bytes(secret: &SignedSecretKey) -> Vec<u8> {
    SignedPublicKey::from(secret.clone()).to_bytes().unwrap()
}

fn only_key(secret: &SignedSecretKey) -> SigningKey {
    let mut keys = signing_keys(armoured_public(secret).as_bytes(), LATER);
    assert_eq!(keys.len(), 1, "{keys:?}");
    keys.remove(0)
}

fn pgp_hash(hash: HashAlgorithm) -> pgp::crypto::hash::HashAlgorithm {
    match hash {
        HashAlgorithm::Sha256 => pgp::crypto::hash::HashAlgorithm::Sha256,
        HashAlgorithm::Sha384 => pgp::crypto::hash::HashAlgorithm::Sha384,
        HashAlgorithm::Sha512 => pgp::crypto::hash::HashAlgorithm::Sha512,
    }
}

/// What gpg-agent's PKSIGN gives back for `digest`, made with `secret`'s signing key: RSA
/// PKCS #1 v1.5 with the hash's DigestInfo, ECDSA over the digest, EdDSA with the digest as
/// its message.
fn holder(secret: &SignedSecretKey, hash: HashAlgorithm, digest: &[u8]) -> SignatureValue {
    match signer(secret).sign(&Password::empty(), pgp_hash(hash), digest).unwrap() {
        SignatureBytes::Mpis(m) if m.len() == 1 => SignatureValue::Rsa { s: m[0].as_ref().to_vec() },
        SignatureBytes::Mpis(m) => SignatureValue::Ecc { r: m[0].as_ref().to_vec(), s: m[1].as_ref().to_vec() },
        SignatureBytes::Native(b) => SignatureValue::Ecc { r: b[..32].to_vec(), s: b[32..].to_vec() },
    }
}

fn sign(secret: &SignedSecretKey, key: &SigningKey, data: &[u8], hash: HashAlgorithm) -> String {
    let at = made(secret) + 60;
    sign_detached(data, key, at, hash, |d| Ok::<_, ()>(holder(secret, hash, d))).unwrap()
}

fn yes(_: &Fingerprint, _: &str) -> bool {
    true
}

fn checked(secret: &SignedSecretKey, part: &str, armour: &str) -> Outcome {
    let ring: KeyRing = [public(secret)].into_iter().collect();
    crate::verify(&pgp_mime("alice@example.org", part, armour), ALICE, &ring, LATER, yes).unwrap().outcome
}

fn round_trip(secret: &SignedSecretKey, hash: HashAlgorithm, algorithm: &str) -> SigningKey {
    let key = only_key(secret);
    assert_eq!(key.algorithm(), algorithm);
    let armour = sign(secret, &key, SIGNED_PART.as_bytes(), hash);
    assert!(armour.starts_with("-----BEGIN PGP SIGNATURE-----\r\n\r\n"), "{armour}");
    assert!(armour.ends_with("-----END PGP SIGNATURE-----\r\n"), "{armour}");
    let Outcome::Verified(signer) = checked(secret, SIGNED_PART, &armour) else { panic!("{armour}") };
    assert_eq!(&signer.fingerprint, key.primary());
    assert_eq!(signer.key_id, key.key_id());
    assert_eq!(signer.signed, u64::from(made(secret) + 60));
    // Altered after signing: changed.
    let outcome = checked(secret, &SIGNED_PART.replace("attached", "lost"), &armour);
    assert!(matches!(outcome, Outcome::Changed(_)), "{outcome:?}");
    key
}

#[test]
fn an_rsa_signature_round_trips() {
    let key = round_trip(&RSA, HashAlgorithm::Sha256, "RSA");
    assert_eq!(key.fingerprint(), key.primary());
    assert_eq!(key.addresses(), ["alice@example.org".to_string()]);
    assert_eq!(key.created(), u64::from(made(&RSA)));
    assert_eq!(key.expires(), None);
}

#[test]
fn an_eddsa_subkey_signature_round_trips_with_each_hash() {
    for hash in [HashAlgorithm::Sha256, HashAlgorithm::Sha384, HashAlgorithm::Sha512] {
        let key = round_trip(&ED, hash, "EdDSA");
        assert_ne!(key.fingerprint(), key.primary());
        assert_eq!(key.primary(), public(&ED).fingerprint());
    }
}

#[test]
fn an_ecdsa_p256_signature_round_trips() {
    round_trip(&P256, HashAlgorithm::Sha256, "ECDSA");
    round_trip(&P256, HashAlgorithm::Sha512, "ECDSA");
}

#[test]
fn a_version_4_ed25519_key_signs() {
    let secret = made_with(KeyType::Ed25519, KeyVersion::V4, None, 24);
    round_trip(&secret, HashAlgorithm::Sha256, "Ed25519");
}

#[test]
fn micalg_names_the_hash() {
    assert_eq!(HashAlgorithm::Sha256.micalg(), "pgp-sha256");
    assert_eq!(HashAlgorithm::Sha384.micalg(), "pgp-sha384");
    assert_eq!(HashAlgorithm::Sha512.micalg(), "pgp-sha512");
}

#[test]
fn values_with_leading_zeros_still_encode() {
    for secret in [&*RSA, &*ED, &*P256] {
        let key = only_key(secret);
        let hash = HashAlgorithm::Sha256;
        let armour = sign_detached(SIGNED_PART.as_bytes(), &key, made(secret) + 60, hash, |d| {
            Ok::<_, ()>(match holder(secret, hash, d) {
                SignatureValue::Rsa { s } => SignatureValue::Rsa { s: [vec![0, 0], s].concat() },
                SignatureValue::Ecc { r, s } => SignatureValue::Ecc { r: [vec![0], r].concat(), s: [vec![0, 0, 0], s].concat() },
            })
        })
        .unwrap();
        assert!(matches!(checked(secret, SIGNED_PART, &armour), Outcome::Verified(_)));
    }
}

#[test]
fn garbage_from_the_holder_is_a_bad_signature() {
    let data = SIGNED_PART.as_bytes();
    for secret in [&*RSA, &*ED, &*P256] {
        let key = only_key(secret);
        let junk = [
            SignatureValue::Rsa { s: vec![0x5A; 256] },
            SignatureValue::Rsa { s: Vec::new() },
            SignatureValue::Ecc { r: vec![1; 32], s: vec![2; 32] },
            SignatureValue::Ecc { r: Vec::new(), s: Vec::new() },
            SignatureValue::Ecc { r: vec![0xFF; 4096], s: vec![1] },
        ];
        for value in junk {
            let result = sign_detached(data, &key, 1_700_000_000, HashAlgorithm::Sha256, |_| Ok::<_, ()>(value.clone()));
            assert_eq!(result, Err(SignError::BadSignature), "{value:?}");
        }
        // A real signature, but over another digest.
        let other = holder(secret, HashAlgorithm::Sha256, &[7; 32]);
        let result = sign_detached(data, &key, made(secret) + 60, HashAlgorithm::Sha256, |_| Ok::<_, ()>(other));
        assert_eq!(result, Err(SignError::BadSignature));
    }
}

#[test]
fn a_signature_by_another_key_is_a_bad_signature() {
    let key = only_key(&P256);
    let other = made_with(KeyType::ECDSA(ECCCurve::P256), KeyVersion::V4, None, 31);
    let result = sign_detached(SIGNED_PART.as_bytes(), &key, made(&P256) + 60, HashAlgorithm::Sha256, |d| {
        Ok::<_, ()>(holder(&other, HashAlgorithm::Sha256, d))
    });
    assert_eq!(result, Err(SignError::BadSignature));
}

#[test]
fn the_holders_error_is_passed_through() {
    let key = only_key(&ED);
    let mut seen = 0;
    let result = sign_detached(SIGNED_PART.as_bytes(), &key, 1_700_000_000, HashAlgorithm::Sha384, |d| {
        seen = d.len();
        Err::<SignatureValue, _>("cancelled")
    });
    assert_eq!(result, Err(SignError::Holder("cancelled")));
    assert_eq!(seen, 48, "the digest is the hash's size");
}

#[test]
fn too_much_to_sign_is_too_large() {
    let key = only_key(&ED);
    let data = vec![0u8; crate::MAX_MESSAGE + 1];
    let result = sign_detached(&data, &key, 1, HashAlgorithm::Sha256, |_| -> Result<SignatureValue, ()> { panic!("not asked") });
    assert_eq!(result, Err(SignError::TooLarge));
}

/// The fields of a key packet: MPIs, or a curve OID and then an MPI.
fn mpis(mut fields: &[u8], oid: bool) -> Vec<Vec<u8>> {
    if oid {
        let len = fields[0] as usize;
        fields = &fields[1 + len..];
    }
    let mut out = Vec::new();
    while fields.len() >= 2 {
        let bits = u16::from_be_bytes([fields[0], fields[1]]) as usize;
        let n = bits.div_ceil(8);
        out.push(fields[2..2 + n].to_vec());
        fields = &fields[2 + n..];
    }
    out
}

fn fields(secret: &SignedSecretKey) -> Vec<u8> {
    let key = SignedPublicKey::from(secret.clone());
    match key.public_subkeys.first() {
        Some(sub) => sub.key.public_params().to_bytes().unwrap(),
        None => key.primary_key.public_params().to_bytes().unwrap(),
    }
}

#[test]
fn matches_the_holders_rsa_key_with_leading_zeros() {
    let key = only_key(&RSA);
    let m = mpis(&fields(&RSA), false);
    let (n, e) = (&m[0], &m[1]);
    assert!(key.matches(&AgentKey::Rsa { n, e }));
    let padded = [vec![0u8], n.clone()].concat();
    assert!(key.matches(&AgentKey::Rsa { n: &padded, e }));
    let mut other = n.clone();
    *other.last_mut().unwrap() ^= 2;
    assert!(!key.matches(&AgentKey::Rsa { n: &other, e }));
    assert!(!key.matches(&AgentKey::Rsa { n, e: &[3] }));
    assert!(!key.matches(&AgentKey::Ecc { curve: "Ed25519", q: n }));
}

#[test]
fn matches_the_holders_ed25519_point_with_or_without_its_prefix() {
    let key = only_key(&ED);
    let q = &mpis(&fields(&ED), true)[0];
    assert_eq!(q.len(), 33);
    assert_eq!(q[0], 0x40);
    assert!(key.matches(&AgentKey::Ecc { curve: "Ed25519", q }));
    assert!(key.matches(&AgentKey::Ecc { curve: "ed25519", q: &q[1..] }));
    assert!(key.matches(&AgentKey::Ecc { curve: "\"1.3.6.1.4.1.11591.15.1\"", q }));
    assert!(!key.matches(&AgentKey::Ecc { curve: "NIST P-256", q }));
    // The primary key of the same certificate isn't the signing subkey.
    let primary = SignedPublicKey::from(ED.clone()).primary_key.public_params().to_bytes().unwrap();
    assert!(!key.matches(&AgentKey::Ecc { curve: "Ed25519", q: &mpis(&primary, true)[0] }));
    let other = only_key(&made_with(KeyType::Ed25519Legacy, KeyVersion::V4, None, 41));
    assert!(!other.matches(&AgentKey::Ecc { curve: "Ed25519", q }));
}

#[test]
fn matches_the_holders_p256_point_by_its_curve() {
    let key = only_key(&P256);
    let q = &mpis(&fields(&P256), true)[0];
    assert!(key.matches(&AgentKey::Ecc { curve: "NIST P-256", q }));
    assert!(key.matches(&AgentKey::Ecc { curve: "nistp256", q }));
    assert!(!key.matches(&AgentKey::Ecc { curve: "NIST P-384", q }));
    assert!(!key.matches(&AgentKey::Ecc { curve: "brainpoolP256r1", q }));
    assert!(!key.matches(&AgentKey::Rsa { n: q, e: &[1, 0, 1] }));
}

#[test]
fn a_signing_subkey_without_its_back_signature_is_not_offered() {
    let mut key = SignedPublicKey::from(ED.clone());
    let mut flags = KeyFlags::default();
    flags.set_sign(true);
    let binding = config(&ED.primary_key, SignatureType::SubkeyBinding, made(&ED) + 10, vec![SubpacketData::KeyFlags(flags)])
        .sign_subkey_binding(&ED.primary_key, &key.primary_key, &Password::empty(), &key.public_subkeys[0].key)
        .unwrap();
    key.public_subkeys[0].signatures = vec![binding];
    // The primary key was made to certify only; the subkey can't sign without signing back.
    assert!(signing_keys(&key.to_bytes().unwrap(), LATER).is_empty());
}

#[test]
fn an_encryption_only_subkey_is_not_offered() {
    let secret = made_with(KeyType::Ed25519Legacy, KeyVersion::V4, Some(KeyType::ECDH(ECCCurve::Curve25519Legacy)), 42);
    let keys = signing_keys(&key_bytes(&secret), LATER);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].fingerprint(), keys[0].primary());
}

#[test]
fn a_revoked_key_is_not_offered() {
    let mut key = SignedPublicKey::from(ED.clone());
    let sig = config(&ED.primary_key, SignatureType::KeyRevocation, made(&ED) + 10, vec![])
        .sign_key(&ED.primary_key, &Password::empty(), &key.primary_key)
        .unwrap();
    key.details.revocation_signatures.push(sig);
    assert!(signing_keys(&key.to_bytes().unwrap(), LATER).is_empty());
}

#[test]
fn an_expired_key_is_not_offered_after_it_expires() {
    let secret = made_with(KeyType::Ed25519Legacy, KeyVersion::V4, None, 43);
    let t = made(&secret);
    let mut key = SignedPublicKey::from(secret.clone());
    let id = key.details.users[0].id.clone();
    let sig = config(&secret.primary_key, SignatureType::CertPositive, t + 10, vec![SubpacketData::KeyExpirationTime(Duration::from_secs(1_000))])
        .sign_certification(&secret.primary_key, &key.primary_key, &Password::empty(), pgp::types::Tag::UserId, &id)
        .unwrap();
    key.details.users[0].signatures.push(sig);
    let bytes = key.to_bytes().unwrap();
    let before = signing_keys(&bytes, u64::from(t) + 500);
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].expires(), Some(u64::from(t) + 1_000));
    assert!(signing_keys(&bytes, u64::from(t) + 1_000).is_empty());
    // Nor before it was made.
    assert!(signing_keys(&bytes, u64::from(t) - 1).is_empty());
}

#[test]
fn a_version_6_key_is_not_offered() {
    let secret = secret(ALICE, Make { v6: true, seed: 44, ..Make::default() });
    assert!(signing_keys(&key_bytes(&secret), LATER).is_empty());
}

#[test]
fn many_keys_in_one_export_are_all_read() {
    let mut binary = Vec::new();
    let mut armoured = String::new();
    let mut expected = Vec::new();
    for seed in 50..62 {
        let secret = secret(&format!("P{seed} <p{seed}@example.org>"), Make { seed, ..Make::default() });
        binary.extend(key_bytes(&secret));
        armoured.push_str(&armoured_public(&secret));
        expected.push(public(&secret).fingerprint().clone());
    }
    binary.extend(key_bytes(&RSA));
    binary.extend(key_bytes(&ED));
    binary.extend(key_bytes(&P256));
    let from_binary: Vec<Fingerprint> = signing_keys(&binary, LATER).iter().map(|k| k.primary().clone()).collect();
    assert_eq!(from_binary.len(), expected.len() + 3);
    assert_eq!(from_binary[..expected.len()], expected[..]);
    let from_armour: Vec<Fingerprint> = signing_keys(armoured.as_bytes(), LATER).iter().map(|k| k.primary().clone()).collect();
    assert_eq!(from_armour, expected);
    // One armour block holding them all, as `gpg --export --armor` writes it.
    let one_block = format!("-----BEGIN PGP PUBLIC KEY BLOCK-----\r\n\r\n{}\r\n-----END PGP PUBLIC KEY BLOCK-----\r\n", base64(&binary));
    assert_eq!(signing_keys(one_block.as_bytes(), LATER).len(), expected.len() + 3);
}

#[test]
fn a_broken_certificate_among_good_ones_is_left_out() {
    let mut bytes = key_bytes(&ED);
    let mut broken = key_bytes(&P256);
    let last = broken.len() - 5;
    broken[last] ^= 0xFF;
    bytes.extend(broken);
    bytes.extend(key_bytes(&RSA));
    let keys: Vec<&'static str> = signing_keys(&bytes, LATER).iter().map(SigningKey::algorithm).collect();
    assert_eq!(keys, ["EdDSA", "RSA"]);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn signing_keys_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = signing_keys(&bytes, LATER);
    }

    #[test]
    fn signing_keys_never_panics_on_a_cut_or_flipped_key(cut in 0usize..4096, flip in 0usize..4096, bit in 0u8..8) {
        let mut bytes = key_bytes(&ED);
        let at = flip % bytes.len();
        bytes[at] ^= 1 << bit;
        bytes.truncate(cut % (bytes.len() + 1));
        let _ = signing_keys(&bytes, LATER);
        let mut armoured = String::from("-----BEGIN PGP PUBLIC KEY BLOCK-----\n\n");
        armoured.push_str(&base64(&bytes));
        armoured.push_str("\n-----END PGP PUBLIC KEY BLOCK-----\n");
        let _ = signing_keys(armoured.as_bytes(), LATER);
    }

    #[test]
    fn any_value_from_the_holder_is_refused_or_checks(r in prop::collection::vec(any::<u8>(), 0..80), s in prop::collection::vec(any::<u8>(), 0..80), rsa in any::<bool>()) {
        let key = only_key(&ED);
        let value = if rsa { SignatureValue::Rsa { s } } else { SignatureValue::Ecc { r, s } };
        let result = sign_detached(SIGNED_PART.as_bytes(), &key, 1_700_000_000, HashAlgorithm::Sha256, |_| Ok::<_, ()>(value));
        prop_assert_eq!(result, Err(SignError::BadSignature));
    }
}
