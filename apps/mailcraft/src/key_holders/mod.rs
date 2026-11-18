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
//!   ([`usable`]: email protection, or no extended key usage at all; RSA of 2048 bits or more,
//!   P-256 or P-384), as [`HeldCertificate`], expired ones included and marked
//!   ([`HeldCertificate::in_date`]), the same on every system;
//! - `sign_digest(fingerprint, scheme, digest_algorithm, digest)` signs a SHA-256, SHA-384 or
//!   SHA-512 digest: RSA PKCS#1 v1.5, RSA-PSS (salt as long as the digest, MGF1 with the same
//!   hash; RFC 4056) or ECDSA on P-256 and P-384, whose signature comes back DER-encoded
//!   (`Ecdsa-Sig-Value`, RFC 5753) on every system, as CMS wants it. A certificate out of its
//!   dates, or not for mail, signs nothing ([`check_for_signing`]);
//! - `decrypt_key(fingerprint, padding, wrapped)` unwraps an RSA-encrypted content-encryption key
//!   (RSAES-PKCS1-v1_5 or RSAES-OAEP; RFC 8551), an expired certificate's key too, as old mail
//!   needs ([`check_for_decrypting`]). A caller must treat every failure here alike (CMS's
//!   random-key rule, RFC 3218), so that a bad padding is not told from a wrong key;
//! - importing a `.p12` into the system's store where there is one.
//!
//! PINs (R14): on Windows and macOS the system asks for a PIN or the keychain password itself,
//! in its own window, and MailCraft never sees it. A PKCS#11 token whose reader has a protected
//! path (a PIN pad) is asked on the reader; any other token needs the PIN the owner typed into
//! MailCraft's window, handed in as a [`Pin`] together with the token it was asked for: held in
//! memory, cleared when dropped, never logged, and shown by `Debug` as `Pin(…)`. Nothing here
//! keeps a PIN between calls.
//!
//! Nothing of this is in the browser build: `apps/mailcraft` is the desktop program alone.
//!
//! Until the signing and decrypting parts of #30 call it, only the tests do. Outside a test build
//! everything here is unused, which `expect(dead_code)` says (an `expect`, not an `allow`: it
//! fails once a caller arrives, and comes off then). A test build expects nothing, so the tests
//! must reach every item, but the Windows and macOS stores', which no test can reach and which
//! keep an `expect` of their own.
#![cfg_attr(not(test), expect(dead_code, reason = "called from the signing and decrypting parts of #30"))]

pub mod certificate;
pub mod encoding;
#[cfg(target_os = "macos")]
#[expect(dead_code, reason = "called from the signing and decrypting parts of #30; no test can reach the keychain")]
pub mod macos_keychain;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub mod pkcs11;
#[cfg(target_os = "windows")]
#[expect(dead_code, reason = "called from the signing and decrypting parts of #30; no test can reach the store")]
pub mod windows_store;

#[cfg(test)]
mod tests;

pub use certificate::{HeldCertificate, KeyKind};

use zeroize::Zeroizing;

/// The most certificates a listing gives.
pub const MAX_CERTIFICATES: usize = 256;
/// The most certificates a store or a token is looked through for them: more is damage, not an
/// owner's keys. Larger than [`MAX_CERTIFICATES`], so that a store full of other certificates
/// (web sites', roots) still shows the owner's mail ones.
pub const MAX_SCANNED: usize = 4096;
/// The largest certificate read (DER bytes). Real ones are a few kilobytes.
pub const MAX_CERTIFICATE_LEN: usize = 64 * 1024;
/// The largest `.p12` file imported.
pub const MAX_P12_LEN: usize = 1024 * 1024;
/// The largest wrapped key handed to a system to unwrap: an RSA-16384 block.
pub const MAX_WRAPPED_KEY_LEN: usize = 2048;
/// The smallest RSA key used (NIST SP 800-131A; RFC 8551 §2.4.1 makes 2048 the floor for signing).
pub const MIN_RSA_BITS: u32 = 2048;

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

/// The PKCS#11 token a PIN is asked for and given to: its slot, its serial number and its
/// label as the window names it. A PIN handed back goes to this token only, and only while the
/// slot still holds the token with this serial number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRef {
    pub slot: u64,
    pub serial: String,
    pub label: String,
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
    /// The certificate may not be used for this (not for mail, out of its dates, too weak).
    NotAllowed(&'static str),
    /// What was handed in can't be right (a digest of the wrong length, an oversized input).
    BadInput(&'static str),
    /// The token needs its PIN and none was given: the window asks the owner for it.
    PinNeeded(TokenRef),
    /// The slot no longer holds the token the PIN was asked for: nothing was sent to it.
    TokenChanged,
    /// The token refused the PIN.
    PinIncorrect,
    /// The token's PIN is locked after too many wrong tries.
    PinLocked,
    /// The token's PIN has expired and must be changed (with the token's own tool).
    PinExpired,
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
            Self::NotAllowed(why) => write!(f, "this certificate can't be used: {why}"),
            Self::BadInput(what) => write!(f, "bad input: {what}"),
            Self::PinNeeded(token) => write!(f, "the PIN of {} is needed", token.label),
            Self::TokenChanged => f.write_str("the token was taken out or changed"),
            Self::PinIncorrect => f.write_str("the PIN is not right"),
            Self::PinLocked => f.write_str("the PIN is locked"),
            Self::PinExpired => f.write_str("the PIN has expired and must be changed"),
            Self::Cancelled => f.write_str("cancelled"),
            Self::System(why) => write!(f, "the system said: {why}"),
        }
    }
}

impl std::error::Error for KeyHolderError {}

/// A key unwrapped by the system: overwritten when dropped.
pub type UnwrappedKey = Zeroizing<Vec<u8>>;

/// Seconds since 1970 now, or `None` when the clock can't say (then nothing is signed).
pub fn now() -> Option<u64> {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Whether a certificate is one a listing shows: for mail, able to sign or unwrap, and a key we
/// use (RSA of [`MIN_RSA_BITS`] or more, P-256, P-384).
fn listable(held: &HeldCertificate) -> bool {
    held.for_mail && (held.can_sign || held.can_decrypt) && strong_enough(held.key)
}

fn strong_enough(key: KeyKind) -> bool {
    match key {
        KeyKind::Rsa { bits } => bits >= MIN_RSA_BITS,
        KeyKind::EcP256 | KeyKind::EcP384 => true,
        KeyKind::Other => false,
    }
}

/// The certificates among `found` (DER, as the store gave them) that may be used for mail, each
/// once: at most [`MAX_SCANNED`] looked through, at most [`MAX_CERTIFICATES`] given; ones that
/// can't be read are left out. Expired ones are kept, marked as such for `now`.
pub fn usable<I>(found: I, now: Option<u64>) -> Vec<HeldCertificate>
where
    I: IntoIterator<Item = Vec<u8>>,
{
    let mut out: Vec<HeldCertificate> = Vec::new();
    for der in found.into_iter().take(MAX_SCANNED) {
        if out.len() >= MAX_CERTIFICATES {
            break;
        }
        let Ok(held) = HeldCertificate::read(der, now) else { continue };
        if listable(&held) && !out.iter().any(|h| h.fingerprint == held.fingerprint) {
            out.push(held);
        }
    }
    out
}

/// Whether `held` may sign `scheme` now: what a listing shows, able to sign, in its dates.
pub fn check_for_signing(held: &HeldCertificate, scheme: SignatureScheme) -> Result<(), KeyHolderError> {
    if !listable(held) {
        return Err(KeyHolderError::NotAllowed("not a certificate for mail"));
    }
    if !held.can_sign {
        return Err(KeyHolderError::NotAllowed("its key usage doesn't allow signing"));
    }
    if !held.in_date {
        return Err(KeyHolderError::NotAllowed("it is expired or not valid yet"));
    }
    if !scheme.fits(held.key) {
        return Err(KeyHolderError::Unsupported("this certificate's key doesn't sign that way"));
    }
    Ok(())
}

/// Whether `held` may unwrap a message key: what a listing shows, an RSA key allowed to; an
/// expired certificate's key unwraps the mail sent to it while it was valid.
pub fn check_for_decrypting(held: &HeldCertificate) -> Result<(), KeyHolderError> {
    if !listable(held) {
        return Err(KeyHolderError::NotAllowed("not a certificate for mail"));
    }
    if !held.can_decrypt {
        return Err(KeyHolderError::NotAllowed("its key usage doesn't allow unwrapping keys"));
    }
    Ok(())
}

/// The wrapped key, when its length can be a wrapped key's.
pub fn check_wrapped(wrapped: &[u8]) -> Result<&[u8], KeyHolderError> {
    if wrapped.is_empty() || wrapped.len() > MAX_WRAPPED_KEY_LEN { Err(KeyHolderError::BadInput("the wrapped key's length")) } else { Ok(wrapped) }
}

/// The `.p12` file, when its size can be one's.
pub fn check_p12(p12: &[u8]) -> Result<&[u8], KeyHolderError> {
    if p12.is_empty() || p12.len() > MAX_P12_LEN { Err(KeyHolderError::BadInput("the .p12 file's size")) } else { Ok(p12) }
}
