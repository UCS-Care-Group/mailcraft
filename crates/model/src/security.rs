//! Certificates and OpenPGP keys the mailbox keeps: the public halves only.
//!
//! A private key is never here: it lives in the operating system's credential store, or in memory
//! for the session. [`StoredKey::own`] only says that the owner has one for this key.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// S/MIME (X.509 certificates) or OpenPGP.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyScheme {
    #[default]
    Smime,
    OpenPgp,
}

impl KeyScheme {
    pub fn code(self) -> &'static str {
        match self {
            KeyScheme::Smime => "smime",
            KeyScheme::OpenPgp => "openpgp",
        }
    }

    pub fn parse(s: &str) -> Option<KeyScheme> {
        match s.trim().to_ascii_lowercase().as_str() {
            "smime" | "s/mime" | "x509" => Some(KeyScheme::Smime),
            "openpgp" | "pgp" | "gpg" => Some(KeyScheme::OpenPgp),
            _ => None,
        }
    }

    /// For people: `S/MIME`, `OpenPGP`.
    pub fn label(self) -> &'static str {
        match self {
            KeyScheme::Smime => "S/MIME",
            KeyScheme::OpenPgp => "OpenPGP",
        }
    }
}

/// A certificate or an OpenPGP public key, and what the owner says about it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StoredKey {
    pub scheme: KeyScheme,
    /// SHA-256 of the certificate, or the OpenPGP fingerprint: upper-case hex.
    pub fingerprint: String,
    pub name: String,
    /// The addresses it is for, lower-case.
    pub emails: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub issuer: String,
    /// Seconds since 1970.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_before: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_after: Option<i64>,
    /// A certificate authority's certificate.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_ca: bool,
    /// The owner vouches for it: an S/MIME certificate trusted as a root (signers it issued are
    /// verified), an OpenPGP key whose fingerprint the owner checked with its holder.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub trusted: bool,
    /// The owner's own: a private key goes with it (kept in the system's credential store or in
    /// memory, never here).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub own: bool,
    /// Taken from a signed message, not imported by the owner.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub collected: bool,
    /// The certificate (DER) or the transferable public key (binary OpenPGP).
    #[serde(with = "crate::b64")]
    pub data: Arc<Vec<u8>>,
}

impl StoredKey {
    pub fn is_for(&self, email: &str) -> bool {
        let email = email.trim().to_ascii_lowercase();
        !email.is_empty() && self.emails.contains(&email)
    }

    /// In force at `now` (seconds since 1970); a key that names no end is.
    pub fn in_force(&self, now: i64) -> bool {
        self.not_before.is_none_or(|t| t <= now) && self.not_after.is_none_or(|t| now <= t)
    }

    /// The fingerprint in groups of four, for reading aloud.
    pub fn grouped_fingerprint(&self) -> String {
        let chars: Vec<char> = self.fingerprint.chars().collect();
        chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
    }
}

/// Most keys a mailbox keeps.
pub const MAX_KEYS: usize = 5000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_answer() {
        let k = StoredKey {
            scheme: KeyScheme::OpenPgp,
            fingerprint: "ABCDEF0123".into(),
            emails: vec!["ada@example.org".into()],
            not_after: Some(100),
            data: Arc::new(vec![1, 2, 3]),
            ..Default::default()
        };
        assert!(k.is_for(" Ada@Example.org") && !k.is_for(""));
        assert!(k.in_force(100) && !k.in_force(101));
        assert_eq!(k.grouped_fingerprint(), "ABCD EF01 23");
        let json = serde_json::to_string(&k).unwrap();
        assert!(!json.contains("trusted") && json.contains("\"scheme\":\"openPgp\""), "{json}");
        assert_eq!(serde_json::from_str::<StoredKey>(&json).unwrap(), k);
        assert_eq!(KeyScheme::parse("PGP"), Some(KeyScheme::OpenPgp));
    }
}
