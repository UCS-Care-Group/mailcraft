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
//!   [`is_dav_evidence`](crate::status::is_dav_evidence) alone.
//!
//! What a server answers is hostile: a `Location` is resolved by [`Origin::resolve`] like any
//! `href`, and nothing of the answer is kept but the address it led to and the names of the
//! sign-in schemes it offers.

use crate::href::Origin;
use crate::http::{EndpointError, HttpExchange, TransportError};
use crate::request::{Endpoint, Kind};
use crate::status::{Verdict, challenges, classify, is_dav_evidence};
use std::fmt;

/// Most redirects followed in one search.
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

/// Where a search starts: an origin and an address on it.
#[derive(Clone, PartialEq, Eq)]
pub struct Start {
    origin: Origin,
    url: String,
}

impl Start {
    /// The place a domain's `_caldavs._tcp` or `_carddavs._tcp` SRV record names (RFC 6764 §3),
    /// over TLS: `host` at `port`, at the `path` of the TXT record beside it (§4) or, without
    /// one, at the well-known address of `kind`. A host that is no plain name, a port of 0 and a
    /// path that is none are refused.
    pub fn from_srv(host: &str, port: u16, path: Option<&str>, kind: Kind) -> Result<Self, EndpointError> {
        let label = |label: &str| !label.is_empty() && label.len() <= 63 && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if port == 0 || host.len() > MAX_HOST || !host.split('.').all(label) {
            return Err(EndpointError::Malformed);
        }
        let origin = Origin::parse(&format!("https://{host}:{port}"))?;
        let path = match path {
            Some(path) if path.starts_with('/') && !path.starts_with("//") => path,
            Some(_) => return Err(EndpointError::Malformed),
            None => well_known(kind),
        };
        Self::on(origin, path)
    }

    /// The domain itself, at the well-known address of `kind`: where the search starts when the
    /// domain has no SRV record for the service (RFC 6764 §6).
    pub fn at_domain(domain: &str, kind: Kind) -> Result<Self, EndpointError> {
        Self::from_srv(domain, 443, None, kind)
    }

    /// An address the owner typed, or one a server named and the owner agreed to
    /// ([`Found::Elsewhere`]): `https`, or `http` on this machine.
    pub fn at(url: &str) -> Result<Self, EndpointError> {
        let origin = Origin::parse(url)?;
        if !(origin.secure() || origin.is_loopback()) {
            return Err(EndpointError::NotEncrypted);
        }
        Self::on(origin, url)
    }

    fn on(origin: Origin, address: &str) -> Result<Self, EndpointError> {
        let url = origin.resolve(&origin.base(), address).map_err(|_| EndpointError::Malformed)?;
        Ok(Self { origin, url })
    }

    /// The origin the search keeps to.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    /// The address asked first.
    pub fn url(&self) -> &str {
        &self.url
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
/// redirects on the start's origin. `now` (Unix seconds) is passed on to [`classify`]. An error
/// is the exchange's own: nothing could be asked.
pub fn probe(http: &impl HttpExchange, start: &Start, now: i64) -> Result<Found, TransportError> {
    let origin = &start.origin;
    let endpoint = Endpoint::anonymous(origin.clone());
    let mut url = start.url.clone();
    let mut asked: Vec<String> = Vec::new();
    for _ in 0..=MAX_STEPS {
        let Ok(request) = endpoint.propfind_principal(&url) else { return Ok(Found::NotDav) };
        let response = http.exchange(&request)?;
        if is_dav_evidence(&response, origin, &url) {
            return Ok(Found::Server { root: url, schemes: challenges(&response) });
        }
        let Verdict::Redirect(_) = classify(&response, now) else { return Ok(Found::NotDav) };
        let Some(location) = response.header("location") else { return Ok(Found::NotDav) };
        match origin.resolve(&url, location) {
            Ok(next) => {
                // (A server that sends the search round in a circle is no server to wait for.)
                asked.push(url);
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
