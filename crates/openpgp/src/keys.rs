// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public keys: read from their bytes (armoured or not), their self-signatures checked once
//! when they are read, and kept in a [`KeyRing`] that finds them by fingerprint, key id or
//! address. A key's addresses are those of the User IDs it signed itself (RFC 9580 §5.2.1,
//! §10.1); nothing here is a web of trust (R6): whether a key is Verified is the caller's to say.

use pgp::composed::{Deserializable, SignedPublicKey};
use pgp::crypto::hash::HashAlgorithm;
use pgp::crypto::public_key::PublicKeyAlgorithm;
use pgp::packet::{KeyFlags, Signature, SignatureType, SubpacketData};
use pgp::ser::Serialize;
use pgp::types::{KeyDetails, KeyVersion, Tag};

use crate::armour::{self, Kind};
use crate::{Error, mime};

/// How many bytes a block of Public keys may hold, decoded.
pub(crate) const MAX_KEY_BYTES: usize = 1 << 20;
/// How many keys one block (a file, an attachment) is read for.
pub(crate) const MAX_KEYS_PER_BLOCK: usize = 16;
/// How many User IDs, subkeys and signatures on each are looked at in one key: more is not a
/// key anybody sends, and each one costs a signature check.
const MAX_USERS: usize = 64;
const MAX_SUBKEYS: usize = 32;
const MAX_SIGNATURES: usize = 64;
/// How many signature checks reading one key may cost (third-party certifications are never
/// checked, so a real key needs a handful).
const MAX_CHECKS: usize = 256;
/// How many packets one block is read for before it is given up on.
const MAX_PACKETS: usize = 4_096;

/// A key's fingerprint (RFC 9580 §5.5.4): 20 bytes for a version 4 key, 32 for version 6.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Fingerprint(Vec<u8>);

impl Fingerprint {
    /// The fingerprint written as hexadecimal digits (spaces and a `0x` in front allowed).
    pub fn from_hex(text: &str) -> Option<Fingerprint> {
        let text = text.trim();
        let text = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")).unwrap_or(text);
        let digits: Vec<u32> = text.chars().filter(|c| !c.is_whitespace()).map(|c| c.to_digit(16)).collect::<Option<_>>()?;
        if !matches!(digits.len(), 40 | 64) {
            return None;
        }
        Some(Fingerprint(digits.chunks(2).map(|p| p.iter().fold(0u8, |acc, d| (acc << 4) | (*d as u8))).collect()))
    }

    /// The fingerprint's bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Upper-case hexadecimal, no spaces: what [`Fingerprint::from_hex`] reads back.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// Upper-case hexadecimal in groups of four, for an owner to compare by eye.
    pub fn grouped(&self) -> String {
        let hex = self.to_hex();
        let groups: Vec<&str> = (0..hex.len()).step_by(4).filter_map(|i| hex.get(i..i.saturating_add(4).min(hex.len()))).collect();
        groups.join(" ")
    }

    pub(crate) fn of(fp: &pgp::types::Fingerprint) -> Fingerprint {
        Fingerprint(fp.as_bytes().to_vec())
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A key id (RFC 9580 §5.5.4): eight bytes that a signature names its signer by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyId(pub [u8; 8]);

impl KeyId {
    /// Upper-case hexadecimal, sixteen digits.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02X}")).collect()
    }

    pub(crate) fn of(id: &pgp::types::KeyId) -> KeyId {
        let mut bytes = [0u8; 8];
        for (to, from) in bytes.iter_mut().zip(id.as_ref()) {
            *to = *from;
        }
        KeyId(bytes)
    }
}

impl std::fmt::Display for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// A public-key algorithm's name, as an owner may see it in the details.
pub(crate) fn algorithm_name(algorithm: PublicKeyAlgorithm) -> &'static str {
    match algorithm {
        PublicKeyAlgorithm::RSA | PublicKeyAlgorithm::RSASign | PublicKeyAlgorithm::RSAEncrypt => "RSA",
        PublicKeyAlgorithm::DSA => "DSA",
        PublicKeyAlgorithm::ECDSA => "ECDSA",
        PublicKeyAlgorithm::EdDSALegacy => "EdDSA",
        PublicKeyAlgorithm::Ed25519 => "Ed25519",
        PublicKeyAlgorithm::Ed448 => "Ed448",
        PublicKeyAlgorithm::ECDH => "ECDH",
        PublicKeyAlgorithm::X25519 => "X25519",
        PublicKeyAlgorithm::X448 => "X448",
        PublicKeyAlgorithm::Elgamal | PublicKeyAlgorithm::ElgamalEncrypt => "Elgamal",
        _ => "unknown",
    }
}

/// A hash algorithm's name, as an owner may see it in the details.
pub(crate) fn hash_name(hash: HashAlgorithm) -> &'static str {
    match hash {
        HashAlgorithm::Md5 => "MD5",
        HashAlgorithm::Sha1 => "SHA-1",
        HashAlgorithm::Ripemd160 => "RIPEMD-160",
        HashAlgorithm::Sha224 => "SHA-224",
        HashAlgorithm::Sha256 => "SHA-256",
        HashAlgorithm::Sha384 => "SHA-384",
        HashAlgorithm::Sha512 => "SHA-512",
        HashAlgorithm::Sha3_256 => "SHA3-256",
        HashAlgorithm::Sha3_512 => "SHA3-512",
        _ => "unknown",
    }
}

/// Hashes no longer safe for a signature (R4): MD5, SHA-1 and RIPEMD-160 (RFC 9580 §9.5).
pub(crate) fn is_weak(hash: HashAlgorithm) -> bool {
    matches!(hash, HashAlgorithm::Md5 | HashAlgorithm::Sha1 | HashAlgorithm::Ripemd160)
}

/// One key of a [`PublicKey`] that may make Digital signatures: the primary key or a subkey.
#[derive(Clone, Debug)]
pub(crate) struct Signing {
    /// `None` for the primary key, else the subkey's place in the key.
    pub(crate) subkey: Option<usize>,
    pub(crate) fingerprint: Fingerprint,
    pub(crate) key_id: KeyId,
    pub(crate) version: KeyVersion,
    pub(crate) algorithm: PublicKeyAlgorithm,
    pub(crate) created: u32,
    /// When it stops being valid, in seconds since 1970; `None` when never.
    pub(crate) expires: Option<u64>,
    pub(crate) revoked: bool,
    /// Whether its self-signature lets it sign (subkeys say so; a primary key that says nothing
    /// may, as RFC 9580 §5.2.3.29 leaves it).
    pub(crate) may_sign: bool,
}

/// A Public key (an OpenPGP certificate, RFC 9580 §10.1) whose self-signatures check.
#[derive(Clone, Debug)]
pub struct PublicKey {
    key: SignedPublicKey,
    fingerprint: Fingerprint,
    addresses: Vec<String>,
    /// The newest self-signature's time, to tell an updated copy of the same key.
    updated: u32,
    pub(crate) signing: Vec<Signing>,
}

impl PublicKey {
    /// Every Public key in `bytes`: armoured (`-----BEGIN PGP PUBLIC KEY BLOCK-----`, as many
    /// blocks as there are) or binary, at most 16 a block. A key whose
    /// self-signatures don't check, or that is larger than a key anybody sends, is left out.
    /// [`Error::Unreadable`] when no key at all could be read.
    pub fn parse(bytes: &[u8]) -> Result<Vec<PublicKey>, Error> {
        let mut out = Vec::new();
        let blocks = armour::blocks(bytes, Kind::PublicKey, MAX_KEYS_PER_BLOCK, MAX_KEY_BYTES);
        if blocks.is_empty() {
            if bytes.len() > MAX_KEY_BYTES {
                return Err(Error::TooLarge);
            }
            out.extend(Self::binary(bytes));
        } else {
            for block in blocks {
                let room = MAX_KEYS_PER_BLOCK.saturating_sub(out.len());
                out.extend(Self::binary(&block).into_iter().take(room));
            }
        }
        if out.is_empty() { Err(Error::Unreadable) } else { Ok(out) }
    }

    /// The keys in binary packets.
    pub(crate) fn binary(bytes: &[u8]) -> Vec<PublicKey> {
        if count_packets(bytes) > MAX_PACKETS {
            return Vec::new();
        }
        let Some(Ok(keys)) = crate::guard(|| SignedPublicKey::from_bytes_many(bytes).map(|it| it.take(MAX_KEYS_PER_BLOCK).collect::<Vec<_>>()))
        else {
            return Vec::new();
        };
        keys.into_iter().filter_map(Result::ok).filter_map(|key| crate::guard(|| PublicKey::checked(key)).flatten()).collect()
    }

    /// The key, once its self-signatures check: its addresses from the User IDs it certified,
    /// and the keys of it that may sign.
    fn checked(key: SignedPublicKey) -> Option<PublicKey> {
        if key.details.users.len() > MAX_USERS || key.public_subkeys.len() > MAX_SUBKEYS {
            return None;
        }
        let primary = &key.primary_key;
        let mut addresses: Vec<String> = Vec::new();
        // The newest valid self-signature on the key or one of its User IDs: its key flags and
        // expiry are the primary key's (RFC 9580 §5.2.3.10, §10.1.1).
        let mut newest: Option<&Signature> = None;
        let mut budget = Budget(MAX_CHECKS);
        for sig in key.details.direct_signatures.iter().take(MAX_SIGNATURES) {
            if sig.typ() == Some(SignatureType::Key) && usable(sig) && is_self(sig, primary) && budget.spend() && sig.verify_key(primary).is_ok() {
                consider(sig, &mut newest);
            }
        }
        for user in &key.details.users {
            let mut certified = false;
            let mut revoked = false;
            for sig in user.signatures.iter().take(MAX_SIGNATURES) {
                let Some(typ) = sig.typ() else { continue };
                let certification =
                    matches!(typ, SignatureType::CertGeneric | SignatureType::CertPersona | SignatureType::CertCasual | SignatureType::CertPositive);
                if !(certification || typ == SignatureType::CertRevocation) || !usable(sig) || !is_self(sig, primary) {
                    continue;
                }
                if !budget.spend() || sig.verify_certification(primary, Tag::UserId, &user.id).is_err() {
                    continue;
                }
                if certification {
                    certified = true;
                    consider(sig, &mut newest);
                } else {
                    revoked = true;
                }
            }
            if certified && !revoked {
                let address = mime::address(&String::from_utf8_lossy(user.id.id()));
                if !address.is_empty() && !addresses.contains(&address) {
                    addresses.push(address);
                }
            }
        }
        let newest = newest?;
        let revoked = key.details.revocation_signatures.iter().take(MAX_SIGNATURES).any(|sig| {
            sig.typ() == Some(SignatureType::KeyRevocation)
                && usable(sig)
                && is_self(sig, primary)
                && budget.spend()
                && sig.verify_key(primary).is_ok()
        });
        let primary_created = primary.created_at().as_secs();
        let mut signing = vec![Signing {
            subkey: None,
            fingerprint: Fingerprint::of(&primary.fingerprint()),
            key_id: KeyId::of(&primary.legacy_key_id()),
            version: primary.version(),
            algorithm: primary.algorithm(),
            created: primary_created,
            expires: expiry(primary_created, newest),
            revoked,
            may_sign: flags(newest).is_none_or(|f| f.sign()),
        }];
        let mut updated = created(newest);
        for (i, sub) in key.public_subkeys.iter().enumerate() {
            let mut binding: Option<&Signature> = None;
            let mut sub_revoked = false;
            for sig in sub.signatures.iter().take(MAX_SIGNATURES) {
                if !usable(sig) || !is_self(sig, primary) || !budget.spend() || sig.verify_subkey_binding(primary, &sub.key).is_err() {
                    continue;
                }
                match sig.typ() {
                    Some(SignatureType::SubkeyRevocation) => sub_revoked = true,
                    Some(SignatureType::SubkeyBinding) if binding.is_none_or(|b| created(sig) >= created(b)) => binding = Some(sig),
                    _ => {}
                }
            }
            let Some(binding) = binding else { continue };
            updated = updated.max(created(binding));
            let wants_sign = flags(binding).is_some_and(|f| f.sign());
            // A subkey that signs must sign back that it belongs to this key (§5.2.1): else
            // anybody's key could claim somebody else's subkey.
            let backed = wants_sign
                && binding.embedded_signature().is_some_and(|back| budget.spend() && back.verify_primary_key_binding(&sub.key, primary).is_ok());
            if !wants_sign {
                continue;
            }
            let sub_created = sub.key.created_at().as_secs();
            signing.push(Signing {
                subkey: Some(i),
                fingerprint: Fingerprint::of(&sub.key.fingerprint()),
                key_id: KeyId::of(&sub.key.legacy_key_id()),
                version: sub.key.version(),
                algorithm: sub.key.algorithm(),
                created: sub_created,
                expires: expiry(sub_created, binding),
                revoked: revoked || sub_revoked,
                may_sign: backed,
            });
        }
        Some(PublicKey { fingerprint: Fingerprint::of(&key.fingerprint()), key, addresses, updated, signing })
    }

    /// The primary key's fingerprint: what the owner compares, and marks Verified.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// The primary key's key id.
    pub fn key_id(&self) -> KeyId {
        KeyId::of(&self.key.primary_key.legacy_key_id())
    }

    /// The addresses of the User IDs the key certified itself (lower case), in its order.
    pub fn addresses(&self) -> &[String] {
        &self.addresses
    }

    /// Whether `address` (an address, or a From field's value) is one of the key's own.
    pub fn has_address(&self, address: &str) -> bool {
        let address = mime::address(address);
        !address.is_empty() && self.addresses.contains(&address)
    }

    /// When the primary key was made, in seconds since 1970.
    pub fn created(&self) -> u64 {
        u64::from(self.key.primary_key.created_at().as_secs())
    }

    /// The primary key's algorithm, by name ("RSA", "Ed25519", …).
    pub fn algorithm(&self) -> &'static str {
        algorithm_name(self.key.primary_key.algorithm())
    }

    /// Whether the key's owner revoked it.
    pub fn revoked(&self) -> bool {
        self.signing.first().is_some_and(|s| s.revoked)
    }

    /// When the primary key stops being valid, in seconds since 1970; `None` when never.
    pub fn expires(&self) -> Option<u64> {
        self.signing.first().and_then(|s| s.expires)
    }

    /// The key as binary OpenPGP packets, to be kept and read back with [`PublicKey::parse`].
    pub fn to_bytes(&self) -> Vec<u8> {
        self.key.to_bytes().unwrap_or_default()
    }

    pub(crate) fn inner(&self) -> &SignedPublicKey {
        &self.key
    }
}

/// How many signature checks reading one key may cost.
struct Budget(usize);

impl Budget {
    /// Whether one more check may be made, counting it.
    fn spend(&mut self) -> bool {
        let left = self.0 > 0;
        self.0 = self.0.saturating_sub(1);
        left
    }
}

/// Makes `sig` the `newest` when it is at least as new.
fn consider<'a>(sig: &'a Signature, newest: &mut Option<&'a Signature>) {
    if newest.is_none_or(|n| created(sig) >= created(n)) {
        *newest = Some(sig);
    }
}

/// A signature's creation time, 0 when it has none.
fn created(sig: &Signature) -> u32 {
    sig.created().map_or(0, |t| t.as_secs())
}

/// A self-signature with a hash that may still be trusted for one: everything but MD5 (SHA-1
/// self-signatures are still on many keys, and rPGP checks SHA-1 for collision attacks).
fn usable(sig: &Signature) -> bool {
    sig.hash_alg().is_some_and(|h| h != HashAlgorithm::Md5)
}

/// Whether `sig` names `key` as its issuer, or names nobody.
fn is_self(sig: &Signature, key: &impl KeyDetails) -> bool {
    let (ids, fps) = (sig.issuer_key_id(), sig.issuer_fingerprint());
    (ids.is_empty() && fps.is_empty()) || ids.iter().any(|id| **id == key.legacy_key_id()) || fps.iter().any(|fp| **fp == key.fingerprint())
}

/// The key flags a self-signature states, if it states any.
fn flags(sig: &Signature) -> Option<KeyFlags> {
    sig.config()?.hashed_subpackets.iter().find_map(|p| match &p.data {
        SubpacketData::KeyFlags(flags) => Some(flags.clone()),
        _ => None,
    })
}

/// When a key made at `created` stops being valid by self-signature `sig`.
fn expiry(created: u32, sig: &Signature) -> Option<u64> {
    let after = sig.key_expiration_time()?.as_secs();
    (after > 0).then(|| u64::from(created).saturating_add(u64::from(after)))
}

/// How many packet headers `bytes` holds, counted without reading them (RFC 9580 §4.2): a
/// cap on work for a block of many tiny packets.
fn count_packets(bytes: &[u8]) -> usize {
    let mut at = 0usize;
    let mut count = 0usize;
    while let Some(&tag) = bytes.get(at) {
        if tag & 0x80 == 0 || count > MAX_PACKETS {
            break;
        }
        count = count.saturating_add(1);
        let len = |o: usize| bytes.get(at.saturating_add(o)).copied().map_or(0usize, usize::from);
        let (header, body) = if tag & 0x40 != 0 {
            match len(1) {
                0..=191 => (2, len(1)),
                192..=223 => (3, ((len(1) - 192) << 8) + len(2) + 192),
                255 => (6, (len(2) << 24) | (len(3) << 16) | (len(4) << 8) | len(5)),
                // A partial length: the rest is one stream, counted as one.
                _ => break,
            }
        } else {
            match tag & 0x03 {
                0 => (2, len(1)),
                1 => (3, (len(1) << 8) | len(2)),
                2 => (5, (len(1) << 24) | (len(2) << 16) | (len(3) << 8) | len(4)),
                _ => break,
            }
        };
        at = at.saturating_add(header).saturating_add(body);
    }
    count
}

/// The Public keys the caller holds: by address, fingerprint and key id. Linear look-ups; the
/// caller decides how many keys it keeps.
#[derive(Clone, Debug, Default)]
pub struct KeyRing {
    keys: Vec<PublicKey>,
}

impl KeyRing {
    /// An empty ring.
    pub fn new() -> KeyRing {
        KeyRing::default()
    }

    /// Adds `key`; a key already here with the same fingerprint is replaced when `key` is the
    /// newer copy (its newest self-signature is later) and kept otherwise. Whether the ring
    /// changed.
    pub fn insert(&mut self, key: PublicKey) -> bool {
        match self.keys.iter_mut().find(|k| k.fingerprint == key.fingerprint) {
            // A copy without the revocation another one carries never undoes it.
            Some(old) if key.updated > old.updated && (key.revoked() || !old.revoked()) => {
                *old = key;
                true
            }
            Some(_) => false,
            None => {
                self.keys.push(key);
                true
            }
        }
    }

    /// Every key in the ring.
    pub fn keys(&self) -> &[PublicKey] {
        &self.keys
    }

    /// The keys that have `address` (an address, or a From field's value) as one of their own.
    pub fn for_address<'a>(&'a self, address: &str) -> impl Iterator<Item = &'a PublicKey> + 'a {
        let address = mime::address(address);
        self.keys.iter().filter(move |k| !address.is_empty() && k.addresses.contains(&address))
    }

    /// The key with primary fingerprint `fingerprint`.
    pub fn get(&self, fingerprint: &Fingerprint) -> Option<&PublicKey> {
        self.keys.iter().find(|k| &k.fingerprint == fingerprint)
    }

    /// How many keys the ring holds.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the ring holds no key.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The signing keys (primary or subkey) a signature names by fingerprint or key id.
    pub(crate) fn signers<'a>(&'a self, fps: &'a [Fingerprint], ids: &'a [KeyId]) -> impl Iterator<Item = (&'a PublicKey, &'a Signing)> + 'a {
        self.keys
            .iter()
            .flat_map(move |k| k.signing.iter().filter(move |s| fps.contains(&s.fingerprint) || ids.contains(&s.key_id)).map(move |s| (k, s)))
    }
}

impl FromIterator<PublicKey> for KeyRing {
    fn from_iter<I: IntoIterator<Item = PublicKey>>(iter: I) -> KeyRing {
        let mut ring = KeyRing::new();
        for key in iter {
            ring.insert(key);
        }
        ring
    }
}
