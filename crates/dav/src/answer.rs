// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A multistatus answer (RFC 4918 §13) read into typed entries.
//!
//! Everything is hostile. XML goes through [`crate::xml`] (size, depth and node budgets, no DTD);
//! every `href` is resolved against the address that was asked and dropped (and counted in
//! [`Multistatus::skipped`], never logged by name) unless it is on the account's origin; ETags and
//! sync tokens over [`MAX_TOKEN`] bytes are refused, names are cut at [`MAX_NAME`] characters,
//! colours are parsed strictly, and the number of entries, of components, of reports and of
//! hrefs a property holds is capped. A property that is missing says nothing about a resource.

use crate::href::Origin;
use crate::request::{APPLE, CALSERVER, MAX_TOKEN};
use crate::xml::{self, CALDAV, CARDDAV, DAV, Node};

/// Most entries one answer yields (resources a collection holds).
pub const MAX_ENTRIES: usize = 20_000;
/// Most collections an account keeps.
pub const MAX_COLLECTIONS: usize = 50;
/// Largest `calendar-data` or `address-data` kept in one entry; a larger one is flagged.
pub const MAX_DATA: usize = 1024 * 1024;
/// Longest display name kept, in characters.
pub const MAX_NAME: usize = mailcraft_model::MAX_NAME;
const MAX_LIST: usize = 32;
const MAX_HOMES: usize = 8;

/// Why an answer is not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerError {
    /// Not a multistatus, or XML over its budgets.
    Invalid,
    /// A sync token over [`MAX_TOKEN`] bytes or with a control character.
    Token,
}

impl std::fmt::Display for AnswerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "the server's answer is not a DAV answer",
            Self::Token => "the server's sync token is not one",
        })
    }
}
impl std::error::Error for AnswerError {}

/// One `response` of a multistatus.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// The resource's address: absolute, normalised, on the account's origin.
    pub href: String,
    /// The response's own status (RFC 4918 §9.2.1; 404 marks a member a sync has removed), 200
    /// when it has none and its properties came.
    pub status: u16,
    /// This is the address that was asked (a Depth 1 answer lists the collection itself).
    pub is_base: bool,
    /// `getetag`, opaque, quotes and weak mark included.
    pub etag: Option<String>,
    /// `calendar-data` or `address-data`.
    pub data: Option<String>,
    /// The data was over [`MAX_DATA`] and was not kept.
    pub data_too_large: bool,
    /// `resourcetype` holds `collection`.
    pub is_collection: bool,
    /// `resourcetype` came with a 200: what the entry is (`is_collection` and the rest) is known.
    /// An entry without it may be a calendar, or a member, whose properties failed.
    pub typed: bool,
    /// `resourcetype` holds `calendar` (RFC 4791 §4.2).
    pub is_calendar: bool,
    /// `resourcetype` holds `addressbook` (RFC 6352 §5.2).
    pub is_addressbook: bool,
    /// `displayname`, cut at [`MAX_NAME`] characters.
    pub display_name: Option<String>,
    /// `calendar-color`, `#RRGGBB` or `#RRGGBBAA` read strictly (the alpha is dropped).
    pub color: Option<[u8; 3]>,
    /// `supported-calendar-component-set`, upper case; none when the property was not given.
    pub components: Option<Vec<String>>,
    /// `supported-report-set`: the local names of the reports.
    pub reports: Vec<String>,
    /// `supported-address-data`: the `content-type;version` pairs.
    pub address_data: Vec<String>,
    /// From `current-user-privilege-set`: no write privilege is given. None when it was not given.
    pub read_only: Option<bool>,
    /// `getctag`.
    pub ctag: Option<String>,
    /// `sync-token` as a property of a collection.
    pub sync_token: Option<String>,
    /// `current-user-principal`.
    pub principal: Option<String>,
    /// `calendar-home-set`.
    pub calendar_homes: Vec<String>,
    /// `addressbook-home-set`.
    pub addressbook_homes: Vec<String>,
    /// A principal on **another** origin (iCloud answers one host with homes on another): an
    /// absolute address as data only. Nothing in this crate builds a credentialed request for it;
    /// discovery decides whether its host may be used.
    pub foreign_principal: Option<String>,
    /// Calendar homes on other origins, as data only (see [`Entry::foreign_principal`]).
    pub foreign_calendar_homes: Vec<String>,
    /// Address book homes on other origins, as data only.
    pub foreign_addressbook_homes: Vec<String>,
}

impl std::fmt::Debug for Entry {
    /// Status and flags only: no address, name, principal, home or calendar data.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("status", &self.status)
            .field("is_base", &self.is_base)
            .field("is_collection", &self.is_collection)
            .field("is_calendar", &self.is_calendar)
            .field("is_addressbook", &self.is_addressbook)
            .field("has_etag", &self.etag.is_some())
            .field("has_data", &self.data.is_some())
            .finish()
    }
}

impl std::fmt::Debug for Multistatus {
    /// Counts and flags only.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Multistatus")
            .field("entries", &self.entries.len())
            .field("has_sync_token", &self.sync_token.is_some())
            .field("truncated", &self.truncated)
            .field("skipped", &self.skipped)
            .field("capped", &self.capped)
            .finish()
    }
}

impl Entry {
    /// Whether a collection holds events: `VEVENT` is in its component set, or the set was not
    /// given (RFC 4791 §5.2.3: absent means every kind).
    pub fn holds_events(&self) -> bool {
        self.is_calendar && self.components.as_ref().is_none_or(|c| c.iter().any(|n| n == "VEVENT"))
    }
    /// Whether the server offers `sync-collection` (RFC 6578) on it.
    pub fn supports_sync(&self) -> bool {
        self.reports.iter().any(|r| r == "sync-collection")
    }
    /// Whether the entry is a removed member of a sync (404).
    pub fn is_gone(&self) -> bool {
        self.status == 404 || self.status == 410
    }
}

/// A read multistatus.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Multistatus {
    /// The responses that were usable, in order, at most [`MAX_ENTRIES`].
    pub entries: Vec<Entry>,
    /// The `sync-token` of the answer (RFC 6578 §3.6).
    pub sync_token: Option<String>,
    /// The answer stops short: a `507` for the request's own address (RFC 6578 §3.6). Ask again
    /// from `sync_token`.
    pub truncated: bool,
    /// Responses and properties dropped because their address was off the origin, malformed, or
    /// their ETag was not one.
    pub skipped: usize,
    /// More than [`MAX_ENTRIES`] came; the rest were not read.
    pub capped: bool,
}

impl Multistatus {
    /// Reads `body`. `base` is the address that was asked (on `origin`): `href`s resolve against
    /// it, and the entry for it is marked.
    pub fn parse(body: &[u8], origin: &Origin, base: &str) -> Result<Self, AnswerError> {
        let root = xml::parse(body).map_err(|_| AnswerError::Invalid)?;
        let base = origin.resolve(&origin.base(), base).map_err(|_| AnswerError::Invalid)?;
        let mut out = Self::default();
        if let Some(token) = root.child(DAV, "sync-token") {
            out.sync_token = Some(token_of(&token.text).ok_or(AnswerError::Token)?).filter(|t| !t.is_empty());
        }
        for response in root.children.iter().filter(|n| n.is(DAV, "response")) {
            if out.entries.len() >= MAX_ENTRIES {
                out.capped = true;
                break;
            }
            read_response(response, origin, &base, &mut out);
        }
        Ok(out)
    }

    /// The entries that are neither the asked address nor gone: the members.
    pub fn members(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.is_base && !e.is_gone())
    }
}

fn token_of(text: &str) -> Option<String> {
    let text = text.trim();
    (text.len() <= MAX_TOKEN && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

/// The numeric code of a status line (`HTTP/1.1 207 Multi-Status`).
pub fn status_code(line: &str) -> Option<u16> {
    let mut words = line.split_whitespace();
    if !words.next()?.starts_with("HTTP/") {
        return None;
    }
    words.next().filter(|w| w.len() == 3 && w.bytes().all(|b| b.is_ascii_digit()))?.parse::<u16>().ok().filter(|c| (100..=599).contains(c))
}

fn read_response(response: &Node, origin: &Origin, base: &str, out: &mut Multistatus) {
    let hrefs: Vec<&Node> = response.children.iter().filter(|n| n.is(DAV, "href")).take(MAX_HOMES).collect();
    let status = response.value(DAV, "status").and_then(status_code);
    let has_props = response.children.iter().any(|n| n.is(DAV, "propstat"));
    let first_only = if has_props || status.is_none() { 1 } else { MAX_HOMES };
    for href in hrefs.into_iter().take(first_only) {
        let Ok(url) = origin.resolve(base, &href.text) else {
            out.skipped += 1;
            continue;
        };
        let mut entry = Entry {
            is_base: url == base || url.trim_end_matches('/') == base.trim_end_matches('/'),
            href: url,
            status: status.unwrap_or(200),
            ..Entry::default()
        };
        if status == Some(507) && !has_props && entry.is_base {
            // RFC 6578 §3.6: only the request-URI's own 507 says the answer is cut short.
            out.truncated = true;
            continue;
        }
        if has_props && !read_props(response, origin, base, &mut entry, &mut out.skipped) {
            out.skipped += 1;
            continue;
        }
        if out.entries.len() >= MAX_ENTRIES {
            out.capped = true;
            return;
        }
        out.entries.push(entry);
    }
}

/// Reads the properties that came with 200. False when the entry is unusable (none are).
fn read_props(response: &Node, origin: &Origin, base: &str, e: &mut Entry, skipped: &mut usize) -> bool {
    for p in xml::properties(response) {
        match (p.namespace.as_str(), p.name.as_str()) {
            // (An ETag that is empty or isn't one is none: the member is still there, and is
            // not to be taken for one that is gone.)
            (DAV, "getetag") => match token_of(&p.text) {
                Some(t) if !t.is_empty() => e.etag = Some(t),
                _ => {}
            },
            (DAV, "displayname") => {
                let name: String = p.text.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
                if !name.is_empty() {
                    e.display_name = Some(name);
                }
            }
            (DAV, "resourcetype") => {
                e.typed = true;
                e.is_collection = p.child(DAV, "collection").is_some();
                e.is_calendar = p.child(CALDAV, "calendar").is_some();
                e.is_addressbook = p.child(CARDDAV, "addressbook").is_some();
            }
            (DAV, "sync-token") => {
                if let Some(t) = token_of(&p.text).filter(|t| !t.is_empty()) {
                    e.sync_token.get_or_insert(t);
                }
            }
            (CALSERVER, "getctag") => {
                if let Some(t) = token_of(&p.text).filter(|t| !t.is_empty()) {
                    e.ctag.get_or_insert(t);
                }
            }
            (APPLE, "calendar-color") => e.color = color(&p.text),
            (CALDAV, "supported-calendar-component-set") => {
                e.components = Some(
                    p.children
                        .iter()
                        .filter(|c| c.is(CALDAV, "comp"))
                        .filter_map(|c| c.component_name.as_deref())
                        .take(MAX_LIST)
                        .map(str::to_ascii_uppercase)
                        .collect(),
                );
            }
            (DAV, "supported-report-set") => {
                e.reports = p
                    .children
                    .iter()
                    .filter(|r| r.is(DAV, "supported-report"))
                    .filter_map(|r| r.child(DAV, "report"))
                    .flat_map(|r| r.children.iter())
                    .filter(|r| match r.namespace.as_str() {
                        DAV => r.name == "sync-collection",
                        CALDAV => matches!(r.name.as_str(), "calendar-query" | "calendar-multiget"),
                        CARDDAV => matches!(r.name.as_str(), "addressbook-query" | "addressbook-multiget"),
                        _ => false,
                    })
                    .map(|r| r.name.clone())
                    .take(MAX_LIST)
                    .collect();
            }
            (CARDDAV, "supported-address-data") => {
                e.address_data = p
                    .children
                    .iter()
                    .filter(|c| c.is(CARDDAV, "address-data-type"))
                    .map(|c| format!("{};{}", c.attribute("content-type").unwrap_or("text/vcard"), c.attribute("version").unwrap_or("3.0")))
                    .take(MAX_LIST)
                    .collect();
            }
            (DAV, "current-user-privilege-set") => {
                let write = p
                    .children
                    .iter()
                    .filter(|c| c.is(DAV, "privilege"))
                    .flat_map(|c| c.children.iter())
                    .any(|c| c.namespace == DAV && matches!(c.name.as_str(), "write" | "write-content" | "all"));
                e.read_only = Some(!write);
            }
            (DAV, "current-user-principal") => {
                if let Some(h) = p.child(DAV, "href") {
                    match origin.resolve(base, &h.text) {
                        Ok(url) => e.principal = Some(url),
                        Err(_) => {
                            *skipped += 1;
                            e.foreign_principal = origin.foreign(&h.text);
                        }
                    }
                }
            }
            (CALDAV, "calendar-home-set") => homes(p, origin, base, (&mut e.calendar_homes, &mut e.foreign_calendar_homes), skipped),
            (CARDDAV, "addressbook-home-set") => homes(p, origin, base, (&mut e.addressbook_homes, &mut e.foreign_addressbook_homes), skipped),
            (CALDAV, "calendar-data") | (CARDDAV, "address-data") => {
                if p.text.len() > MAX_DATA {
                    e.data_too_large = true;
                } else {
                    e.data = Some(p.text.clone());
                }
            }
            _ => {}
        }
    }
    true
}

fn homes(p: &Node, origin: &Origin, base: &str, into: (&mut Vec<String>, &mut Vec<String>), skipped: &mut usize) {
    let (into, foreign) = into;
    for h in p.children.iter().filter(|c| c.is(DAV, "href")).take(MAX_HOMES) {
        match origin.resolve(base, &h.text) {
            Ok(url) if !into.contains(&url) => into.push(url),
            Ok(_) => {}
            Err(_) => {
                *skipped += 1;
                if let Some(url) = origin.foreign(&h.text)
                    && foreign.len() < MAX_HOMES
                    && !foreign.contains(&url)
                {
                    foreign.push(url);
                }
            }
        }
    }
}

/// `#RRGGBB` or `#RRGGBBAA`, strictly (Apple's `calendar-color`); anything else is no colour.
pub fn color(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| hex.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok());
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// The collections of a Depth 1 listing of a home: events' calendars and address books, at most
/// [`MAX_COLLECTIONS`] of them. Plain resources in the home (a `personal.ics` a server keeps there)
/// and collections of other kinds are passed over, never a failure.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collections {
    /// Calendars that hold events.
    pub calendars: Vec<Entry>,
    /// Address books.
    pub address_books: Vec<Entry>,
    /// More collections than the cap came; the rest were left out.
    pub capped: bool,
    /// Entries passed over (plain resources, task or journal calendars, the home itself).
    pub passed_over: usize,
    /// Addresses dropped (off the origin and the like).
    pub skipped: usize,
}

impl Collections {
    /// Adds what another listing brought: a collection already held (by address) is not added
    /// twice, and the cap holds for the whole.
    pub fn merge(&mut self, other: Collections) {
        self.passed_over = self.passed_over.saturating_add(other.passed_over);
        self.skipped = self.skipped.saturating_add(other.skipped);
        self.capped |= other.capped;
        for (entries, books) in [(other.calendars, false), (other.address_books, true)] {
            for entry in entries {
                let held = self.calendars.iter().chain(self.address_books.iter()).any(|have| have.href == entry.href);
                if held {
                    self.passed_over = self.passed_over.saturating_add(1);
                } else if self.calendars.len() + self.address_books.len() >= MAX_COLLECTIONS {
                    self.capped = true;
                } else if books {
                    self.address_books.push(entry);
                } else {
                    self.calendars.push(entry);
                }
            }
        }
    }

    /// Sorts a read listing of the home asked at `base`.
    pub fn from_listing(listing: Multistatus) -> Self {
        let mut out = Self { skipped: listing.skipped, ..Self::default() };
        for entry in listing.entries {
            let book = entry.is_addressbook && entry.is_collection;
            // (A home that is itself a calendar or an address book lists itself: it is kept.)
            let usable = !entry.is_gone() && entry.is_collection && (book || entry.holds_events());
            if !usable {
                out.passed_over += 1;
            } else if out.calendars.len() + out.address_books.len() >= MAX_COLLECTIONS {
                out.capped = true;
            } else if book {
                out.address_books.push(entry);
            } else {
                out.calendars.push(entry);
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "answer_tests.rs"]
mod tests;
