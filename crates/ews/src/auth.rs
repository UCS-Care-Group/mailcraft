// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Choosing how to sign in from what the server offers (#252, E4): the `WWW-Authenticate`
//! headers of a 401 (RFC 7235).
//!
//! NTLM is chosen first where both are offered, then Basic. A server that offers only Negotiate
//! (Kerberos through SPNEGO) is refused with a typed error that says so: it is not a wrong
//! password, and asking again would not help.

use std::fmt;

/// How the program signs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// NTLMv2 ([`crate::ntlm`]): three legs on one held connection.
    Ntlm,
    /// Basic (RFC 7617): one `Authorization` value ([`crate::Secret::basic`]).
    Basic,
}

/// Why no way of signing in can be chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthError {
    /// The server offers Negotiate (Kerberos) and nothing this program speaks.
    NegotiateOnly,
    /// The server offers nothing this program speaks (or nothing at all).
    NothingSupported,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NegotiateOnly => "the server only signs in with Kerberos (Negotiate), which this program does not speak yet",
            Self::NothingSupported => "the server offers no way of signing in that this program speaks (NTLM or Basic)",
        })
    }
}

impl std::error::Error for AuthError {}

/// The most challenges looked at in all the headers, and the most of one header read.
const MAX_CHALLENGES: usize = 32;
const MAX_VALUE: usize = 8 * 1024;

/// The challenges of the headers: each scheme in lower case, and what follows it (a token or
/// parameters), as written.
fn challenges(headers: &[String]) -> Vec<(String, &str)> {
    let mut found = Vec::new();
    for header in headers {
        let mut end = MAX_VALUE.min(header.len());
        while !header.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let header = header.get(..end).unwrap_or("");
        let mut start = 0usize;
        let mut quoted = false;
        let mut segments = Vec::new();
        for (at, c) in header.char_indices() {
            match c {
                '"' => quoted = !quoted,
                ',' if !quoted => {
                    segments.push(header.get(start..at).unwrap_or(""));
                    start = at.saturating_add(1);
                }
                _ => {}
            }
        }
        segments.push(header.get(start..).unwrap_or(""));
        for segment in segments {
            let segment = segment.trim();
            let (word, rest) = match segment.split_once(char::is_whitespace) {
                Some((word, rest)) => (word, rest.trim()),
                None => (segment, ""),
            };
            // `realm="x"` is a parameter of the challenge before it; a word without an equals
            // sign begins a new challenge (the equals signs of a Base64 token come after a space).
            if word.is_empty() || word.contains('=') {
                continue;
            }
            found.push((word.to_ascii_lowercase(), rest));
            if found.len() >= MAX_CHALLENGES {
                return found;
            }
        }
    }
    found
}

/// The way to sign in, from the `WWW-Authenticate` headers of a 401: NTLM when offered, else
/// Basic.
pub fn choose(headers: &[String]) -> Result<Scheme, AuthError> {
    let offered = challenges(headers);
    let has = |scheme: &str| offered.iter().any(|(name, _)| name == scheme);
    if has("ntlm") {
        Ok(Scheme::Ntlm)
    } else if has("basic") {
        Ok(Scheme::Basic)
    } else if has("negotiate") {
        Err(AuthError::NegotiateOnly)
    } else {
        Err(AuthError::NothingSupported)
    }
}

/// The Base64 token of the NTLM challenge the server sent (`NTLM TlRMTVNT…`), when there is one.
pub fn ntlm_token(headers: &[String]) -> Option<&str> {
    challenges(headers).into_iter().find(|(name, rest)| name == "ntlm" && !rest.is_empty()).map(|(_, rest)| rest)
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod auth_tests;
