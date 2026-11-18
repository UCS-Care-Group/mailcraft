// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a Zimbra account's server is ([`ZimbraServer`], held by [`crate::Account::zimbra`]), and
//! what its settings must be before anything of the owner's is sent to it.
//!
//! Settings are checked as text, as an [`crate::EwsServer`]'s are: that takes no network library,
//! so the engine and the browser build can do it, and a host that is about to connect checks them
//! again, since a mailbox file is hostile too.

use serde::{Deserialize, Serialize};

/// The server of a [`crate::AccountKind::Zimbra`] account. Plain settings: the password is given
/// by whoever holds the connection, and is never kept here; nor is the sign-in token the server
/// hands back for it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ZimbraServer {
    /// The address of the server's web client (`https://mail.example.org`), under which its SOAP
    /// service (`/service/soap`) and its REST addresses (`/service/home/~/`) are. It names the
    /// organisation's server, so it is never written to a log; [`ZimbraServer::check_url`] says
    /// whether it may be used.
    pub url: String,
    /// The name the account signs in with (`ada` or `ada@example.org`); empty means the account's
    /// e-mail address.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The SHA-256 fingerprint (64 hexadecimal digits) of the one certificate the owner chose to
    /// trust for this account although the system doesn't; empty when none is. Never an "ignore
    /// errors" switch: any other certificate is refused.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trusted: String,
    /// Where the account's changes go on from: the token the server gave at the end of the last
    /// `SyncRequest`, opaque. Kept in the mailbox file with what it brought, in one step; empty
    /// until the mailbox has been read once. Not a setting: an edit of the settings leaves it as it
    /// is, and it is never checked as one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub token: String,
}

/// The address, the name, the fingerprint and the token stay out of debug output, which may reach
/// a log.
impl std::fmt::Debug for ZimbraServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZimbraServer")
            .field("url", &!self.url.is_empty())
            .field("username", &!self.username.is_empty())
            .field("trusted", &!self.trusted.is_empty())
            .field("token", &!self.token.is_empty())
            .finish()
    }
}

impl ZimbraServer {
    /// Longest address, in characters.
    pub const MAX_URL: usize = 2048;
    /// Longest user name, in characters.
    pub const MAX_USERNAME: usize = 254;
    /// Longest change token kept, in bytes: Zimbra's are a number, or two joined by a hyphen.
    pub const MAX_TOKEN: usize = 256;

    /// Whether `address` may be a Zimbra server's address: `Ok`, or why not, in words that follow
    /// the setting's name. The rules of a JMAP session address (`https://` only, plain `http://` to
    /// this computer only, for tests; no control character, space or backslash; no user name in
    /// front of the server; a plausible server name and port), and no query or fragment, since the
    /// service's paths are put after it.
    pub fn check_url(address: &str) -> Result<(), &'static str> {
        let host = crate::jmap_server::accepted(address)?;
        if address.contains(['?', '#']) {
            return Err("has a query or a fragment in it: give the address of the web client alone");
        }
        if crate::is_exchange_online(host) || crate::is_google_host(host) {
            return Err("is on Microsoft's or Google's servers, which are added as accounts of their own");
        }
        Ok(())
    }

    /// Whether `name` may be a user name: not too long and no control character.
    pub fn check_username(name: &str) -> Result<(), &'static str> {
        crate::EwsServer::check_username(name)
    }

    /// Whether `fingerprint` is none, or a SHA-256 fingerprint as 64 hexadecimal digits.
    pub fn check_fingerprint(fingerprint: &str) -> Result<(), &'static str> {
        crate::EwsServer::check_fingerprint(fingerprint)
    }

    /// Whether `token` may be a change token: at most [`ZimbraServer::MAX_TOKEN`] bytes of letters,
    /// digits, hyphens, dots and colons. One that may not is not gone on from: the mailbox is read
    /// afresh.
    pub fn token_ok(token: &str) -> bool {
        token.len() <= Self::MAX_TOKEN && token.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b))
    }

    /// Why these settings can't be used, when they can't.
    pub fn problem(&self) -> Option<&'static str> {
        Self::check_url(&self.url)
            .err()
            .or_else(|| Self::check_username(&self.username).err())
            .or_else(|| Self::check_fingerprint(&self.trusted).err())
    }

    /// The name of the server the address is on (`mail.example.org`), when the address may be
    /// used: for telling the owner which server it is, without the rest of the address.
    pub fn host(&self) -> Option<&str> {
        Self::check_url(&self.url).ok().and_then(|()| crate::jmap_server::accepted(&self.url).ok())
    }

    /// The address with no `/` at its end, which the service's paths follow.
    pub fn base(&self) -> &str {
        self.url.trim().trim_end_matches('/')
    }
}

#[cfg(test)]
#[path = "zimbra_server_tests.rs"]
mod zimbra_server_tests;
