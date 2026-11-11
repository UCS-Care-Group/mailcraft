// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypting to OpenPGP recipients (#30 part 7): an OpenPGP message (RFC 9580 §10.3) of a MIME
//! entity, for the body of a PGP/MIME `multipart/encrypted` message (RFC 3156 §4).
//!
//! Only Public keys are used here. A random session key encrypts the entity (SEIPD version 1 with
//! the strongest of AES-256, AES-192 and AES-128 that every recipient's key lists; AES-128 is the
//! one every implementation must have; version 2 with AES-256 and OCB when every recipient's key
//! is a version 6 key), and is encrypted to each recipient's encryption key: the newest subkey
//! that may encrypt, binding signature checked, neither revoked nor expired nor weak, else the
//! primary key when it may. A key without one isn't encrypted to ([`EncryptError::Key`]).
//!
//! The random numbers come from the `random` function the host hands in, as for S/MIME
//! (`mailcraft_smime::encrypt`): a source that fails ends it, and nothing made meanwhile is given
//! back. rPGP does the arithmetic; the RSA it uses is a public-key operation only.

use pgp::composed::{ArmorOptions, MessageBuilder};
use pgp::crypto::aead::{AeadAlgorithm, ChunkSize};
use pgp::crypto::public_key::PublicKeyAlgorithm;
use pgp::crypto::sym::SymmetricKeyAlgorithm;
use pgp::types::KeyVersion;
use rand_core::{CryptoRng, RngCore};

use crate::MAX_MESSAGE;
use crate::keys::{Budget, Encrypting, MAX_CHECKS_PER_CALL, PublicKey};

/// The most recipients one message is encrypted to.
pub const MAX_RECIPIENTS: usize = 200;

/// Why nothing was encrypted. Nothing in it is of the content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptError {
    /// There is nobody to encrypt to.
    NoRecipients,
    /// A key can't be encrypted to: what is wrong with it.
    Key(&'static str),
    /// The content or the list of recipients is larger than is taken.
    TooLarge,
    /// The random numbers couldn't be had.
    Random,
    /// The message couldn't be written.
    Encoding,
}

impl std::fmt::Display for EncryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EncryptError::NoRecipients => f.write_str("there is nobody to encrypt to"),
            EncryptError::Key(why) => write!(f, "a key can't be encrypted to: {why}"),
            EncryptError::TooLarge => f.write_str("the message is too large to encrypt"),
            EncryptError::Random => f.write_str("the system's random numbers couldn't be had"),
            EncryptError::Encoding => f.write_str("the encrypted message couldn't be written"),
        }
    }
}

impl std::error::Error for EncryptError {}

/// A certificate of the owner's that mail may be encrypted to: for the owner's own key, which the
/// host finds in GnuPG, to be encrypted to as well.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    /// The primary key's fingerprint, in hex.
    pub fingerprint: String,
    /// The addresses of its User IDs.
    pub addresses: Vec<String>,
    /// The certificate as binary OpenPGP packets, as [`encrypt`] takes it.
    pub bytes: Vec<u8>,
}

/// The key of `key` to encrypt to at `now`: the newest that is valid, strong enough and of an
/// algorithm rPGP encrypts with; why not when there is none.
fn usable(key: &PublicKey, now: u64) -> Result<&Encrypting, &'static str> {
    let Some(primary) = key.primary() else { return Err("its primary key can't be read") };
    if primary.revoked.is_some() {
        return Err("it has been revoked");
    }
    if primary.expired_at(now) {
        return Err("it has expired");
    }
    let mut why = "it has no key to encrypt to";
    let mut best: Option<&Encrypting> = None;
    for e in &key.encrypting {
        let supported = matches!(
            e.algorithm,
            PublicKeyAlgorithm::RSA
                | PublicKeyAlgorithm::RSAEncrypt
                | PublicKeyAlgorithm::ECDH
                | PublicKeyAlgorithm::X25519
                | PublicKeyAlgorithm::X448
        ) && matches!(e.version, KeyVersion::V4 | KeyVersion::V6);
        if !supported {
            why = "its key is of a kind that isn't encrypted to here";
        } else if e.revoked {
            why = "its encryption key has been revoked";
        } else if e.expires.is_some_and(|end| now >= end) {
            why = "its encryption key has expired";
        } else if u64::from(e.created) > now {
            why = "its encryption key isn't valid yet";
        } else if e.weak.is_some() {
            why = "its encryption key is too weak";
        } else if best.is_none_or(|b| e.created > b.created) {
            best = Some(e);
        }
    }
    best.ok_or(why)
}

/// The first certificate in `bytes` (one address's kept key) that may be encrypted to at `now`.
fn certificate(bytes: &[u8], now: u64) -> Result<PublicKey, &'static str> {
    let keys = PublicKey::parse_with(bytes, &mut Budget::new(MAX_CHECKS_PER_CALL)).map_err(|_| "it can't be read")?;
    let mut why = "it can't be read";
    for key in keys {
        match usable(&key, now) {
            Ok(_) => return Ok(key),
            Err(w) => why = w,
        }
    }
    Err(why)
}

/// Whether mail may be encrypted to the key in `bytes` at `now` (seconds since 1970); what is
/// wrong when it can't, in words.
pub fn can_encrypt_to(bytes: &[u8], now: u64) -> Result<(), &'static str> {
    crate::guard(|| certificate(bytes, now).map(|_| ())).unwrap_or(Err("it can't be read"))
}

/// The certificates in `bytes` (binary or armoured, as `gpg --export` writes them) that mail may
/// be encrypted to at `now`, at most 64. Nothing is trusted here: it is the owner's own keyring.
pub fn encryption_certificates(bytes: &[u8], now: u64) -> Vec<Certificate> {
    const MAX_FOUND: usize = 64;
    let mut out = Vec::new();
    if bytes.len() > crate::sign::MAX_INPUT {
        return out;
    }
    let blocks = crate::armour::blocks(bytes, crate::armour::Kind::PublicKey, 1_024, crate::sign::MAX_INPUT);
    let blocks = if blocks.is_empty() { vec![bytes.to_vec()] } else { blocks };
    let mut budget = Budget::new(16_384);
    for block in &blocks {
        for piece in crate::sign::split_certificates(block, 1_024) {
            let Some(Ok(key)) = crate::guard(|| pgp::composed::Deserializable::from_bytes(piece)) else { continue };
            let Some(Some(key)) = crate::guard(|| PublicKey::checked(key, &mut budget)) else { continue };
            if out.len() >= MAX_FOUND {
                return out;
            }
            if usable(&key, now).is_ok() {
                out.push(Certificate { fingerprint: key.fingerprint().to_hex(), addresses: key.addresses().to_vec(), bytes: key.to_bytes() });
            }
        }
    }
    out
}

/// The random numbers the host gave, as rPGP asks for them. When the host fails, what was asked
/// for is zeros and [`Random::failed`] says so: whatever is made from it is thrown away.
struct Random<'a> {
    fill: &'a mut dyn FnMut(&mut [u8]) -> bool,
    failed: bool,
}

impl Random<'_> {
    fn bytes(&mut self, dst: &mut [u8]) {
        if self.failed || !(self.fill)(dst) {
            self.failed = true;
            dst.fill(0);
        }
    }
}

impl RngCore for Random<'_> {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.bytes(&mut b);
        u32::from_le_bytes(b)
    }
    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.bytes(&mut b);
        u64::from_le_bytes(b)
    }
    fn fill_bytes(&mut self, dst: &mut [u8]) {
        self.bytes(dst);
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), rand_core::Error> {
        self.bytes(dst);
        if self.failed { Err(rand_core::Error::new(NoRandom)) } else { Ok(()) }
    }
}

impl CryptoRng for Random<'_> {}

#[derive(Debug)]
struct NoRandom;

impl std::fmt::Display for NoRandom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no random numbers")
    }
}

impl std::error::Error for NoRandom {}

/// The armoured OpenPGP message (`-----BEGIN PGP MESSAGE-----`) of `content` for every
/// recipient: each a certificate as kept (binary packets). `now` is seconds since 1970. `random`
/// fills its buffer with random bytes from the operating system and says whether it could.
pub fn encrypt(content: &[u8], recipients: &[Vec<u8>], now: u64, random: &mut dyn FnMut(&mut [u8]) -> bool) -> Result<String, EncryptError> {
    if content.is_empty() {
        return Err(EncryptError::Encoding);
    }
    if content.len() > MAX_MESSAGE || recipients.len() > MAX_RECIPIENTS {
        return Err(EncryptError::TooLarge);
    }
    if recipients.is_empty() {
        return Err(EncryptError::NoRecipients);
    }
    let mut keys: Vec<PublicKey> = Vec::new();
    for bytes in recipients {
        let key = crate::guard(|| certificate(bytes, now)).unwrap_or(Err("it can't be read")).map_err(EncryptError::Key)?;
        // Each recipient once, whatever way the list was made.
        if !keys.iter().any(|k| k.fingerprint() == key.fingerprint()) {
            keys.push(key);
        }
    }
    let mut random = Random { fill: random, failed: false };
    let made = crate::guard(|| write(content, &keys, now, &mut random)).unwrap_or(Err(EncryptError::Encoding));
    if random.failed {
        return Err(EncryptError::Random);
    }
    made
}

/// Encrypt the session key of `builder` to the key `e` of `key` (the builder's two kinds share
/// no type to name).
macro_rules! add_to {
    ($builder:expr, $rng:expr, $key:expr, $e:expr) => {{
        let inner = $key.inner();
        let added = match $e.subkey {
            None => $builder.encrypt_to_key(&mut *$rng, &inner.primary_key).map(|_| ()),
            Some(i) => match inner.public_subkeys.get(i) {
                Some(sub) => $builder.encrypt_to_key(&mut *$rng, &sub.key).map(|_| ()),
                None => return Err(EncryptError::Key("its encryption key can't be found")),
            },
        };
        added.map_err(|_| EncryptError::Key("its key can't be encrypted to"))?;
    }};
}

fn write(content: &[u8], keys: &[PublicKey], now: u64, rng: &mut Random<'_>) -> Result<String, EncryptError> {
    let mut chosen = Vec::new();
    for key in keys {
        chosen.push((key, usable(key, now).map_err(EncryptError::Key)?));
    }
    let all_v6 = chosen.iter().all(|(_, e)| e.version == KeyVersion::V6);
    let source = MessageBuilder::from_bytes(Vec::<u8>::new(), content.to_vec());
    let armoured = if all_v6 {
        // Version 6 keys read SEIPD version 2 (RFC 9580 §5.13.2): AES-256 in OCB mode.
        let mut builder = source.seipd_v2(&mut *rng, SymmetricKeyAlgorithm::AES256, AeadAlgorithm::Ocb, ChunkSize::default());
        for (key, e) in &chosen {
            add_to!(builder, rng, key, e);
        }
        builder.to_armored_string(&mut *rng, ArmorOptions::default())
    } else {
        let alg = [SymmetricKeyAlgorithm::AES256, SymmetricKeyAlgorithm::AES192]
            .into_iter()
            .find(|a| keys.iter().all(|k| k.prefs.symmetric.contains(a)))
            .unwrap_or(SymmetricKeyAlgorithm::AES128);
        let mut builder = source.seipd_v1(&mut *rng, alg);
        for (key, e) in &chosen {
            add_to!(builder, rng, key, e);
        }
        builder.to_armored_string(&mut *rng, ArmorOptions::default())
    };
    armoured.map_err(|_| EncryptError::Encoding)
}
