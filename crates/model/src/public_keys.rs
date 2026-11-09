// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Public keys kept per address (#30 R9), contact or not: S/MIME certificates collected from
//! Digitally signed mail that arrived (and, later, OpenPGP keys), for checking and, later,
//! encrypting to that address. Nothing is sent for them: what is kept came in a message. Kept in
//! the mailbox, capped per address and in all.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize};

use crate::digital_signature::SignatureFormat;
use mailcraft_time::Timestamp;

/// The most bytes of one key (an S/MIME certificate is a few KB).
pub const MAX_KEY_BYTES: usize = 16 * 1024;
/// The most keys kept for one address: past it, the one seen longest ago goes.
pub const MAX_KEYS_PER_ADDRESS: usize = 8;
/// The most addresses keys are kept for: past it, the address whose newest key was seen longest
/// ago goes.
pub const MAX_KEY_ADDRESSES: usize = 4_000;
/// The longest address a key is kept for.
const MAX_ADDRESS: usize = 254;

/// One Public key of an address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKey {
    pub format: SignatureFormat,
    /// The key as it came: an X.509 certificate in DER for S/MIME.
    #[serde(with = "crate::b64")]
    pub data: Arc<Vec<u8>>,
    /// Its fingerprint as [`crate::digital_signature::Signer::fingerprint`] has it: what tells
    /// two keys apart.
    pub fingerprint: String,
    /// Until when the certificate is valid; 0 when it doesn't say.
    #[serde(default)]
    pub not_after: Timestamp,
    /// When it was last seen in a message that came.
    #[serde(default)]
    pub seen: Timestamp,
}

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

    /// This set of keys' revision: another set, or this one once it has changed, has another.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Keep `key` for `address`: a key kept already (by fingerprint) is only seen again. Whether
    /// anything changed. A key too large, of no fingerprint, or for an empty or overlong address
    /// isn't kept.
    pub fn add(&mut self, address: &str, key: PublicKey) -> bool {
        let address = key_of(address);
        if address.is_empty() || address.len() > MAX_ADDRESS || !address.contains('@') || key.data.is_empty() || key.data.len() > MAX_KEY_BYTES {
            return false;
        }
        if key.fingerprint.is_empty() || key.fingerprint.len() > 256 {
            return false;
        }
        let keys = self.keys.entry(address).or_default();
        if let Some(kept) = keys.iter_mut().find(|k| k.fingerprint == key.fingerprint && k.format == key.format) {
            if kept.seen >= key.seen {
                return false;
            }
            kept.seen = key.seen;
        } else {
            keys.push(key);
        }
        keys.sort_by_key(|k| std::cmp::Reverse(k.seen));
        keys.truncate(MAX_KEYS_PER_ADDRESS);
        self.trim();
        self.revision = fresh_revision();
        true
    }

    /// Past [`MAX_KEY_ADDRESSES`], the addresses seen longest ago go.
    fn trim(&mut self) {
        while self.keys.len() > MAX_KEY_ADDRESSES {
            let oldest = self.keys.iter().min_by_key(|(_, keys)| keys.first().map_or(0, |k| k.seen)).map(|(a, _)| a.clone());
            match oldest {
                Some(a) => {
                    self.keys.remove(&a);
                }
                None => break,
            }
        }
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
                            keys.add(&address, key);
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
