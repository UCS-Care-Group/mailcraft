// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The platform's credential store. Secrets never appear in an error's text or in `Debug` output.

use std::fmt;

/// The platform's credential store. Keys are `password:<address>` and `refresh:<address>`.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError>;
    fn delete(&self, key: &str);
    /// Why the store itself couldn't be used the last time something was asked of it (none is
    /// running, or it is locked), as opposed to there being nothing kept under a key. Taking it
    /// clears it. (`get` may wait for the owner to unlock the store: the window never calls it
    /// itself, only a background thread does.)
    fn take_trouble(&self) -> Option<StoreTrouble> {
        None
    }
}

/// Why a secret could not be kept. The text is ours and never carries the secret, a key or
/// anything a platform said (platform errors may echo what was being stored).
#[derive(Clone, PartialEq, Eq)]
pub enum SecretError {
    /// The store refused or failed to keep it.
    Refused,
    /// The secret is longer than the store can keep.
    TooLong,
    /// Some other reason, in our own words (a fixed phrase written here, never text the system
    /// returned, which may echo a secret); shown as it is.
    Other(&'static str),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Refused => f.write_str("the system's credential store could not keep this sign-in"),
            SecretError::TooLong => f.write_str("it is too long for the system's credential store"),
            SecretError::Other(why) => f.write_str(why),
        }
    }
}

impl fmt::Debug for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Refused => f.write_str("Refused"),
            SecretError::TooLong => f.write_str("TooLong"),
            SecretError::Other(_) => f.write_str("Other(..)"),
        }
    }
}

impl std::error::Error for SecretError {}

/// Why the system's credential store couldn't keep or give back a sign-in
/// ([`SecretStore::take_trouble`]). Then nothing is remembered between runs: the owner is asked
/// at each start, as on a system without a store, and is told why once a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreTrouble {
    /// No store answered: on Linux, no Secret Service (GNOME Keyring, KWallet) is running.
    Unavailable,
    /// The store is locked, or unlocking it was refused.
    Locked,
}

/// The name an account's secret of `kind` (`password` or `refresh`) is kept under in the credential
/// store: bound to the address and to the server it is for, so that a secret remembered for one
/// server is never handed to another.
pub fn secret_key(kind: &str, a: &mailcraft_model::Account) -> String {
    let address = a.email.trim().to_ascii_lowercase();
    let server = |s: &Option<mailcraft_model::Server>| match s {
        Some(s) => format!("{}:{}", s.host.trim().to_ascii_lowercase(), s.port),
        None => "none".to_string(),
    };
    if a.kind == mailcraft_model::AccountKind::Microsoft {
        format!("{kind}:{address}|graph")
    } else if a.kind == mailcraft_model::AccountKind::Jmap {
        // (Its whole session address: another one, on the same server or not, is another server;
        // and a bearer token is no password.)
        let (session, bearer) = a.jmap.as_ref().map_or(("none", false), |j| (j.session.trim(), j.bearer));
        format!("{kind}:{address}|jmap={session}|bearer={bearer}")
    } else if a.kind == mailcraft_model::AccountKind::Exchange {
        // (Its whole EWS address: another one, on the same server or not, is another server;
        // the user name it signs in with: a password is for one name; and the certificate it
        // trusts besides the system's, when it does: a password remembered for the server the
        // system vouches for is never handed to a connection that a pin lets through, AU3-02.)
        let url = a.ews.as_ref().map_or("none", |e| e.url.trim());
        let user = a.ews.as_ref().map_or_else(String::new, |e| e.username.trim().to_ascii_lowercase());
        match a.ews.as_ref().map(|e| e.trusted.trim().to_ascii_lowercase()).filter(|pin| !pin.is_empty()) {
            Some(pin) => format!("{kind}:{address}|ews={url}|user={user}|pin={pin}"),
            None => format!("{kind}:{address}|ews={url}|user={user}"),
        }
    } else {
        format!("{kind}:{address}|imap={}|smtp={}", server(&a.incoming), server(&a.outgoing))
    }
}
