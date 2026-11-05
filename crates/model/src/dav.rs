// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! DAV collection metadata and resource snapshots. No credentials or I/O live here.
use crate::{Contact, Event, Id};
use serde::{Deserialize, Serialize};

/// One discovered calendar or address book. Permissions default to read-only until known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Collection {
    pub account: Id,
    pub href: String,
    pub name: String,
    pub contacts: bool,
    pub read_only: bool,
}

impl Default for Collection {
    fn default() -> Self {
        Self { account: 0, href: String::new(), name: String::new(), contacts: false, read_only: true }
    }
}

/// The downloaded document and the parsed fields it produced. Keeping both lets a later edit
/// change known properties without rewriting unknown ones. An ETag is opaque, including quotes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Resource {
    pub account: Id,
    pub collection: String,
    pub href: String,
    pub etag: String,
    pub raw: String,
    pub contacts: Vec<Contact>,
    pub events: Vec<Event>,
}

#[cfg(test)]
#[path = "dav_tests.rs"]
mod tests;

// ---- Linchange CalDAV/CardDAV settings ----

/// The CalDAV and CardDAV services of an account on a Linchange server. Linchange serves both on
/// the same HTTPS listener as its web application: calendars under `/caldav/` and address books
/// under `/carddav/`.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavServer {
    /// The CalDAV service root (`https://mail.example.org/caldav/`). A calendar client walks down
    /// from it to the owner's principal and calendar home.
    pub caldav: String,
    /// The CardDAV service root (`https://mail.example.org/carddav/`).
    pub carddav: String,
}

/// The addresses stay out of debug output, which may reach a log.
impl std::fmt::Debug for DavServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavServer").field("caldav", &!self.caldav.is_empty()).field("carddav", &!self.carddav.is_empty()).finish()
    }
}

impl DavServer {
    /// Longest address either service may have, in characters.
    pub const MAX_URL: usize = 2048;

    /// Whether `address` may be a DAV service root: `Ok`, or why not, in words that follow the
    /// setting's name (`is empty`, `isn't an https:// address`…).
    ///
    /// - `https://` only; plain `http://` only to a server on this computer (for tests).
    /// - At most [`DavServer::MAX_URL`] characters, and no control character, space or backslash.
    /// - No user name or password in front of the server's name.
    /// - A server named by letters, digits, dots and dashes, and a port from 1 to 65535.
    ///
    /// Nothing is looked up and nothing is sent.
    pub fn check_url(address: &str) -> Result<(), &'static str> {
        if address.is_empty() {
            return Err("is empty");
        }
        if address.chars().take(Self::MAX_URL + 1).count() > Self::MAX_URL {
            return Err("is longer than 2048 characters");
        }
        if address.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err("has a control character or a space in it");
        }
        if address.contains('\\') {
            return Err("has a backslash in it");
        }
        let (secure, rest) = if let Some(rest) = after_scheme(address, "https://") {
            (true, rest)
        } else if let Some(rest) = after_scheme(address, "http://") {
            (false, rest)
        } else {
            return Err("isn't an https:// address");
        };
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = rest.get(..end).unwrap_or_default();
        if authority.is_empty() {
            return Err("names no server");
        }
        if authority.contains('@') {
            return Err("carries a user name or a password: the secret is given apart from the address");
        }
        let (host, port) = split_authority(authority)?;
        if let Some(port) = port {
            let digits = !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit());
            if !digits || !port.parse::<u16>().is_ok_and(|n| n != 0) {
                return Err("has a port that isn't a number from 1 to 65535");
            }
        }
        if !secure && !is_loopback(host) {
            return Err("is a plain http:// address to a server that isn't on this computer: use https://");
        }
        Ok(())
    }

    /// Why these settings can't be used, when they can't ([`DavServer::check_url`]).
    pub fn problem(&self) -> Option<&'static str> {
        Self::check_url(&self.caldav).err().or_else(|| Self::check_url(&self.carddav).err())
    }

    /// The server a service is on (`mail.example.org`), when its address may be used.
    pub fn host(address: &str) -> Option<&str> {
        if Self::check_url(address).is_err() {
            return None;
        }
        let rest = address.split_once("://").map(|(_, rest)| rest)?;
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = rest.get(..end)?;
        split_authority(authority).ok().map(|(host, _)| host)
    }

    /// The CalDAV and CardDAV service roots of a Linchange server at `host`, as its documentation
    /// publishes them.
    pub fn for_host(host: &str) -> Option<DavServer> {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() || host.len() > 253 || !host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b':' | b'[' | b']')) {
            return None;
        }
        let base = if host.contains(':') && !host.starts_with('[') { format!("https://[{host}]") } else { format!("https://{host}") };
        Some(DavServer { caldav: format!("{base}/caldav/"), carddav: format!("{base}/carddav/") })
    }
}

fn after_scheme<'a>(address: &'a str, scheme: &str) -> Option<&'a str> {
    address.get(..scheme.len()).filter(|head| head.eq_ignore_ascii_case(scheme)).and_then(|_| address.get(scheme.len()..))
}

fn split_authority(authority: &str) -> Result<(&str, Option<&str>), &'static str> {
    const NOT_A_NAME: &str = "names a server that isn't letters, digits, dots and dashes, or an IP address";
    if authority.starts_with('[') {
        let close = authority.find(']').ok_or(NOT_A_NAME)?;
        let host = authority.get(..=close).ok_or(NOT_A_NAME)?;
        let inside = authority.get(1..close).ok_or(NOT_A_NAME)?;
        if inside.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(NOT_A_NAME);
        }
        return match authority.get(close + 1..) {
            Some("") => Ok((host, None)),
            Some(after) => after.strip_prefix(':').map(|port| (host, Some(port))).ok_or(NOT_A_NAME),
            None => Err(NOT_A_NAME),
        };
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    if host.is_empty() || host.len() > 253 || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
        return Err(NOT_A_NAME);
    }
    Ok((host, port))
}

fn is_loopback(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    match host.strip_prefix('[').and_then(|inside| inside.strip_suffix(']')) {
        Some(inside) => inside.parse::<std::net::Ipv6Addr>().is_ok_and(|ip| ip.is_loopback()),
        None => host.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback()),
    }
}

#[cfg(test)]
#[path = "dav_server_tests.rs"]
mod server_tests;
