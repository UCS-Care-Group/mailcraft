// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a domain keeps its calendars and its address books, by the records of RFC 6764: the
//! `_caldavs._tcp.<domain>` and `_carddavs._tcp.<domain>` SRV records (§3) and the `path` of
//! the TXT record beside each (§4), asked of the same DNS-over-HTTPS name servers as the mail
//! servers' records ([`discover`](crate::discover)): nobody new is told the domain. Only the
//! domain goes out (inside the fixed name of the service), never a mailbox name.
//!
//! The services over TLS alone: `_caldav._tcp` and `_carddav._tcp` name servers without
//! encryption, and are never asked for. Nothing is sent to the place found: whether a CalDAV
//! or CardDAV server answers there is another question (`mailcraft_dav::discovery`), and a
//! domain without a record is asked at its own well-known address.
//!
//! The answers are a name server's JSON and hostile: the records looked at are capped by the
//! lookup, a target that isn't a plain host name is passed over, and a path is taken only when
//! it is a short one of printable ASCII that starts with one slash.

use serde_json::Value;

use crate::NetError;
use crate::dns::{MAX_RESOLVERS, RESOLVERS, domain_name};
use crate::dns_srv::{SrvTarget, lookup_srv_at, question, records};

/// The record type of text.
const TXT: u64 = 16;
/// The longest path taken from a TXT record: one character string holds no more (RFC 1035
/// §3.3), less the key.
const MAX_PATH: usize = 250;
/// Most character strings of one TXT record looked at.
const MAX_STRINGS: usize = 16;

/// What is looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DavService {
    /// Calendars, over CalDAV (RFC 4791).
    Calendars,
    /// Address books, over CardDAV (RFC 6352).
    Contacts,
}

impl DavService {
    /// The service's name in the DNS, over TLS.
    fn name(self) -> &'static str {
        match self {
            Self::Calendars => "_caldavs._tcp",
            Self::Contacts => "_carddavs._tcp",
        }
    }
}

/// The places a domain names for a service.
#[derive(Clone, PartialEq, Eq)]
pub struct DavPlace {
    /// The domain that was asked about, lower-cased: what the hosts below are vouched for by,
    /// and by its DNS alone. A host outside it is named to the owner before a password goes there.
    pub domain: String,
    /// The hosts and their ports, reached over TLS, best first (lower priority first, then the
    /// higher weight, as for the mail servers' records): never empty, and the ones after the
    /// first are for when the ones before can't be reached.
    pub targets: Vec<SrvTarget>,
    /// The path the TXT record gives, when it gives one: where the server's tree starts. Without
    /// one the service's well-known address is asked.
    pub path: Option<String>,
}

/// The place is the organisation's own: not for a log.
impl std::fmt::Debug for DavPlace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavPlace").field("targets", &self.targets.len()).field("path", &self.path.is_some()).finish_non_exhaustive()
    }
}

/// The places `domain` names for `service`: its SRV targets, with the path of its TXT
/// record. `None` when the domain has no such record, or says the service isn't offered (a
/// target of `.`). Only the domain is sent, over HTTPS, to a public name server (Cloudflare's;
/// Google's when that one fails). Blocks, for some seconds at most per question and name server.
pub fn lookup_dav(domain: &str, service: DavService) -> Result<Option<DavPlace>, NetError> {
    lookup_dav_at(&RESOLVERS, domain, service)
}

/// [`lookup_dav`], asking the name servers at `endpoints` instead (`https://` addresses, or
/// `http://` ones on this machine).
pub fn lookup_dav_at(endpoints: &[&str], domain: &str, service: DavService) -> Result<Option<DavPlace>, NetError> {
    // (A port of 0 is no port to connect to.)
    let targets: Vec<SrvTarget> = lookup_srv_at(endpoints, service.name(), domain)?.into_iter().filter(|target| target.port != 0).collect();
    let Some(domain) = domain_name(domain).filter(|_| !targets.is_empty()) else { return Ok(None) };
    // The path is asked for only once there is a place for it to be at. A name server that
    // can't say leaves the well-known address, which every server answers at.
    let name = format!("{}.{domain}", service.name());
    let path = endpoints.iter().take(MAX_RESOLVERS).find_map(|endpoint| match question(endpoint, &name, "TXT").and_then(|answer| records(&answer)) {
        Ok(records) => Some(path(&records)),
        Err(e) => {
            log::debug!("a name server couldn't be asked: {e}");
            None
        }
    });
    Ok(Some(DavPlace { domain, targets, path: path.flatten() }))
}

/// The `path` of the TXT records in an answer (RFC 6764 §4, in the `key=value` strings of
/// RFC 6763 §6): `{"Status":0,"Answer":[{"type":16,"data":"\"path=/dav\""}]}`. The first
/// `path` is the one that counts (§6.4), and when it is no path there is none; whatever else is
/// in there is passed over.
fn path(records: &[Value]) -> Option<String> {
    let first = records
        .iter()
        .filter(|record| record.get("type").and_then(Value::as_u64) == Some(TXT))
        .filter_map(|record| record.get("data").and_then(Value::as_str))
        .flat_map(strings)
        // (A string without `=` is a key without a value, `path` too.)
        .find(|pair| pair.split('=').next().is_some_and(|key| key.eq_ignore_ascii_case("path")))?;
    let (_, value) = first.split_once('=')?;
    let fine = value.len() <= MAX_PATH
        && value.starts_with('/')
        && !value.starts_with("//")
        && value.bytes().all(|b| b.is_ascii_graphic() && !matches!(b, b'\\' | b'#' | b'"'));
    fine.then(|| value.to_owned())
}

/// The character strings of one TXT record as a name server writes them: each in quotes, with
/// a backslash before a quote or a backslash inside (`"path=/dav" "x=y"`), or, from a server
/// that leaves the quotes out, the text as it stands.
fn strings(data: &str) -> Vec<String> {
    let data = data.trim();
    if !data.starts_with('"') {
        return vec![data.to_owned()];
    }
    let mut found = Vec::new();
    let mut open: Option<String> = None;
    let mut chars = data.chars();
    while let Some(c) = chars.next() {
        match (&mut open, c) {
            (Some(_), '"') => {
                found.extend(open.take());
                if found.len() >= MAX_STRINGS {
                    break;
                }
            }
            (Some(text), '\\') => text.extend(chars.next()),
            (Some(text), c) => text.push(c),
            (None, '"') => open = Some(String::new()),
            (None, _) => {}
        }
    }
    found
}

#[cfg(test)]
#[path = "dav_srv_tests.rs"]
mod dav_srv_tests;
