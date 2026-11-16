// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where an account's calendar and contacts servers are (CalDAV and CardDAV; [`DavServer`], held
//! by [`crate::Account::dav`]), what is known of each of their collections, and what the settings
//! must be before anything of the owner's is sent there (#209, spec C4, ADR 0026).
//!
//! Settings are checked as text, as an [`crate::EwsServer`]'s are: that takes no network library,
//! so the engine and the browser build can do it, and a host that is about to connect checks them
//! again, since a mailbox file is hostile too.
//!
//! **The password is per origin.** A DAV server's password is never kept here, and never borrowed
//! from another origin: a home and every collection is on an origin (scheme, host and port,
//! [`origin_of`]), and what a host holds for one origin is sent to that origin alone
//! ([`DavServer::origins`] lists them; the credential store names an entry by address, origin,
//! user name and pinned certificate).

use serde::{Deserialize, Serialize};

use crate::{Rgb, is_exchange_online, is_google_host, is_microsoft_domain};

/// What is known of one calendar or address book of the server, between two runs (ADR 0026): kept
/// in the mailbox file with the account, written in the same step as the events or contacts it
/// brought, so that a crash can't leave a token ahead of the data. Not a setting: an edit of the
/// settings leaves it as it is, unless the server changes, which forgets it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavCollectionState {
    /// The collection's address, absolute (it may be on another origin than the home: iCloud's).
    /// Never written to a log.
    pub href: String,
    /// What the server calls it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// An address book (CardDAV), else a calendar.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub contacts: bool,
    /// The colour the server gave it, when it did.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub color: Option<Rgb>,
    /// The owner may not change what is in it (`current-user-privilege-set`, or not known yet).
    pub read_only: bool,
    /// The kinds of component it holds (`VEVENT`, `VTODO`), as `supported-calendar-component-set`
    /// said; empty when it didn't say.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub components: Vec<String>,
    /// Where the collection's changes go on from (RFC 6578), opaque; empty before the first read.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sync_token: String,
    /// The collection's change tag (`getctag`), opaque, for a server without sync tokens.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub ctag: String,
}

/// Nothing is known of the owner's rights until the server has said, so a collection starts, and a
/// file that doesn't say reads, as read-only.
impl Default for DavCollectionState {
    fn default() -> Self {
        DavCollectionState {
            href: String::new(),
            name: String::new(),
            contacts: false,
            color: None,
            read_only: true,
            components: Vec::new(),
            sync_token: String::new(),
            ctag: String::new(),
        }
    }
}

/// The address and name stay out of debug output, which may reach a log.
impl std::fmt::Debug for DavCollectionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavCollectionState")
            .field("href", &!self.href.is_empty())
            .field("contacts", &self.contacts)
            .field("read_only", &self.read_only)
            .field("sync_token", &!self.sync_token.is_empty())
            .finish()
    }
}

/// The calendar and contacts servers of an account. Plain settings: the password is given by
/// whoever holds the connection, per origin, and is never kept here.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavServer {
    /// The addresses of the calendar homes (`calendar-home-set`), absolute, at most
    /// [`DavServer::MAX_HOMES`]; empty: no calendars. They may name the organisation's servers,
    /// so they are never written to a log; [`DavServer::check_home`] says whether one may be used.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub calendar_homes: Vec<String>,
    /// The addresses of the contacts homes (`addressbook-home-set`), as above.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub contact_homes: Vec<String>,
    /// The name the account signs in with; empty means the account's own (its address).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The SHA-256 fingerprint (64 hexadecimal digits) of the one certificate the owner chose to
    /// trust for this account although the system doesn't; empty when none is. Never an "ignore
    /// errors" switch.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trusted: String,
    /// The password for these servers is not the account's own: the owner gave one for them.
    /// Either way it is held, and sent, per origin.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub separate_password: bool,
    /// What is known of each collection, at most [`DavServer::MAX_COLLECTIONS`]. Not a setting.
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::sync::lenient_list")]
    pub collections: Vec<DavCollectionState>,
    /// What a later version wrote here that this one doesn't know, kept as it was so that saving
    /// here doesn't lose it.
    #[serde(flatten, skip_serializing_if = "serde_json::Map::is_empty")]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// Only whether each field is set: an address, a name and a fingerprint stay out of debug output,
/// which may reach a log.
impl std::fmt::Debug for DavServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavServer")
            .field("calendar_homes", &self.calendar_homes.len())
            .field("contact_homes", &self.contact_homes.len())
            .field("username", &!self.username.is_empty())
            .field("trusted", &!self.trusted.is_empty())
            .field("separate_password", &self.separate_password)
            .field("collections", &self.collections.len())
            .field("other", &!self.other.is_empty())
            .finish()
    }
}

impl DavServer {
    /// Longest address, in characters.
    pub const MAX_ADDRESS: usize = 2048;
    /// Most homes of each kind.
    pub const MAX_HOMES: usize = 8;
    /// Most collections an account keeps state for (spec Q18).
    pub const MAX_COLLECTIONS: usize = 50;
    /// Longest user name, in characters.
    pub const MAX_USERNAME: usize = crate::EwsServer::MAX_USERNAME;

    /// Whether `address` may be a DAV home or collection address: `Ok`, or why not, in words that
    /// follow the setting's name. `https://` only (plain `http://` to this computer only, for
    /// tests), no control character, space or backslash, no user name in front of the server, a
    /// plausible server name and port, and not Microsoft's (a Microsoft account signs in on
    /// Microsoft's page) nor Google's (a password never goes there: Google signs in with a token).
    pub fn check_home(address: &str) -> Result<(), &'static str> {
        let host = crate::jmap_server::accepted(address)?;
        if is_exchange_online(host) || is_microsoft_domain(host) {
            return Err("is on a Microsoft server: Microsoft 365 and Outlook.com is added as a Microsoft account, not as a calendar server");
        }
        if is_google_host(host) {
            return Err("is on a Google server: a password is never sent there");
        }
        Ok(())
    }

    /// Whether `name` may be a user name (as [`crate::EwsServer::check_username`]).
    pub fn check_username(name: &str) -> Result<(), &'static str> {
        crate::EwsServer::check_username(name)
    }

    /// Whether `fingerprint` is none, or a SHA-256 fingerprint as 64 hexadecimal digits.
    pub fn check_fingerprint(fingerprint: &str) -> Result<(), &'static str> {
        crate::EwsServer::check_fingerprint(fingerprint)
    }

    /// Whether `homes` may be a list of homes: not too many, each one usable.
    pub fn check_homes(homes: &[String]) -> Result<(), &'static str> {
        if homes.len() > Self::MAX_HOMES {
            return Err("names more than 8 homes");
        }
        homes.iter().try_for_each(|home| Self::check_home(home))
    }

    /// Why these settings can't be used, when they can't: the setting at fault (`homes`,
    /// `username`, `trusted`) and why, in words that follow its name.
    pub fn problem(&self) -> Option<(&'static str, &'static str)> {
        if self.calendar_homes.is_empty() && self.contact_homes.is_empty() {
            return Some(("homes", "need a calendar or contacts address"));
        }
        Self::check_homes(&self.calendar_homes)
            .err()
            .or_else(|| Self::check_homes(&self.contact_homes).err())
            .map(|why| ("homes", why))
            .or_else(|| Self::check_username(&self.username).err().map(|why| ("username", why)))
            .or_else(|| Self::check_fingerprint(&self.trusted).err().map(|why| ("trusted", why)))
    }

    /// The origins the server's homes and collections are on, each once, in the order met: where a
    /// password may be asked for and held, one per origin ([`origin_of`]). Addresses that can't
    /// be used name none.
    pub fn origins(&self) -> Vec<String> {
        let mut origins: Vec<String> = Vec::new();
        let named = self.calendar_homes.iter().chain(&self.contact_homes).map(String::as_str).chain(self.collections.iter().map(|c| c.href.as_str()));
        for address in named {
            if let Some(origin) = origin_of(address)
                && !origins.contains(&origin)
            {
                origins.push(origin);
            }
        }
        origins
    }

    /// The name of the server the first home is on (`dav.example.org`), when the address may be
    /// used: for telling the owner which server it is, without the rest of the address.
    pub fn host(&self) -> Option<&str> {
        self.calendar_homes.iter().chain(&self.contact_homes).find_map(|home| crate::jmap_server::accepted(home).ok())
    }

    /// Whether `other` is another server than this: other homes, another user name or another
    /// pinned certificate. What was kept for the old one (the collections' tokens, the secrets
    /// held for it) is not given to the new.
    pub fn is_another_server(&self, other: &DavServer) -> bool {
        let homes = |s: &DavServer| {
            let mut all: Vec<(bool, String)> = s
                .calendar_homes
                .iter()
                .map(|h| (false, h.trim().to_string()))
                .chain(s.contact_homes.iter().map(|h| (true, h.trim().to_string())))
                .collect();
            all.sort();
            all
        };
        homes(self) != homes(other)
            || self.username.trim() != other.username.trim()
            || self.trusted.trim().to_lowercase() != other.trusted.trim().to_lowercase()
    }
}

/// The origin of `address` (`https://dav.example.org`, `https://dav.example.org:8443`): the scheme
/// and host in lower case and the port when it isn't the scheme's own; `None` when the address
/// may not be used ([`DavServer::check_home`] says why: Microsoft's and Google's hosts have no origin). Two addresses with the same origin share
/// a password; one with another origin never does.
pub fn origin_of(address: &str) -> Option<String> {
    use crate::jmap_server::{after_scheme, split_authority};
    // (The refusals of a home are the origin's too: no password is ever held for Microsoft's or
    // Google's hosts.)
    DavServer::check_home(address).ok()?;
    let (scheme, default_port, rest) =
        if let Some(rest) = after_scheme(address, "https://") { ("https", "443", rest) } else { ("http", "80", after_scheme(address, "http://")?) };
    let authority = rest.get(..rest.find(['/', '?', '#']).unwrap_or(rest.len()))?;
    let (host, port) = split_authority(authority).ok()?;
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    Some(match port.filter(|port| port.trim_start_matches('0') != default_port) {
        Some(port) => format!("{scheme}://{host}:{}", port.trim_start_matches('0')),
        None => format!("{scheme}://{host}"),
    })
}

#[cfg(test)]
#[path = "dav_server_tests.rs"]
mod dav_server_tests;
