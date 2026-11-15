// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public keys kept per address (#30 R9), contact or not: S/MIME certificates and OpenPGP keys
//! collected from Digitally signed mail that arrived, and those the owner imported, for checking
//! and, later, encrypting to that address. Nothing is sent for them: what is kept came in a
//! message or a file. Kept in the mailbox, capped per address and in all; what the owner imported
//! or marked Verified never goes to make room before what was only collected.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize};

use crate::digital_signature::SignatureFormat;
use mailcraft_time::Timestamp;

/// The most bytes of one key (an S/MIME certificate is a few KB, an OpenPGP key as kept, with
/// its self-signatures only, as much).
pub const MAX_KEY_BYTES: usize = 16 * 1024;
/// The most keys kept for one address: past it, the one seen longest ago goes (one only
/// collected before any the owner imported or marked Verified).
pub const MAX_KEYS_PER_ADDRESS: usize = 8;
/// The most addresses keys are kept for: past it, the address whose newest key was seen longest
/// ago goes (one with nothing of the owner's first).
pub const MAX_KEY_ADDRESSES: usize = 4_000;
/// The longest address a key is kept for.
const MAX_ADDRESS: usize = 254;
/// The longest fingerprint kept.
const MAX_FINGERPRINT: usize = 256;

/// Where a kept key came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySource {
    /// From a Digitally signed message that arrived (#30 R9): learnt, nothing the owner did.
    #[default]
    Collected,
    /// Imported by the owner from a file (`keys.import`).
    Imported,
}

impl KeySource {
    /// Its name for an agent and a script: `collected`, `imported`.
    pub fn as_str(self) -> &'static str {
        match self {
            KeySource::Collected => "collected",
            KeySource::Imported => "imported",
        }
    }

    fn is_collected(&self) -> bool {
        *self == KeySource::Collected
    }
}

/// One Public key of an address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKey {
    pub format: SignatureFormat,
    /// The key as it came: an X.509 certificate in DER for S/MIME, the transferable Public key
    /// as binary OpenPGP packets for OpenPGP.
    #[serde(with = "crate::b64")]
    pub data: Arc<Vec<u8>>,
    /// Its fingerprint as [`crate::digital_signature::Signer::fingerprint`] has it: what tells
    /// two keys apart.
    pub fingerprint: String,
    /// Until when the certificate or key is valid; 0 when it doesn't say.
    #[serde(default)]
    pub not_after: Timestamp,
    /// When it was last seen in a message that came, or imported.
    #[serde(default)]
    pub seen: Timestamp,
    /// An OpenPGP key the owner marked Verified for this address after comparing its
    /// fingerprint (#30 R6). Only [`PublicKeys::set_verified`] sets it, which only the window
    /// calls: never a message, an import, a vCard or a command's params ([`PublicKeys::add`]
    /// keeps what is kept and never takes it from the key it is given). Never for S/MIME, whose
    /// trust is the certification authority's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub verified: bool,
    /// Where it came from.
    #[serde(default, skip_serializing_if = "KeySource::is_collected")]
    pub source: KeySource,
}

impl PublicKey {
    /// Whether it is the owner's to keep: Verified or imported. Such a key never goes to make
    /// room before one that was only collected.
    pub fn is_owners(&self) -> bool {
        self.verified || self.source == KeySource::Imported
    }

    fn is(&self, format: SignatureFormat, fingerprint: &str) -> bool {
        self.format == format && self.fingerprint == fingerprint
    }
}

/// A key as [`PublicKeys::differences`] names it: its address, format and fingerprint.
pub type KeyName = (String, SignatureFormat, String);

/// The Public keys of each address, by the address in lower case. Written as that map alone.
#[derive(Clone, Debug)]
pub struct PublicKeys {
    keys: BTreeMap<String, Vec<PublicKey>>,
    /// A number no other set of keys has had, taken anew at each change: what was worked out
    /// from the keys (a Digital signature's result) holds while it is the same.
    revision: u64,
}

/// The next [`PublicKeys::revision`].
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn fresh_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

impl Default for PublicKeys {
    fn default() -> Self {
        PublicKeys { keys: BTreeMap::new(), revision: fresh_revision() }
    }
}

impl PartialEq for PublicKeys {
    fn eq(&self, other: &Self) -> bool {
        self.keys == other.keys
    }
}

impl Eq for PublicKeys {}

impl Serialize for PublicKeys {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.keys.serialize(s)
    }
}

impl PublicKeys {
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// How many addresses have keys.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// The keys kept for `address`, newest seen first.
    pub fn of(&self, address: &str) -> &[PublicKey] {
        self.keys.get(&key_of(address)).map_or(&[], Vec::as_slice)
    }

    /// Every address with its keys, by address.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &[PublicKey])> {
        self.keys.iter().map(|(a, k)| (a.as_str(), k.as_slice()))
    }

    /// This set of keys' revision: another set, or this one once it has changed, has another.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Keep `key` for `address`. A key kept already (by format and fingerprint) is seen again:
    /// its bytes become those given when they differ (an OpenPGP key's newer copy, which the
    /// caller has merged with the kept one so that a revocation is never lost: the bytes given
    /// stand), an imported one stays imported and becomes so when `key` is, and whether the
    /// owner marked it Verified stays as it was. A new key is never Verified, whatever `key`
    /// says. Whether anything changed. A key too large, of no fingerprint, or for an empty or
    /// overlong address isn't kept.
    pub fn add(&mut self, address: &str, key: PublicKey) -> bool {
        self.insert(address, key, false)
    }

    /// [`PublicKeys::add`]; `from_file`: the key comes from the owner's own mailbox file, and
    /// keeps the mark they gave it.
    fn insert(&mut self, address: &str, mut key: PublicKey, from_file: bool) -> bool {
        let address = key_of(address);
        if address.is_empty() || address.len() > MAX_ADDRESS || !address.contains('@') || key.data.is_empty() || key.data.len() > MAX_KEY_BYTES {
            return false;
        }
        if key.fingerprint.is_empty() || key.fingerprint.len() > MAX_FINGERPRINT {
            return false;
        }
        if !from_file || key.format != SignatureFormat::OpenPgp {
            key.verified = false;
        }
        let keys = self.keys.entry(address.clone()).or_default();
        let before = keys.clone();
        if let Some(kept) = keys.iter_mut().find(|k| k.is(key.format, &key.fingerprint)) {
            if kept.data != key.data {
                kept.data = key.data;
                kept.not_after = key.not_after;
            }
            kept.seen = kept.seen.max(key.seen);
            if key.source == KeySource::Imported {
                kept.source = KeySource::Imported;
            }
        } else {
            keys.push(key);
        }
        keys.sort_by_key(|k| std::cmp::Reverse(k.seen));
        make_room(keys);
        let changed = *keys != before;
        if keys.is_empty() {
            self.keys.remove(&address);
        }
        if changed {
            self.trim();
            self.revision = fresh_revision();
        }
        changed
    }

    /// Take away the key `fingerprint` (of either format) kept for `address`. Whether there was
    /// one.
    pub fn remove(&mut self, address: &str, fingerprint: &str) -> bool {
        let address = key_of(address);
        let Some(keys) = self.keys.get_mut(&address) else { return false };
        let had = keys.len();
        keys.retain(|k| k.fingerprint != fingerprint);
        let removed = keys.len() != had;
        if keys.is_empty() {
            self.keys.remove(&address);
        }
        if removed {
            self.revision = fresh_revision();
        }
        removed
    }

    /// Mark the OpenPGP key `fingerprint` kept for `address` Verified, or not (#30 R6): for the
    /// owner's own word in the window only. Whether it changed; never for an S/MIME key.
    pub fn set_verified(&mut self, address: &str, fingerprint: &str, verified: bool) -> bool {
        let Some(key) = self.keys.get_mut(&key_of(address)).and_then(|keys| keys.iter_mut().find(|k| k.fingerprint == fingerprint)) else {
            return false;
        };
        if key.format != SignatureFormat::OpenPgp || key.verified == verified {
            return false;
        }
        key.verified = verified;
        self.revision = fresh_revision();
        true
    }

    /// The keys this set and `other` differ in (one has it and the other hasn't, or they hold it
    /// otherwise): what a step of the history changed of them.
    pub fn differences(&self, other: &PublicKeys) -> Vec<KeyName> {
        let mut out = Vec::new();
        if self.revision == other.revision {
            return out;
        }
        let addresses: BTreeSet<&String> = self.keys.keys().chain(other.keys.keys()).collect();
        for address in addresses {
            let (mine, theirs) = (self.of(address), other.of(address));
            for k in mine {
                if !theirs.iter().any(|o| o == k) {
                    out.push((address.clone(), k.format, k.fingerprint.clone()));
                }
            }
            for k in theirs {
                if !mine.iter().any(|o| o.is(k.format, &k.fingerprint)) {
                    out.push((address.clone(), k.format, k.fingerprint.clone()));
                }
            }
        }
        out
    }

    /// The keys `names` as `from` holds them, every other key as it is (an undo puts back what a
    /// step changed of them): one `from` lacks goes, one `from` has comes back (never Verified:
    /// only the owner's word in the window marks a key), and one both have keeps the bytes it
    /// has now (a newer copy may carry a revocation, which an undo never takes away), with where
    /// it came from as `from` has it, and Verified only when both say so.
    pub fn put_back(&mut self, from: &PublicKeys, names: &[KeyName]) {
        let mut changed = false;
        for (address, format, fingerprint) in names {
            let target = from.of(address).iter().find(|k| k.is(*format, fingerprint));
            let current = self.keys.get_mut(address).and_then(|keys| keys.iter_mut().find(|k| k.is(*format, fingerprint)));
            match (target, current) {
                (Some(t), Some(c)) => {
                    let verified = c.verified && t.verified;
                    if c.source != t.source || c.verified != verified {
                        c.source = t.source;
                        c.verified = verified;
                        changed = true;
                    }
                }
                (Some(t), None) => {
                    changed |= self.insert(address, PublicKey { verified: false, ..t.clone() }, false);
                }
                (None, Some(_)) => {
                    if let Some(keys) = self.keys.get_mut(address) {
                        keys.retain(|k| !k.is(*format, fingerprint));
                        if keys.is_empty() {
                            self.keys.remove(address);
                        }
                    }
                    changed = true;
                }
                (None, None) => {}
            }
        }
        if changed {
            self.revision = fresh_revision();
        }
    }

    /// Past [`MAX_KEY_ADDRESSES`], an address goes: of those with no key that is the owner's
    /// ([`PublicKey::is_owners`]), the one seen longest ago; only when every address has one,
    /// the one seen longest ago of all.
    fn trim(&mut self) {
        let newest = |keys: &Vec<PublicKey>| keys.first().map_or(0, |k| k.seen);
        while self.keys.len() > MAX_KEY_ADDRESSES {
            let oldest = self
                .keys
                .iter()
                .filter(|(_, keys)| !keys.iter().any(PublicKey::is_owners))
                .min_by_key(|(_, keys)| newest(keys))
                .or_else(|| self.keys.iter().min_by_key(|(_, keys)| newest(keys)))
                .map(|(a, _)| a.clone());
            match oldest {
                Some(a) => {
                    self.keys.remove(&a);
                }
                None => break,
            }
        }
    }
}

/// Past [`MAX_KEYS_PER_ADDRESS`] (`keys` newest seen first), the key seen longest ago goes: one
/// only collected before any that is the owner's.
fn make_room(keys: &mut Vec<PublicKey>) {
    while keys.len() > MAX_KEYS_PER_ADDRESS {
        let victim = keys.iter().rposition(|k| !k.is_owners()).unwrap_or(keys.len().saturating_sub(1));
        keys.remove(victim);
    }
}

fn key_of(address: &str) -> String {
    address.trim().to_lowercase()
}

impl<'de> Deserialize<'de> for PublicKeys {
    /// As a file holds them, with the caps of [`PublicKeys::add`]: what is past them is left out.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = PublicKeys;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a map of addresses to Public keys")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<PublicKeys, A::Error> {
                let mut keys = PublicKeys::default();
                let mut read = 0usize;
                while let Some(address) = map.next_key::<String>()? {
                    read = read.saturating_add(1);
                    if read > MAX_KEY_ADDRESSES {
                        map.next_value::<serde::de::IgnoredAny>()?;
                        continue;
                    }
                    // (A key that isn't one is left out, not the mailbox.)
                    let serde_json::Value::Array(list) = map.next_value::<serde_json::Value>()? else { continue };
                    for entry in list.iter().take(MAX_KEYS_PER_ADDRESS) {
                        if let Ok(key) = crate::load_limit::from_value::<PublicKey>(entry) {
                            // (The owner's own file: what they marked Verified stays so.)
                            keys.insert(&address, key, true);
                        }
                    }
                }
                Ok(keys)
            }
            // What isn't a map in their place (a file from elsewhere, or damaged) is no keys: the
            // mailbox still opens.
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<PublicKeys, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(PublicKeys::default())
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<PublicKeys, E> {
                Ok(PublicKeys::default())
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<PublicKeys, E> {
                Ok(PublicKeys::default())
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<PublicKeys, E> {
                Ok(PublicKeys::default())
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<PublicKeys, E> {
                Ok(PublicKeys::default())
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<PublicKeys, E> {
                Ok(PublicKeys::default())
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<PublicKeys, E> {
                Ok(PublicKeys::default())
            }
        }
        d.deserialize_any(V)
    }
}
