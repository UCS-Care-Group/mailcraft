// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where an Exchange server account's Exchange Web Services are ([`EwsServer`], held by
//! [`crate::Account::ews`]), and what its settings must be before anything of the owner's is sent
//! to it (#252).
//!
//! Settings are checked as text, as a [`crate::JmapServer`]'s are: that takes no network library,
//! so the engine and the browser build can do it, and a host that is about to connect checks them
//! again, since a mailbox file is hostile too.

use serde::{Deserialize, Serialize};

/// The server of a [`crate::AccountKind::Exchange`] account. Plain settings: the password is
/// given by whoever holds the connection, and is never kept here.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsServer {
    /// The address of the Exchange Web Services endpoint (`https://…/EWS/Exchange.asmx`). It may
    /// name the organisation's servers, so it is never written to a log;
    /// [`EwsServer::check_url`] says whether it may be used.
    pub url: String,
    /// The name the account signs in with (`DOMAIN\user` or a UPN); empty means the account's
    /// e-mail address.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The SHA-256 fingerprint (64 hexadecimal digits) of the one certificate the owner chose to
    /// trust for this account although the system doesn't; empty when none is. Never an "ignore
    /// errors" switch: any other certificate is refused.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trusted: String,
    /// Where the account's folder hierarchy goes on from: the sync state the server gave at the
    /// end of the last `SyncFolderHierarchy`, opaque. Kept in the mailbox file with the folders it
    /// brought, in one step; empty until the hierarchy has been read once. Not a setting: an edit
    /// of the settings leaves it as it is, and it is never checked as one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hierarchy: String,
}

/// The address, the name and the fingerprint stay out of debug output, which may reach a log.
impl std::fmt::Debug for EwsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsServer")
            .field("url", &!self.url.is_empty())
            .field("username", &!self.username.is_empty())
            .field("trusted", &!self.trusted.is_empty())
            .field("hierarchy", &!self.hierarchy.is_empty())
            .finish()
    }
}

impl EwsServer {
    /// Longest address, in characters.
    pub const MAX_URL: usize = 2048;
    /// Longest user name, in characters.
    pub const MAX_USERNAME: usize = 254;

    /// Whether `address` may be an EWS address: `Ok`, or why not, in words that follow the
    /// setting's name. The rules of a JMAP session address: `https://` only (plain `http://` to
    /// this computer only, for tests), no control character, space or backslash, no user name in
    /// front of the server, a plausible server name and port.
    pub fn check_url(address: &str) -> Result<(), &'static str> {
        let host = crate::jmap_server::accepted(address)?;
        if is_exchange_online(host) {
            return Err("is on a Microsoft server: Microsoft 365 and Outlook.com mail is added as a Microsoft account, not as an Exchange server");
        }
        Ok(())
    }

    /// Whether `name` may be a user name: not too long and no control character. `DOMAIN\user`
    /// and `user@example.org` are both fine.
    pub fn check_username(name: &str) -> Result<(), &'static str> {
        if name.chars().take(Self::MAX_USERNAME + 1).count() > Self::MAX_USERNAME {
            return Err("is longer than 254 characters");
        }
        if name.chars().any(char::is_control) {
            return Err("has a control character in it");
        }
        Ok(())
    }

    /// Whether `fingerprint` is none, or a SHA-256 fingerprint as 64 hexadecimal digits.
    pub fn check_fingerprint(fingerprint: &str) -> Result<(), &'static str> {
        if fingerprint.is_empty() || (fingerprint.len() == 64 && fingerprint.bytes().all(|b| b.is_ascii_hexdigit())) {
            Ok(())
        } else {
            Err("isn't a SHA-256 fingerprint (64 hexadecimal digits)")
        }
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
        crate::jmap_server::accepted(&self.url).ok()
    }
}

/// Hosts that are Microsoft's own mail: Exchange Online (Microsoft 365) and Outlook.com: a name
/// and every name under it. Fixed; nothing is guessed from a pattern, so an organisation's own
/// server called `outlook.abc.com` is not taken for Microsoft's.
const EXCHANGE_ONLINE: [&str; 12] = [
    "outlook.com",
    "hotmail.com",
    "live.com",
    "msn.com",
    "office365.com",
    "office365.us",
    "office.com",
    "onmicrosoft.com",
    "microsoftonline.com",
    "outlook.cn",
    "partner.outlook.cn",
    "office365.cn",
];

/// Whether `host` (lower case, no port) is, or is under, one of [`EXCHANGE_ONLINE`]'s hosts.
/// Used for every host the search is sent to or told about: a redirect, an SRV target, the
/// addresses in an answer. A mailbox there is a Microsoft account, never an Exchange server
/// account, and no sign-in is ever sent there.
pub fn is_exchange_online(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    EXCHANGE_ONLINE.iter().any(|online| is_within(&host, online))
}

/// Whether the domain of an e-mail address (lower case) is one of Microsoft's consumer mail
/// domains, in the forms Microsoft has them in: `outlook.com` and `msn.com`; `hotmail` and
/// `live` and `outlook` with `com`, a country (`hotmail.fr`, `live.it`, `outlook.de`), or `co` or
/// `com` and a country (`hotmail.co.uk`, `live.com.au`, `outlook.com.br`). Only the domain of
/// the address itself: a name that merely starts so (`outlook.abc.com`, `live.bbc.com`,
/// `msn.xy.org`) is somebody else's.
fn is_microsoft_consumer_domain(domain: &str) -> bool {
    let labels: Vec<&str> = domain.split('.').collect();
    let country = |label: &&str| label.len() == 2 && label.bytes().all(|b| b.is_ascii_lowercase());
    match labels.as_slice() {
        ["msn", "com"] | ["outlook" | "hotmail" | "live", "com"] => true,
        ["outlook" | "hotmail" | "live", cc] => country(cc),
        ["outlook" | "hotmail" | "live", "co" | "com", cc] => country(cc),
        _ => false,
    }
}

/// Whether an address in `domain` is Microsoft's: Exchange Online's hosts or Outlook.com's
/// consumer domains. Nothing is sent to it.
pub fn is_microsoft_domain(domain: &str) -> bool {
    is_exchange_online(domain) || is_microsoft_consumer_domain(domain)
}

/// Whether `host` is `domain` or a name under it (`mail.example.org` is within `example.org`;
/// `evil-example.org` is not).
pub fn is_within(host: &str, domain: &str) -> bool {
    host == domain || host.strip_suffix(domain).is_some_and(|front| front.ends_with('.'))
}

#[cfg(test)]
#[path = "ews_server_tests.rs"]
mod ews_server_tests;
