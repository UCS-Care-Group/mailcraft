// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! SASL OAUTHBEARER (RFC 7628): an OAuth 2.0 bearer token in place of a password, the standard
//! way, for IMAP (RFC 3501 AUTHENTICATE) and SMTP (RFC 4954 AUTH).
//!
//! What is sent is the GS2 header (RFC 5801) naming the user, then the server's host and port and
//! the token, each after the byte 0x01: `n,a=<user>,^Ahost=<host>^Aport=<port>^Aauth=Bearer
//! <token>^A^A`. A server that refuses sends its reasons first, as JSON in base64 (§3.2.2); the
//! client answers them with the single byte 0x01 (§3.2.3), and the refusal follows.
//!
//! The reasons are written by the server and read as hostile: capped, three fields only, each
//! short, with no control characters, and none of them ever shown as it came.

use crate::ProtoError;
use mailcraft_charset::base64::STANDARD;
use mailcraft_i18n::said;

/// Longest user name and token together.
const MAX_PAYLOAD: usize = 32 * 1024;
/// Longest host name sent.
const MAX_HOST: usize = 255;
/// Longest reasons read, before and after base64.
pub(crate) const MAX_FAILURE: usize = 4 * 1024;
/// Longest value of one of the reasons kept.
const MAX_FIELD: usize = 512;

/// What a client answers a server's reasons with: the byte 0x01, in base64 (RFC 7628 §3.2.3).
pub(crate) const FAILURE_ANSWER: &str = "AQ==";

/// What SASL OAUTHBEARER sends, in base64. `host` and `port` are the server's, as connected to.
/// A name, host or token that would break out of the message or the line it goes on is refused.
pub(crate) fn oauthbearer(user: &str, host: &str, port: u16, token: &str) -> Result<String, ProtoError> {
    if token.is_empty() {
        return Err(ProtoError::Auth(said!("there is no access token to sign in with")));
    }
    let unsendable = |text: &str| text.bytes().any(|b| matches!(b, 0 | 1 | b'\r' | b'\n'));
    if unsendable(user) || unsendable(token) || unsendable(host) {
        return Err(ProtoError::Auth(said!("the user name or access token has a character that can't be sent")));
    }
    // RFC 6750's token characters: what a bearer token is made of, and nothing that ends the field.
    if !token.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/' | b'=')) {
        return Err(ProtoError::Auth(said!("the user name or access token has a character that can't be sent")));
    }
    if host.len() > MAX_HOST || host.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control()) {
        return Err(ProtoError::Auth(said!("the mail server's name can't be sent with a sign-in")));
    }
    if user.len().saturating_add(token.len()) > MAX_PAYLOAD {
        return Err(ProtoError::Auth(said!("the user name or access token is too long to be sent")));
    }
    let user = saslname(user);
    let host_field = if host.is_empty() { String::new() } else { format!("host={host}\x01") };
    let port_field = if port == 0 { String::new() } else { format!("port={port}\x01") };
    Ok(STANDARD.encode(format!("n,a={user},\x01{host_field}{port_field}auth=Bearer {token}\x01\x01")))
}

/// A name as RFC 5801 writes it in the GS2 header: `,` as `=2C` and `=` as `=3D`.
fn saslname(user: &str) -> String {
    let mut out = String::with_capacity(user.len());
    for c in user.chars() {
        match c {
            ',' => out.push_str("=2C"),
            '=' => out.push_str("=3D"),
            c => out.push(c),
        }
    }
    out
}

/// Why a server refused an OAUTHBEARER (or XOAUTH2) sign-in, as far as it can be read: the three
/// fields of RFC 7628 §3.2.2, each checked and short. Nothing else of what it sent is kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SaslFailure {
    /// `invalid_token`, `insufficient_scope` or another error code of RFC 6750 §3.1: letters,
    /// digits and `_` only, else empty.
    pub status: String,
    /// The scopes the server says are needed: the characters RFC 6749 §3.3 allows, else empty.
    pub scope: String,
    /// Where the server says its identity provider describes itself: an `https://` address,
    /// else empty. Written by the server: never used without the owner's yes.
    pub openid_configuration: String,
}

impl SaslFailure {
    /// The reasons in a server's challenge, given as it came (base64 text, with or without the
    /// `+ ` or `334 ` before it taken off). `None` when it isn't base64 of a JSON object, or is
    /// longer than reasons are.
    pub fn read(challenge: &[u8]) -> Option<SaslFailure> {
        if challenge.len() > MAX_FAILURE.saturating_mul(2) {
            return None;
        }
        let text = std::str::from_utf8(challenge).ok()?.trim();
        let bytes = STANDARD.decode(text).ok()?;
        if bytes.len() > MAX_FAILURE {
            return None;
        }
        let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let object = json.as_object()?;
        let field = |name: &str| object.get(name).and_then(serde_json::Value::as_str).filter(|v| v.len() <= MAX_FIELD).unwrap_or("");
        let status = field("status");
        let status = if !status.is_empty() && status.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') { status } else { "" };
        let scope = field("scope");
        let scope_char = |b: u8| b == b' ' || b == 0x21 || (0x23..=0x5B).contains(&b) || (0x5D..=0x7E).contains(&b);
        let scope = if scope.bytes().all(scope_char) { scope.trim() } else { "" };
        let config = field("openid-configuration");
        let config = if config.starts_with("https://") && config.bytes().all(|b| b.is_ascii_graphic()) { config } else { "" };
        Some(SaslFailure { status: status.into(), scope: scope.into(), openid_configuration: config.into() })
    }

    /// Whether the server says the token is no good (expired, or revoked): a fresh one may do.
    pub fn token_rejected(&self) -> bool {
        self.status == "invalid_token"
    }

    /// Whether the server says the token doesn't cover mail: a fresh one won't do, a fresh sign-in may.
    pub fn scope_missing(&self) -> bool {
        self.status == "insufficient_scope"
    }
}

#[cfg(test)]
#[path = "oauthbearer_tests.rs"]
mod oauthbearer_tests;
