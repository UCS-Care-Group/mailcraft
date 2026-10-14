//! MailCraft signed and encrypted mail: the cryptography, on bytes.
//!
//! * **S/MIME** (RFC 8551): CMS `SignedData` and `EnvelopedData` (RFC 5652) over X.509
//!   certificates (RFC 5280). [`smime`] verifies and makes detached signatures, opens
//!   `application/pkcs7-mime` (enveloped or signed), and encrypts to certificates (RSA key
//!   transport, AES-256-CBC).
//! * **OpenPGP** (RFC 9580, 4880) as PGP/MIME (RFC 3156) and inline: [`openpgp`].
//! * **Keys**: certificates (`.cer`, `.crt`, `.pem`, DER or PEM), PKCS#12 files (`.p12`, `.pfx`,
//!   RFC 7292, read by [`p12`]) and OpenPGP keys (`.asc`, `.gpg`), read by [`import`].
//! * **Trust**: [`chain`] checks a signer's certificate up to a root the owner trusts. A
//!   signature that is mathematically right from a signer nobody vouches for is said to be
//!   exactly that ([`Trust::NotVerified`]).
//!
//! MIME is not done here (`mailcraft-mime` splits and wraps the parts); this crate takes the bytes
//! that are signed or encrypted and gives back bytes.
//!
//! Everything that comes in is hostile: a message, a key file, a certificate a message carries.
//! Nothing here panics on any bytes; sizes, nesting and the work a file can ask for (iterations
//! of a key derivation) are capped.
//!
//! Secret keys ([`SecretKey`]) are kept in memory only and never written by this crate; they show
//! nothing of themselves in `Debug` and their bytes are wiped when dropped.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod ber;
pub mod chain;
mod keys;
pub mod openpgp;
pub mod p12;
mod pem;
pub mod smime;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod tests;

pub use keys::{export_public, import, read_public, secret_belongs_to};

use zeroize::Zeroizing;

/// Largest file or message part this crate reads.
pub const MAX_INPUT: usize = 64 << 20;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// The bytes aren't what they say they are.
    #[error("it can't be read: {0}")]
    Malformed(String),
    /// A key that would be needed isn't here.
    #[error("{0}")]
    NoKey(String),
    /// The file needs its passphrase (or the one given is wrong).
    #[error("the passphrase is missing or wrong")]
    Passphrase,
    /// Something this program doesn't do.
    #[error("not supported: {0}")]
    Unsupported(String),
    /// Signing or encrypting went wrong.
    #[error("{0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, CryptoError>;

/// S/MIME or OpenPGP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Scheme {
    Smime,
    OpenPgp,
}

impl Scheme {
    pub fn code(self) -> &'static str {
        match self {
            Scheme::Smime => "smime",
            Scheme::OpenPgp => "openpgp",
        }
    }

    pub fn parse(s: &str) -> Option<Scheme> {
        match s.trim().to_ascii_lowercase().as_str() {
            "smime" | "s/mime" | "x509" => Some(Scheme::Smime),
            "openpgp" | "pgp" | "gpg" => Some(Scheme::OpenPgp),
            _ => None,
        }
    }
}

/// A certificate or an OpenPGP public key, and what it says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    pub scheme: Scheme,
    /// SHA-256 of the certificate (S/MIME) or the key's own fingerprint (OpenPGP), upper-case hex.
    pub fingerprint: String,
    /// Whom it belongs to: the certificate subject's common name, the key's first user id's name.
    pub name: String,
    /// The addresses it is for, lower-case.
    pub emails: Vec<String>,
    /// Who issued it (S/MIME): the issuer's common name.
    pub issuer: String,
    /// Seconds since 1970, when it is known.
    pub not_before: Option<i64>,
    pub not_after: Option<i64>,
    /// A certificate authority's certificate (S/MIME).
    pub is_ca: bool,
    /// Issued by itself (S/MIME).
    pub self_signed: bool,
    pub can_sign: bool,
    pub can_encrypt: bool,
    /// The certificate (DER) or the transferable public key (binary OpenPGP).
    pub data: Vec<u8>,
}

impl PublicKey {
    /// Whether it is for `email` (case does not matter).
    pub fn is_for(&self, email: &str) -> bool {
        let email = email.trim().to_ascii_lowercase();
        !email.is_empty() && self.emails.contains(&email)
    }

    /// Whether it is in force at `now` (seconds since 1970); a key that names no dates is.
    pub fn in_force(&self, now: i64) -> bool {
        self.not_before.is_none_or(|t| t <= now) && self.not_after.is_none_or(|t| now <= t)
    }
}

/// A private key, in memory only: PKCS#8 (DER) for S/MIME, an OpenPGP transferable secret key
/// without its passphrase (binary) for OpenPGP. The bytes are wiped when it is dropped and never
/// shown by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretKey {
    pub scheme: Scheme,
    /// The fingerprint of its public half ([`PublicKey::fingerprint`]).
    pub fingerprint: String,
    pub data: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretKey").field("scheme", &self.scheme).field("fingerprint", &self.fingerprint).field("data", &"…").finish()
    }
}

/// What a key file held.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub public: Vec<PublicKey>,
    pub secret: Vec<SecretKey>,
}

/// Whether a signature is right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Validity {
    /// The signature matches the content and the signer's key.
    Valid,
    /// It doesn't: the content was changed, or the signature was made with another key.
    Invalid(String),
    /// The key that made it isn't known here, nor carried by the message.
    UnknownSigner,
    /// It is made in a way this program doesn't check.
    Unsupported(String),
}

/// Whether anybody the owner trusts vouches for the signer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    /// The certificate goes up to a root the owner trusts (named), and every certificate on the way
    /// is in force and signed by the next.
    Verified(String),
    /// Nobody vouches for it, or the chain is broken (why).
    NotVerified(String),
}

/// What checking a signature found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureCheck {
    pub scheme: Scheme,
    pub validity: Validity,
    pub trust: Trust,
    /// The signer's certificate or key, when it was found.
    pub signer: Option<PublicKey>,
    /// When the signer says it signed (seconds since 1970).
    pub signed_at: Option<i64>,
    /// Certificates the message carries (S/MIME), for keeping when the owner wants that.
    pub carried: Vec<PublicKey>,
}

impl SignatureCheck {
    pub(crate) fn failed(scheme: Scheme, validity: Validity) -> SignatureCheck {
        SignatureCheck {
            scheme,
            validity,
            trust: Trust::NotVerified("the signature was not checked".into()),
            signer: None,
            signed_at: None,
            carried: Vec::new(),
        }
    }

    /// The addresses the signer's key is for.
    pub fn signer_emails(&self) -> &[String] {
        self.signer.as_ref().map(|k| k.emails.as_slice()).unwrap_or_default()
    }
}

/// Upper-case hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS.get(usize::from(b >> 4)).copied().unwrap_or(b'0')));
        s.push(char::from(DIGITS.get(usize::from(b & 0xf)).copied().unwrap_or(b'0')));
    }
    s
}

/// Random bytes from the system (the page's in the browser).
pub(crate) fn rng() -> rand::rngs::OsRng {
    rand::rngs::OsRng
}

/// Line ends as CRLF: the canonical form of a MIME entity that is signed (RFC 5751 §3.1.1).
pub fn canonical_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 32);
    let mut prev = 0u8;
    for &b in bytes {
        if b == b'\n' && prev != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        prev = b;
    }
    out
}

#[cfg(test)]
mod basics {
    use super::*;

    #[test]
    fn line_ends_become_crlf() {
        assert_eq!(canonical_crlf(b"a\nb\r\nc\n"), b"a\r\nb\r\nc\r\n");
        assert_eq!(hex(&[0x0a, 0xff]), "0AFF");
        let k = SecretKey { scheme: Scheme::Smime, fingerprint: "AB".into(), data: Zeroizing::new(vec![1, 2, 3]) };
        assert!(!format!("{k:?}").contains("[1, 2, 3]"));
    }
}
