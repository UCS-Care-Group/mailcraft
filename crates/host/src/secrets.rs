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
