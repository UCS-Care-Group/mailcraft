// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The owner's own keys, as the host finds and keeps them (#30 R13): what File › Options ›
//! Security lists under Your certificates and keys, and what an import hands the host.
//!
//! The window never touches a private key. It asks the host ([`crate::Services::own_keys`]) to
//! list the owner's certificates and keys, to read which OpenPGP keys the owner's GnuPG trusts
//! fully, and to import a `.p12` or an `.asc` file the owner picked; the host does each on a
//! thread of its own and hands back public facts only. A passphrase typed for an import is held
//! in a [`Passphrase`] for that one call (overwritten when it is let go of, never shown by
//! `Debug`), and the file's bytes likewise; neither is kept, logged or put in a command's params.
//!
//! What MailCraft keeps of an imported key in the window's settings (`ui.json`, which is this
//! computer's, as the credential store holding the key is) is [`ImportedKey`]: its format,
//! fingerprint, name, addresses, end date and the name it is kept under in the credential store,
//! never the key or a passphrase. The browser build has no such host: nothing of this is offered
//! there, and checking Digital signatures works as everywhere.

use mailcraft_model::digital_signature::SignatureFormat;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// The most of the owner's keys a listing gives.
pub const MAX_LISTED: usize = 256;
/// The most imported keys the window's settings name.
pub const MAX_IMPORTED: usize = 64;
/// The largest private key file the window reads to import.
pub const MAX_FILE: usize = 1 << 20;
/// The longest name, address or fingerprint kept of a key, in characters.
pub const MAX_TEXT: usize = 256;
/// The most addresses kept of a key.
pub const MAX_ADDRESSES: usize = 16;

/// Where one of the owner's keys is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyPlace {
    /// Windows' certificate store (Current User › Personal).
    WindowsStore,
    /// The macOS keychain.
    Keychain,
    /// A PKCS#11 token (a smartcard), by its label.
    Token(String),
    /// The owner's GnuPG keyring, or a card GnuPG reads.
    Gnupg,
    /// Imported here, kept in the system's credential store.
    Imported,
}

/// One of the owner's keys, as the host lists it: public facts alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnKey {
    pub format: SignatureFormat,
    /// The name in the certificate or the key's first User ID (cut short, cleaned).
    pub name: String,
    pub addresses: Vec<String>,
    /// The certificate's SHA-256 (hexadecimal pairs with colons), or the OpenPGP fingerprint
    /// grouped in fours.
    pub fingerprint: String,
    /// Valid until, seconds since 1970; 0 when it doesn't end.
    pub not_after: i64,
    /// Out of its dates (or revoked) when listed.
    pub expired: bool,
    pub place: KeyPlace,
    /// Whether it may make Digital signatures.
    pub can_sign: bool,
}

/// Whether GnuPG was found on this computer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Gnupg {
    /// Not looked for yet.
    #[default]
    Unknown,
    /// Found and answering.
    Found,
    /// Not installed: OpenPGP signing works only with a key imported here.
    Missing,
    /// Installed but not working, in words.
    NotWorking(String),
}

/// Where this system keeps S/MIME private keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SmimePlace {
    /// Windows' certificate store: an import goes there.
    WindowsStore,
    /// The macOS keychain: an import goes there.
    Keychain,
    /// PKCS#11 tokens (Linux, FreeBSD): an import is kept in the credential store.
    Tokens,
    /// None this program knows.
    #[default]
    None,
}

/// What the host found: the owner's keys in the system's places (imported ones are the window's
/// own list), whether GnuPG is there, and what couldn't be looked at, in words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub keys: Vec<OwnKey>,
    pub gnupg: Gnupg,
    pub smime: SmimePlace,
    /// What couldn't be read (a token that didn't answer), each in a line of words.
    pub trouble: Vec<String>,
}

/// An imported key's public facts, as the window's settings keep them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedKey {
    pub format: SignatureFormat,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub not_after: i64,
    /// The name it is kept under in the credential store ([`is_store_name`]).
    #[serde(default)]
    pub store_key: String,
    #[serde(default)]
    pub can_sign: bool,
}

impl ImportedKey {
    /// The facts as they may be kept: names that are a key's store names only, text cut short,
    /// addresses capped. `None` for an entry that names anything else in the store, which the
    /// window never touches.
    pub fn sane(mut self) -> Option<ImportedKey> {
        if !is_store_name(&self.store_key) {
            return None;
        }
        self.fingerprint = clip(&self.fingerprint);
        self.name = clip(&self.name);
        self.addresses.truncate(MAX_ADDRESSES);
        for a in &mut self.addresses {
            *a = clip(a);
        }
        Some(self)
    }
}

/// Text cut to [`MAX_TEXT`] characters, without control characters.
pub fn clip(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect()
}

/// The credential store's name for an imported key: `key:smime:` and the certificate's SHA-256,
/// or `key:openpgp:` and the key's fingerprint, in lower-case hexadecimal.
pub fn store_name(format: SignatureFormat, fingerprint_hex: &str) -> String {
    let hex: String = fingerprint_hex.chars().filter(char::is_ascii_hexdigit).map(|c| c.to_ascii_lowercase()).collect();
    match format {
        SignatureFormat::Smime => format!("key:smime:{hex}"),
        SignatureFormat::OpenPgp => format!("key:openpgp:{hex}"),
    }
}

/// Whether `name` is a key's name in the credential store, as [`store_name`] makes it: only
/// such an entry is ever removed for an imported key.
pub fn is_store_name(name: &str) -> bool {
    let hex = name
        .strip_prefix("key:smime:")
        .filter(|h| h.len() == 64)
        .or_else(|| name.strip_prefix("key:openpgp:").filter(|h| matches!(h.len(), 40 | 64)));
    hex.is_some_and(|h| h.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)))
}

/// A passphrase the owner typed for an import: in memory for the one call, overwritten when let
/// go of, never shown by `Debug`. Not `Clone`.
pub struct Passphrase(Zeroizing<String>);

impl Passphrase {
    /// The passphrase, taken over (the `String` is moved, not copied).
    pub fn new(text: String) -> Passphrase {
        Passphrase(Zeroizing::new(text))
    }

    /// Its text, to hand on at once.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passphrase(…)")
    }
}

/// A file of the owner's to import, read by the window: overwritten when let go of.
pub struct KeyFile(pub Zeroizing<Vec<u8>>);

impl std::fmt::Debug for KeyFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KeyFile({} bytes)", self.0.len())
    }
}

/// What the window asks the host.
#[derive(Debug)]
pub enum OwnKeysJob {
    /// List the owner's keys in the system's places, and whether GnuPG is there.
    List,
    /// The (fingerprint in hexadecimal, address) pairs whose User ID the owner's GnuPG keyring
    /// holds at full or ultimate validity (R6), and nothing else of the keyring.
    GnupgValid,
    /// Import a `.p12`/`.pfx` (S/MIME, with its passphrase) or an `.asc`/`.gpg` secret key
    /// (OpenPGP: it is only read, so no passphrase).
    Import { format: SignatureFormat, file: KeyFile, passphrase: Option<Passphrase> },
}

/// Where an import went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Imported {
    /// Into the system's own store (Windows, macOS): the system holds it, and lists it.
    System,
    /// Kept by MailCraft in the credential store; for an OpenPGP key, its public half too
    /// (binary packets), to keep for the owner's own addresses so that their own Digital
    /// signatures check.
    Kept { key: ImportedKey, public: Option<Vec<u8>> },
}

/// What the host answers.
#[derive(Debug)]
pub enum OwnKeysAnswer {
    Listed(Listing),
    /// The pairs, or why GnuPG couldn't say (not installed: an empty list).
    GnupgValid(Result<Vec<(String, String)>, String>),
    /// What came of an import, or why nothing was kept, in words that name no secret.
    Imported(Result<Imported, String>),
}
