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

use std::borrow::Cow;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::digital_signature::SignatureFormat;
use crate::message::Original;
use crate::{Address, Message};

/// The most recipients one message is encrypted to (the owner's own key among them).
pub const MAX_ENCRYPT_TO: usize = 200;
/// The most Bcc recipients a message is encrypted for: each is given a message of their own.
pub const MAX_BLIND: usize = 50;
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

/// The message one Bcc recipient is sent (#30 part 7): the content, encrypted to them and the
/// owner alone, so that nobody else's message names their key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlindCopy {
    /// Their address, in lower case.
    pub address: String,
    /// The Encrypted message's bytes, as [`Message::original`] holds them.
    pub original: Original,
}

/// Where the host hands back the Encrypted message a send went as. The message in the Outbox and
/// the copies of it a run is handed share one, so that when the send is taken in, the copy kept in
/// Sent Items is the Encrypted message and never the clear text it was made from. Equal to every
/// other (it is not part of what a message says), and never written.
#[derive(Clone, Default)]
pub struct Sealed(Arc<Mutex<Option<Original>>>);

impl Sealed {
    /// Hand back what the message went as.
    pub fn put(&self, original: Original) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(original);
        }
    }

    /// What was handed back, once.
    pub fn take(&self) -> Option<Original> {
        self.0.lock().ok().and_then(|mut slot| slot.take())
    }
}

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sealed")
    }
}

impl PartialEq for Sealed {
    fn eq(&self, _: &Sealed) -> bool {
        true
    }
}

impl Eq for Sealed {}

/// The Message-ID of the message the `at`th Bcc recipient is sent of their own: that of the message
/// with `-b{at+1}` before its `@` (or at its end). Empty stays empty.
fn blind_message_id(id: &str, at: usize) -> String {
    let id = id.trim();
    if id.is_empty() {
        return String::new();
    }
    match id.rsplit_once('@') {
        Some((local, host)) => format!("{local}-b{}@{host}", at.saturating_add(1)),
        None => format!("{id}-b{}", at.saturating_add(1)),
    }
}

/// The message a Bcc copy's Message-ID ([`blind_message_id`]) belongs to: its Message-ID without the
/// `-b{n}` before the `@`, in angle brackets as a server holds it. `None` for any other shape.
pub fn blind_copy_base_message_id(id: &str) -> Option<String> {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    let (local, host) = id.rsplit_once('@').map_or((id, None), |(l, h)| (l, Some(h)));
    let (base, n) = local.rsplit_once("-b")?;
    if base.is_empty() || n.is_empty() || n.len() > 10 || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(match host {
        Some(host) => format!("<{base}@{host}>"),
        None => format!("<{base}>"),
    })
}

/// A short tag of a draft's marker (the marker is made on one install, for one draft, and is not a
/// secret): 64-bit FNV-1a in hex. Two installs that save one draft at the same count don't share it.
pub fn marker_tag(marker: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in marker.bytes() {
        hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The Message-ID an Encrypted draft is written with for its `saves`th save: that of the message
/// with `-s{tag}n{saves}` before the `@` (`tag` from [`marker_tag`] of the install's own value and the draft's marker), so that the copies a server may
/// hold of it are told from any other draft that shares the message's own Message-ID (a conflict copy,
/// a copy made in Outlook) and from another install's: only this program writes one, and only such a
/// one is ever looked for by it.
pub fn sealed_draft_message_id(id: &str, install: &str, marker: &str, saves: u32) -> String {
    let id = id.trim();
    if id.is_empty() {
        return String::new();
    }
    // (`install`: a value this install keeps in its settings, never a secret.)
    let tag = marker_tag(&format!("{install}/{marker}"));
    match id.rsplit_once('@') {
        Some((local, host)) => format!("{local}-s{tag}n{saves}@{host}"),
        None => format!("{id}-s{tag}n{saves}"),
    }
}

/// Whether `id` (with or without its angle brackets) is one [`sealed_draft_message_id`] makes.
pub fn is_sealed_draft_message_id(id: &str) -> bool {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    let local = id.rsplit_once('@').map_or(id, |(local, _)| local);
    local.rsplit_once("-s").is_some_and(|(_, rest)| {
        rest.split_once('n').is_some_and(|(tag, n)| {
            tag.len() == 16 && tag.bytes().all(|b| b.is_ascii_hexdigit()) && !n.is_empty() && n.len() <= 10 && n.bytes().all(|b| b.is_ascii_digit())
        })
    })
}

impl Message {
    /// The Bcc recipients written to blind in an Encrypted message: those of the Bcc line who are
    /// not also on To or Cc (who are known to the others), in lower case, each once.
    pub fn blind_addresses(&self) -> Vec<String> {
        let mut out = self.blind_all();
        // (Those whose message of their own has gone already are not to be sent another.)
        out.retain(|a| !self.blind_sent.contains(a));
        out
    }

    /// Every blind recipient, those given their message already included.
    fn blind_all(&self) -> Vec<String> {
        let key = |a: &Address| a.email.trim().to_lowercase();
        let visible: Vec<String> = self.to.iter().chain(&self.cc).map(key).collect();
        let mut out: Vec<String> = Vec::new();
        for a in &self.bcc {
            let email = key(a);
            if !email.is_empty() && !visible.contains(&email) && !out.contains(&email) {
                out.push(email);
            }
        }
        out
    }

    /// Whether the message everybody but the Bcc recipients gets has anyone to go to (a message
    /// written to Bcc recipients alone has none: only their messages go).
    pub fn has_main_recipients(&self) -> bool {
        self.for_main().recipients().any(|a| !a.email.trim().is_empty())
    }

    /// The message that goes to everybody but the Bcc recipients, when each of them is given a
    /// message of their own ([`Message::blind_copies`]): the Bcc line is left out. Else as it is.
    pub fn for_main(&self) -> Cow<'_, Message> {
        if self.blind_copies.is_empty() && self.blind_sent.is_empty() {
            return Cow::Borrowed(self);
        }
        // (With messages of their own to send, the Bcc line goes whole. With none left to send, only
        // those who have had theirs: a message that goes unencrypted keeps the rest of its Bcc line.)
        let mut bcc = if self.blind_copies.is_empty() { self.bcc.clone() } else { Vec::new() };
        bcc.retain(|a| !self.blind_sent.contains(&a.email.trim().to_lowercase()));
        Cow::Owned(Message { bcc, blind_copies: Vec::new(), ..self.clone() })
    }

    /// The messages the Bcc recipients are each sent ([`Message::blind_copies`]): this message with
    /// the Encrypted bytes made for them, and nothing else of its recipients but them when `alone`
    /// (for a server that takes its recipients from the message's own header lines, as Microsoft
    /// Graph and Exchange do), else with its To and Cc as they are and them as its Bcc (for SMTP,
    /// whose envelope is the one address).
    pub fn blind_messages(&self, alone: bool) -> Vec<(String, Message)> {
        let own = self.encrypt_with.as_ref();
        // (Numbered by place among every Bcc recipient, those who have had theirs included, so that
        // after a send that stopped half way a copy never has the number of one that went.)
        let all = self.blind_all();
        self.blind_copies
            .iter()
            .enumerate()
            .map(|(place, copy)| {
                let at = all.iter().position(|a| *a == copy.address.trim().to_lowercase()).unwrap_or(all.len().saturating_add(place));
                let to = Address { name: String::new(), email: copy.address.clone() };
                let mut m = Message { original: Some(copy.original.clone()), blind_copies: Vec::new(), ..self.clone() };
                // Its own Message-ID: a server or a client that takes two messages with one for the
                // same is not to merge or drop one of them (and the message itself keeps its own).
                m.message_id = blind_message_id(&self.message_id, at);
                // (It is Encrypted to this recipient and the owner alone: what says so is its plan.)
                m.encrypt_with = own.map(|e| Encryption {
                    format: e.format,
                    recipients: e.recipients.iter().filter(|r| r.own || r.address == copy.address).cloned().collect(),
                    own_address: e.own_address.clone(),
                });
                if alone {
                    (m.to, m.cc, m.bcc) = (vec![to], Vec::new(), Vec::new());
                } else {
                    m.bcc = vec![to];
                }
                (copy.address.clone(), m)
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "encrypting_tests.rs"]
mod encrypting_tests;
