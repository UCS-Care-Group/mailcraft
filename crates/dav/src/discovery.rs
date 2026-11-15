// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding out whether a CalDAV or CardDAV server answers at a place, and where its tree starts
//! (RFC 6764 §5 and §6), before the owner has typed a password.
//!
//! The search has a [`Start`]: the host, port and path a domain's SRV and TXT records name
//! ([`Start::from_srv`]; the host looks the records up, this crate asks no name server), the
//! domain itself at `/.well-known/caldav` or `/.well-known/carddav` when it has no such record
//! ([`Start::at_domain`]), or an address the owner typed or agreed to ([`Start::at`]).
//! [`probe`] asks there, through whatever performs the exchange:
//!
//! - **No credential goes out.** Every request is built by an anonymous
//!   [`Endpoint`](crate::request::Endpoint): there is nothing here it could send.
//! - **One origin.** A redirect is followed only on the origin the search started at, a step at a
//!   time and at most [`MAX_STEPS`] times. One that names another server is not followed: the
//!   address comes back ([`Found::Elsewhere`]) for the owner to be asked, since a password would
//!   go there next. `https` only (plain `http` to loopback, for tests).
//! - **A 401 alone proves nothing.** A server is taken for a DAV server by
//!   [`is_dav_evidence`](crate::status::is_dav_evidence) alone, and a 401 counts for the kind
//!   that is looked for only when its `DAV` header names that kind's class (`calendar-access`
//!   for calendars, `addressbook` for address books). A multistatus says no kind: what the
//!   server keeps is known once the walk for the principal has found its homes.
//! - **A stale path is not the end.** When the place a TXT record's path names is no DAV
//!   server, the well-known address on the same origin is asked once more (RFC 6764 §6).
//!
//! A [`Start`] remembers the domain it was looked up for ([`Start::domain`]). The host that
//! performs the exchange checks the certificate for the host it connects to; when that host is
//! outside the domain ([`Start::outside_domain`]) only the DNS said so, and the owner is to be
//! asked, naming the host, before a credential goes there, as for a mail server.
//!
//! What a server answers is hostile: a `Location` is resolved by [`Origin::resolve`] like any
//! `href`, and nothing of the answer is kept but the address it led to and the names of the
//! sign-in schemes it offers.

use crate::href::Origin;
use crate::http::{EndpointError, HttpExchange, HttpResponse, TransportError};
use crate::request::{Endpoint, Kind};
use crate::status::{Verdict, challenges, classify, dav_classes, is_dav_evidence};
use std::fmt;

/// Most redirects followed from one address (a search asks from two at most: a record's path,
/// then the well-known address).
pub const MAX_STEPS: usize = 5;
/// The longest host name taken from a record (RFC 1035 §2.3.4).
const MAX_HOST: usize = 253;

/// The address every server of a kind answers at, whatever its tree looks like (RFC 6764 §5).
pub fn well_known(kind: Kind) -> &'static str {
    match kind {
        Kind::Calendar => "/.well-known/caldav",
        Kind::AddressBook => "/.well-known/carddav",
    }
}

/// The class a server of a kind names in its `DAV` header (RFC 4791 §5.1, RFC 6352 §6.1).
fn class(kind: Kind) -> &'static str {
    match kind {
        Kind::Calendar => "calendar-access",
        Kind::AddressBook => "addressbook",
    }
}

/// A host or domain name as a record gives it: labels of letters, digits, hyphens and
/// underscores with dots between them, lower-cased.
fn name(text: &str) -> Result<String, EndpointError> {
    let label = |label: &str| !label.is_empty() && label.len() <= 63 && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if text.len() > MAX_HOST || !text.split('.').all(label) {
        return Err(EndpointError::Malformed);
    }
    Ok(text.to_ascii_lowercase())
}

/// Where a search starts: an origin and an address on it, with what is known of why there.
#[derive(Clone, PartialEq, Eq)]
pub struct Start {
    origin: Origin,
    url: String,
    /// What is looked for, when the start says.
    kind: Option<Kind>,
    /// The domain whose records or name led here.
    domain: Option<String>,
    /// The well-known address on the same origin, when the search starts at a record's path.
    well_known: Option<String>,
}

impl Start {
    /// The place `domain`'s `_caldavs._tcp` or `_carddavs._tcp` SRV record names (RFC 6764 §3),
    /// over TLS: `host` at `port`, at the `path` of the TXT record beside it (§4) or, without
    /// one, at the well-known address of `kind`. A domain or host that is no plain name, a port
    /// of 0 and a path that is none are refused.
    pub fn from_srv(domain: &str, host: &str, port: u16, path: Option<&str>, kind: Kind) -> Result<Self, EndpointError> {
        let (domain, host) = (name(domain)?, name(host)?);
        if port == 0 {
            return Err(EndpointError::Malformed);
        }
        let origin = Origin::parse(&format!("https://{host}:{port}"))?;
        let well_known = origin.resolve(&origin.base(), well_known(kind)).map_err(|_| EndpointError::Malformed)?;
        let (url, well_known) = match path {
            Some(path) if path.starts_with('/') && !path.starts_with("//") => {
                let url = origin.resolve(&origin.base(), path).map_err(|_| EndpointError::Malformed)?;
                // (A path that is the well-known address has nothing to fall back to.)
                let other = (url != well_known).then_some(well_known);
                (url, other)
            }
            Some(_) => return Err(EndpointError::Malformed),
            None => (well_known, None),
        };
        Ok(Self { origin, url, kind: Some(kind), domain: Some(domain), well_known })
    }

    /// The domain itself, at the well-known address of `kind`: where the search starts when the
    /// domain has no SRV record for the service (RFC 6764 §6).
    pub fn at_domain(domain: &str, kind: Kind) -> Result<Self, EndpointError> {
        Self::from_srv(domain, domain, 443, None, kind)
    }

    /// An address the owner typed, or one a server named and the owner agreed to
    /// ([`Found::Elsewhere`]): `https`, or `http` on this machine.
    pub fn at(url: &str) -> Result<Self, EndpointError> {
        let origin = Origin::parse(url)?;
        if !(origin.secure() || origin.is_loopback()) {
            return Err(EndpointError::NotEncrypted);
        }
        let url = origin.resolve(&origin.base(), url).map_err(|_| EndpointError::Malformed)?;
        Ok(Self { origin, url, kind: None, domain: None, well_known: None })
    }

    /// The origin the search keeps to.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    /// The address asked first.
    pub fn url(&self) -> &str {
        &self.url
    }
    /// The domain whose records or name led here; none for an address the owner gave.
    pub fn domain(&self) -> Option<&str> {
        self.domain.as_deref()
    }
    /// Whether the host asked is neither the domain nor a name in it: a place that only the
    /// domain's DNS vouches for.
    pub fn outside_domain(&self) -> bool {
        self.domain.as_deref().is_some_and(|domain| {
            let host = self.origin.host();
            host != domain && !host.strip_suffix(domain).is_some_and(|rest| rest.ends_with('.'))
        })
    }
}

impl fmt::Debug for Start {
    /// Nothing: the server's address is a setting, and is not logged.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Start(..)")
    }
}

/// What a search found.
#[derive(Clone, PartialEq, Eq)]
pub enum Found {
    /// A CalDAV or CardDAV server answers on the start's origin.
    Server {
        /// The address the search ended at: where the walk for the principal starts.
        root: String,
        /// The sign-in schemes the server offers (`basic`, `digest`, `bearer`…); none when it
        /// answered without asking who is there.
        schemes: Vec<String>,
    },
    /// The server sends the search on to another one, which was not asked: its address, for the
    /// owner to see whole and agree to before [`Start::at`] goes on there.
    Elsewhere(String),
    /// Something answered, and nothing says it is a CalDAV or CardDAV server.
    NotDav,
}

impl fmt::Debug for Found {
    /// The kind of answer and no address.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Server { .. } => "Server(..)",
            Self::Elsewhere(_) => "Elsewhere(..)",
            Self::NotDav => "NotDav",
        })
    }
}

/// Asks at `start` whether a CalDAV or CardDAV server is there, without a credential, following
/// redirects on the start's origin; when a record's path leads to none, once more from the
/// well-known address. `now` (Unix seconds) is passed on to [`classify`]. An error is the
/// exchange's own: nothing could be asked.
pub fn probe(http: &impl HttpExchange, start: &Start, now: i64) -> Result<Found, TransportError> {
    let mut asked: Vec<String> = Vec::new();
    match walk(http, start, &start.url, &mut asked, now)? {
        Found::NotDav => {}
        found => return Ok(found),
    }
    match &start.well_known {
        // (Not when the first walk has been there already.)
        Some(well_known) if !asked.contains(well_known) => walk(http, start, well_known, &mut asked, now),
        _ => Ok(Found::NotDav),
    }
}

/// Whether `response` is evidence of a server of the kind the start looks for.
fn evidence(response: &HttpResponse, start: &Start, url: &str) -> bool {
    is_dav_evidence(response, &start.origin, url)
        && (response.status() != 401 || start.kind.is_none_or(|kind| dav_classes(response).iter().any(|c| c == class(kind))))
}

/// One walk from `from`: the address, and what its redirects on the origin lead to. `asked`
/// holds every address a request went to.
fn walk(http: &impl HttpExchange, start: &Start, from: &str, asked: &mut Vec<String>, now: i64) -> Result<Found, TransportError> {
    let origin = &start.origin;
    let endpoint = Endpoint::anonymous(origin.clone());
    let mut url = from.to_owned();
    for _ in 0..=MAX_STEPS {
        let Ok(request) = endpoint.propfind_principal(&url) else { return Ok(Found::NotDav) };
        let response = http.exchange(&request)?;
        if evidence(&response, start, &url) {
            return Ok(Found::Server { root: url, schemes: challenges(&response) });
        }
        asked.push(url.clone());
        let Verdict::Redirect(_) = classify(&response, now) else { return Ok(Found::NotDav) };
        let Some(location) = response.header("location") else { return Ok(Found::NotDav) };
        match origin.resolve(&url, location) {
            Ok(next) => {
                // (A server that sends the search round in a circle is no server to wait for.)
                if asked.contains(&next) {
                    return Ok(Found::NotDav);
                }
                url = next;
            }
            Err(_) => {
                // (An address without a scheme on another host is that host over TLS.)
                let location = location.trim();
                let absolute = if location.starts_with("//") { format!("https:{location}") } else { location.to_owned() };
                return Ok(origin.foreign(&absolute).map_or(Found::NotDav, Found::Elsewhere));
            }
        }
    }
    Ok(Found::NotDav)
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
