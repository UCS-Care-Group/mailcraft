// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digital signatures made when a message is sent (#30 R16, R16a, R19).
//!
//! An account names the key it signs with ([`SigningKey`]: an S/MIME certificate or an OpenPGP
//! key, by its fingerprint, which is public; the private key stays where the system keeps it)
//! and whether every message from it is Digitally signed ([`AccountSigning::always`]). A message
//! being written follows that setting until the owner turns Sign on or off for it
//! ([`crate::Message::sign`]); what it will be is decided at Send, and the message waiting in the
//! Outbox carries the key it is to be signed with ([`crate::Message::sign_with`]). The host signs
//! it on its way out and nothing sends it unsigned: a message that couldn't be signed is held in
//! the Outbox with why ([`Unsigned`]).

use serde::{Deserialize, Serialize};

use crate::digital_signature::SignatureFormat;

/// The longest fingerprint kept (hex digits): an OpenPGP v6 fingerprint is 64, a SHA-256 one 64.
pub const MAX_FINGERPRINT: usize = 128;
/// The longest name of a key kept (characters).
pub const MAX_KEY_LABEL: usize = 200;

/// The key an account signs with: which of the owner's certificates or OpenPGP keys, by its
/// fingerprint. Nothing secret: the private key is where the system keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningKey {
    pub format: SignatureFormat,
    /// Its fingerprint in hex: an S/MIME certificate's SHA-256, an OpenPGP key's (the signing
    /// key's own, a subkey's when that is what signs).
    pub fingerprint: String,
    /// What the window calls it (the certificate's subject, the key's User ID).
    #[serde(default)]
    pub label: String,
}

impl SigningKey {
    /// A key as it may be kept: a fingerprint of hex digits only, of a fingerprint's length, in
    /// upper case; a name without control characters, shortened. `None` when the fingerprint
    /// can't be one.
    pub fn new(format: SignatureFormat, fingerprint: &str, label: &str) -> Option<SigningKey> {
        let fingerprint: String = fingerprint.chars().filter(|c| !c.is_whitespace() && *c != ':').collect::<String>().to_ascii_uppercase();
        let hex = fingerprint.len() >= 32 && fingerprint.len() <= MAX_FINGERPRINT && fingerprint.bytes().all(|b| b.is_ascii_hexdigit());
        let label: String = label.chars().filter(|c| !c.is_control()).take(MAX_KEY_LABEL).collect::<String>().trim().to_string();
        hex.then_some(SigningKey { format, fingerprint, label })
    }

    /// The same key as [`SigningKey::new`] would keep it, or `None` (one read from a file).
    pub fn checked(&self) -> Option<SigningKey> {
        SigningKey::new(self.format, &self.fingerprint, &self.label)
    }
}

/// How an account Digitally signs its mail (#30 R16).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountSigning {
    /// The key it signs with; `None` until the owner chooses one.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub key: Option<SigningKey>,
    /// Always add a Digital signature: every message from it is signed unless the owner turns
    /// Sign off for that message. Off by default.
    #[serde(skip_serializing_if = "std::ops::Not::not", deserialize_with = "crate::sync::lenient_or_default")]
    pub always: bool,
}

impl AccountSigning {
    pub fn is_default(&self) -> bool {
        self.key.is_none() && !self.always
    }

    /// Whether a message from this account is signed when the owner chose `choice` for it
    /// (`None`: they didn't, and the account's setting holds).
    pub fn signs(&self, choice: Option<bool>) -> bool {
        choice.unwrap_or(self.always)
    }
}

/// A message of the Outbox that was to be Digitally signed and couldn't be (the key isn't there,
/// the owner cancelled the PIN, the key holder refused): it wasn't sent, and is held in the
/// Outbox with why, until the owner sends it again.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Unsigned {
    pub message: crate::Id,
    /// Why, in plain words: no key material, no PIN, no address.
    pub why: String,
}

/// What a message waiting in the Outbox says when it is held for that ([`Unsigned`]).
pub fn held_unsigned(why: &str) -> String {
    let why: String = why.chars().filter(|c| !c.is_control()).take(300).collect();
    format!("It couldn't be Digitally signed: {why}. It wasn't sent; Send Again tries again.")
}
