// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! S/MIME private keys where the system keeps them (#30): the Windows certificate store, the
//! macOS keychain, and PKCS#11 tokens on Linux and FreeBSD. MailCraft never holds the private key
//! itself: it lists the owner's certificates, hands the system a digest to sign or a wrapped
//! content-encryption key to unwrap, and takes back the result.
//!
//! Each system is a module of its own with the same small shape, and no trait joins them (the
//! signing part of #30 unifies them with gpg-agent):
//!
//! - `certificates()` lists the certificates that have a private key and may be used for mail
//!   (email protection, or no extended key usage at all), as [`HeldCertificate`];
//! - `sign_digest(fingerprint, scheme, digest_algorithm, digest)` signs a SHA-256, SHA-384 or
//!   SHA-512 digest: RSA PKCS#1 v1.5, RSA-PSS (salt as long as the digest, MGF1 with the same
//!   hash; RFC 4056) or ECDSA on P-256 and P-384, whose signature comes back DER-encoded
//!   (`Ecdsa-Sig-Value`, RFC 5753) on every system, as CMS wants it;
//! - `decrypt_key(fingerprint, padding, wrapped)` unwraps an RSA-encrypted content-encryption key
//!   (RSAES-PKCS1-v1_5 or RSAES-OAEP; RFC 8551). A caller must treat every failure here alike
//!   (CMS's random-key rule, RFC 3218), so that a bad padding is not told from a wrong key;
//! - importing a `.p12` into the system's store where there is one.
//!
//! PINs (R14): on Windows and macOS the system asks for a PIN or the keychain password itself,
//! in its own window, and MailCraft never sees it. A PKCS#11 token whose reader has a protected
//! path (a PIN pad) is asked on the reader; any other token needs the PIN the owner typed into
//! MailCraft's window, handed in as a [`Pin`]: held in memory, cleared when dropped, never
//! logged, and shown by `Debug` as `Pin(…)`. Nothing here keeps a PIN between calls.
//!
//! Nothing of this is in the browser build: `apps/mailcraft` is the desktop program alone.
// Used from the signing and decrypting parts of #30; until then only the tests call some of it.
#![allow(dead_code)]

pub mod certificate;
pub mod encoding;
#[cfg(target_os = "macos")]
pub mod macos_keychain;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod pkcs11;
#[cfg(target_os = "windows")]
pub mod windows_store;

#[cfg(test)]
mod tests;

pub use certificate::{HeldCertificate, KeyKind};

use zeroize::Zeroizing;

/// The most certificates a store or a token is read for: more is damage, not an owner's keys.
pub const MAX_CERTIFICATES: usize = 256;
/// The largest certificate read (DER bytes). Real ones are a few kilobytes.
pub const MAX_CERTIFICATE_LEN: usize = 64 * 1024;
/// The largest `.p12` file imported.
pub const MAX_P12_LEN: usize = 1024 * 1024;
/// The largest wrapped key handed to a system to unwrap: an RSA-16384 block.
pub const MAX_WRAPPED_KEY_LEN: usize = 2048;

/// A PIN or passphrase the owner typed into MailCraft's window: in memory only, overwritten
/// when dropped, and never shown by `Debug` or `Display`. Deliberately not `Clone`.
pub struct Pin(Zeroizing<String>);

impl Pin {
    /// The owner's PIN, taken over (the `String` given is moved, not copied).
    pub fn new(pin: String) -> Self {
        Self(Zeroizing::new(pin))
    }

    /// The PIN's text, for handing to the system at once.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Pin(…)")
    }
}

/// The hash a digest was made with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestAlgorithm {
    Sha256,
    Sha384,
    Sha512,
}

impl DigestAlgorithm {
    /// How many bytes a digest of this hash has.
    pub fn digest_len(self) -> usize {
        match self {
            Self::Sha256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }

    /// `digest` if it is as long as this hash's digests, else an error.
    pub fn check(self, digest: &[u8]) -> Result<&[u8], KeyHolderError> {
        if digest.len() == self.digest_len() { Ok(digest) } else { Err(KeyHolderError::BadInput("the digest's length is not its hash's")) }
    }
}

/// How a digest is signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureScheme {
    /// RSASSA-PKCS1-v1_5 (RFC 8017 §8.2): what Outlook makes and every reader reads.
    RsaPkcs1v15,
    /// RSASSA-PSS (RFC 8017 §8.1), MGF1 with the digest's hash, salt as long as the digest.
    RsaPss,
    /// ECDSA (P-256 or P-384); the signature comes back as DER `Ecdsa-Sig-Value`.
    Ecdsa,
}

impl SignatureScheme {
    /// Whether this scheme works with a key of this kind.
    pub fn fits(self, key: KeyKind) -> bool {
        match self {
            Self::RsaPkcs1v15 | Self::RsaPss => matches!(key, KeyKind::Rsa { .. }),
            Self::Ecdsa => matches!(key, KeyKind::EcP256 | KeyKind::EcP384),
        }
    }
}

/// How a content-encryption key was wrapped with the owner's RSA public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RsaPadding {
    /// RSAES-PKCS1-v1_5 (RFC 8017 §7.2): what Outlook and most programs send.
    Pkcs1v15,
    /// RSAES-OAEP with SHA-1 and MGF1-SHA-1, an empty label (RFC 3560's default).
    OaepSha1,
    /// RSAES-OAEP with SHA-256 and MGF1-SHA-256, an empty label.
    OaepSha256,
}

/// What a key holder can go wrong with. None of the texts carries a PIN or a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyHolderError {
    /// The store or the PKCS#11 module can't be opened or loaded.
    Unavailable(String),
    /// No certificate with a private key has that fingerprint (any more).
    NotFound,
    /// The key or the system doesn't do what was asked.
    Unsupported(&'static str),
    /// What was handed in can't be right (a digest of the wrong length, an oversized input).
    BadInput(&'static str),
    /// The token needs its PIN and none was given: the window asks the owner for it.
    PinNeeded { token: String },
    /// The token refused the PIN.
    PinIncorrect,
    /// The token's PIN is locked after too many wrong tries.
    PinLocked,
    /// The owner closed the system's prompt.
    Cancelled,
    /// The system said no, in its own words.
    System(String),
}

impl std::fmt::Display for KeyHolderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "the key store can't be opened: {why}"),
            Self::NotFound => f.write_str("no certificate with a private key has that fingerprint"),
            Self::Unsupported(what) => write!(f, "not supported: {what}"),
            Self::BadInput(what) => write!(f, "bad input: {what}"),
            Self::PinNeeded { token } => write!(f, "the PIN of {token} is needed"),
            Self::PinIncorrect => f.write_str("the PIN is not right"),
            Self::PinLocked => f.write_str("the PIN is locked"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::System(why) => write!(f, "the system said: {why}"),
        }
    }
}

impl std::error::Error for KeyHolderError {}

/// A key unwrapped by the system: overwritten when dropped.
pub type UnwrappedKey = Zeroizing<Vec<u8>>;

/// The certificates among `found` (DER, as the store gave them) that may be used for mail, each
/// once, at most [`MAX_CERTIFICATES`]; ones that can't be read are left out.
pub fn usable<I>(found: I) -> Vec<HeldCertificate>
where
    I: IntoIterator<Item = Vec<u8>>,
{
    let mut out: Vec<HeldCertificate> = Vec::new();
    for der in found.into_iter().take(MAX_CERTIFICATES) {
        let Ok(held) = HeldCertificate::from_der(der) else { continue };
        if held.for_mail && (held.can_sign || held.can_decrypt) && !out.iter().any(|h| h.fingerprint == held.fingerprint) {
            out.push(held);
        }
    }
    out
}
