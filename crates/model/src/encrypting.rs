// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Encrypting what the owner sends (#30 part 7: R16, R16a, R17, R18, R20, R21).
//!
//! An account says whether its mail is encrypted whenever every recipient has a Public key
//! ([`AccountEncryption::when_possible`]); a message being written follows that until the owner
//! turns Encrypt on or off for it ([`crate::Message::encrypt`]). What it will be is decided at
//! Send, and the message waiting in the Outbox carries what it is encrypted to
//! ([`crate::Message::encrypt_with`]): only Public keys, copied from the mailbox's key store
//! ([`crate::public_keys`]) when the owner pressed Send. The host encrypts it on its way out, to
//! those keys and to the owner's own, and nothing sends it in the clear: a message that couldn't
//! be encrypted is held in the Outbox with why ([`crate::signing::Unsigned`]).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::digital_signature::SignatureFormat;

/// The most recipients one message is encrypted to (the owner's own key among them).
pub const MAX_ENCRYPT_TO: usize = 200;
/// The longest address kept for a recipient.
const MAX_ADDRESS: usize = 254;

/// How an account encrypts its mail (#30 R16).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountEncryption {
    /// Encrypt when every recipient has a Public key: a message from the account is Encrypted
    /// when it can be, and goes as it is when some recipient has none (it is never stopped for
    /// that: Encrypt, set for a message, is what stops it, R18). Off by default.
    #[serde(skip_serializing_if = "std::ops::Not::not", deserialize_with = "crate::sync::lenient_or_default")]
    pub when_possible: bool,
}

impl AccountEncryption {
    pub fn is_default(&self) -> bool {
        !self.when_possible
    }
}

/// One Public key a message is encrypted to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptTo {
    /// The address it is for, in lower case.
    pub address: String,
    /// Its fingerprint as the key store has it ([`crate::public_keys::PublicKey::fingerprint`]).
    pub fingerprint: String,
    /// The key as the store keeps it: an X.509 certificate in DER for S/MIME, the transferable
    /// Public key as binary OpenPGP packets for OpenPGP.
    #[serde(with = "crate::b64")]
    pub key: Arc<Vec<u8>>,
    /// It is the owner's own (R17).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub own: bool,
}

/// What a message waiting in the Outbox is to be encrypted to, decided when the owner pressed
/// Send: a format, and a Public key for each recipient of that format. The owner's own key is
/// among them when the key store had one for their address; if it didn't, the host adds the
/// one it finds ([`Encryption::own_wanted`]), and holds the message when there is none: Sent Items
/// is always readable (R17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Encryption {
    pub format: SignatureFormat,
    pub recipients: Vec<EncryptTo>,
    /// The owner's address (the account's), whose key the host looks for when none of
    /// [`Encryption::recipients`] is `own`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub own_address: String,
}

impl Encryption {
    /// As it may be kept: at most [`MAX_ENCRYPT_TO`] keys, each with an address and a key that fits
    /// the key store's size. `None` when there is nothing to encrypt to or too much (one read
    /// from a file).
    pub fn checked(&self) -> Option<Encryption> {
        let fits = |r: &EncryptTo| {
            !r.address.is_empty()
                && r.address.len() <= MAX_ADDRESS
                && !r.address.chars().any(char::is_control)
                && !r.key.is_empty()
                && r.key.len() <= crate::public_keys::MAX_KEY_BYTES
        };
        (!self.recipients.is_empty() && self.recipients.len() <= MAX_ENCRYPT_TO && self.recipients.iter().all(fits)).then(|| self.clone())
    }

    /// Whether the host has to find the owner's own key.
    pub fn own_wanted(&self) -> bool {
        !self.recipients.iter().any(|r| r.own)
    }
}

/// What a message waiting in the Outbox says when it is held because it couldn't be Encrypted.
pub fn held_unencrypted(why: &str) -> String {
    let why: String = why.chars().filter(|c| !c.is_control()).take(300).collect();
    format!("It couldn't be Encrypted: {why}. It wasn't sent; Send Again tries again.")
}
