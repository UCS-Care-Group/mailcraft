// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A transferable secret key (RFC 9580 §10.2) read for the window to import (#30 R13): what it
//! is (its fingerprint and addresses, its public half for the key ring), whether every secret
//! part of it is protected by a passphrase, and whether it can sign. Read only: nothing secret is
//! decrypted or used here (an rPGP advisory concerns using an RSA private key, which this never
//! does), and the secret bytes are not kept. The file is hostile: the caps of the Public keys'
//! reader hold, and nothing here panics on any bytes.

use pgp::composed::{Deserializable, SignedSecretKey};
use pgp::types::KeyDetails;

use crate::Error;
use crate::armour::{self, Kind};
use crate::keys::{Budget, Fingerprint, MAX_CHECKS_PER_CALL, MAX_KEY_BYTES, PublicKey, packet_tags};

/// How many packets a secret key may be made of.
const MAX_PACKETS: usize = 4_096;
/// The packet tags of a secret key and a secret subkey (RFC 9580 §5.5.1.3, §5.5.1.4).
const SECRET_KEY: u8 = 5;
const SECRET_SUBKEY: u8 = 7;

/// What a secret key is, read without decrypting anything.
#[derive(Clone, Debug)]
pub struct SecretKeyInfo {
    /// The primary key's fingerprint.
    pub fingerprint: Fingerprint,
    /// The addresses of the User IDs it certified itself, lower case.
    pub addresses: Vec<String>,
    /// Its public half, its self-signatures checked: for the key ring, and to keep for its
    /// addresses.
    pub public: PublicKey,
    /// Whether every secret part of it (the primary key's and each subkey's) is protected by a
    /// passphrase.
    pub protected: bool,
    /// Whether it can make Digital signatures: a key of it that may sign, isn't revoked, isn't
    /// of an algorithm no longer safe, and has its secret part here.
    pub can_sign: bool,
    /// The algorithm of the key it would sign with (the first that can): whether it may be
    /// used here. `None` when it can't sign.
    pub signing_kind: Option<SigningKind>,
}

/// What kind of key signs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningKind {
    Rsa,
    /// EdDSA: Ed25519 (either way of writing it) or Ed448.
    EdDsa,
    Ecdsa,
    /// Any other (DSA, or one not known).
    Other,
}

impl SigningKind {
    fn of(algorithm: pgp::crypto::public_key::PublicKeyAlgorithm) -> SigningKind {
        use pgp::crypto::public_key::PublicKeyAlgorithm as A;
        match algorithm {
            A::RSA | A::RSASign | A::RSAEncrypt => SigningKind::Rsa,
            A::EdDSALegacy | A::Ed25519 | A::Ed448 => SigningKind::EdDsa,
            A::ECDSA => SigningKind::Ecdsa,
            _ => SigningKind::Other,
        }
    }
}

/// Reads the transferable secret key in `bytes`: armoured (`-----BEGIN PGP PRIVATE KEY
/// BLOCK-----`) or binary. The first key, when there are several. [`Error::Unreadable`] when
/// there is no secret key, or its self-signatures don't check; [`Error::TooLarge`] past the
/// size of any key.
pub fn read(bytes: &[u8]) -> Result<SecretKeyInfo, Error> {
    let block = match armour::blocks(bytes, Kind::PrivateKey, 1, MAX_KEY_BYTES).into_iter().next() {
        Some(block) => block,
        None if bytes.len() > MAX_KEY_BYTES => return Err(Error::TooLarge),
        None => bytes.to_vec(),
    };
    let tags = packet_tags(&block, MAX_PACKETS.saturating_add(1));
    if tags.len() > MAX_PACKETS || tags.first() != Some(&SECRET_KEY) {
        return Err(Error::Unreadable);
    }
    let parsed = crate::guard(|| SignedSecretKey::from_bytes_many(block.as_slice()).map(|mut it| it.next()));
    let Some(Ok(Some(Ok(key)))) = parsed else { return Err(Error::Unreadable) };
    crate::guard(|| info(&key)).flatten().ok_or(Error::Unreadable)
}

fn info(key: &SignedSecretKey) -> Option<SecretKeyInfo> {
    let public = PublicKey::checked(key.to_public_key(), &mut Budget::new(MAX_CHECKS_PER_CALL))?;
    let protected = key.primary_key.secret_params().is_encrypted() && key.secret_subkeys.iter().all(|s| s.key.secret_params().is_encrypted());
    let mut secret: Vec<Fingerprint> = vec![Fingerprint::of(&key.primary_key.fingerprint())];
    secret.extend(key.secret_subkeys.iter().map(|s| Fingerprint::of(&s.key.fingerprint())));
    let signer = public.signing.iter().find(|s| s.may_sign && s.revoked.is_none() && s.weak.is_none() && secret.contains(&s.fingerprint));
    let can_sign = signer.is_some();
    let signing_kind = signer.map(|s| SigningKind::of(s.algorithm));
    Some(SecretKeyInfo {
        fingerprint: public.fingerprint().clone(),
        addresses: public.addresses().to_vec(),
        public,
        protected,
        can_sign,
        signing_kind,
    })
}

/// Whether `bytes` hold OpenPGP secret key material: an armoured `PRIVATE KEY BLOCK`, or binary
/// packets among which is a secret key or subkey. So that a secret key is never taken for a
/// Public key to import.
pub fn holds_secret(bytes: &[u8]) -> bool {
    if bytes.windows(25).any(|w| w == b"BEGIN PGP PRIVATE KEY BLO") {
        return true;
    }
    let secret = |packets: &[u8]| packet_tags(packets, MAX_PACKETS).iter().any(|t| matches!(*t, SECRET_KEY | SECRET_SUBKEY));
    secret(bytes) || armour::blocks(bytes, Kind::PublicKey, crate::keys::MAX_KEYS_PER_BLOCK, MAX_KEY_BYTES).iter().any(|b| secret(b))
}
