// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Basic sign-in (RFC 7617): the `Authorization` value for a user name and a password.
//!
//! The value is made once and held as a [`Secret`], whose `Debug` shows nothing. An error
//! here says what is wrong with the user name and never holds the user name or the password.
//! Basic is only ever sent inside TLS (every address a request goes to is checked for it).

use std::fmt;

use crate::http::Secret;

/// Longest user name and password taken, in bytes: far past any directory's limit.
const MAX_PART: usize = 1_024;

/// Why a user name and a password make no Basic credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    /// No user name.
    NoUserName,
    /// A user name with a colon in it: Basic cannot carry one (the colon ends the name).
    ColonInUserName,
    /// A user name or a password with a control character, or too long to be one.
    Unusable,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoUserName => "the user name is empty",
            Self::ColonInUserName => "a user name cannot have a colon in it; use DOMAIN\\user or user@example.org",
            Self::Unusable => "the user name or the password has a character that cannot be sent, or is too long",
        })
    }
}

impl std::error::Error for CredentialError {}

impl Secret {
    /// The `Authorization: Basic` value for `username` (`DOMAIN\user` or a UPN, as the server
    /// knows it) and `password`: `Basic ` and the Base64 of `username:password` in UTF-8.
    pub fn basic(username: &str, password: &str) -> Result<Secret, CredentialError> {
        if username.is_empty() {
            return Err(CredentialError::NoUserName);
        }
        if username.contains(':') {
            return Err(CredentialError::ColonInUserName);
        }
        let unusable = |text: &str| text.len() > MAX_PART || text.chars().any(char::is_control);
        if unusable(username) || unusable(password) {
            return Err(CredentialError::Unusable);
        }
        let mut pair = String::with_capacity(username.len().saturating_add(password.len()).saturating_add(1));
        pair.push_str(username);
        pair.push(':');
        pair.push_str(password);
        Ok(Secret::new(&format!("Basic {}", base64(pair.as_bytes()))))
    }
}

/// Standard Base64 with padding (RFC 4648).
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let at = |n: usize| u32::from(chunk.get(n).copied().unwrap_or(0));
        let group = (at(0) << 16) | (at(1) << 8) | at(2);
        let mut push = |shift: u32, present: bool| {
            // (A sextet is six bits, so the index is below 64.)
            let sextet = usize::try_from((group >> shift) & 63).unwrap_or(0);
            out.push(if present { char::from(ALPHABET.get(sextet).copied().unwrap_or(b'=')) } else { '=' });
        };
        push(18, true);
        push(12, true);
        push(6, chunk.len() > 1);
        push(0, chunk.len() > 2);
    }
    out
}

#[cfg(test)]
#[path = "basic_tests.rs"]
mod basic_tests;
