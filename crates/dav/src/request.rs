// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The requests of CalDAV (RFC 4791), CardDAV (RFC 6352) and WebDAV sync (RFC 6578), as
//! descriptions the host performs.
//!
//! Every request goes through an [`Endpoint`], which holds the account's DAV [`Origin`] and, when
//! there is one, the credential. Every address is resolved and checked against that origin before
//! a request is built, so a credential is attached to nothing that is off it; a create, change or
//! delete must in addition lie inside the collection it is for. Bodies are small, fixed documents;
//! the only text put in them is escaped.

use crate::href::{HrefError, Origin};
use crate::http::{EndpointError, HttpRequest, Method, Secret};
use crate::xml::{CALDAV, CARDDAV, DAV};
use std::fmt;

/// Most hrefs in one multiget (RFC 4791 §7.9 and RFC 6352 §8.7 leave the number to the client).
pub const MAX_MULTIGET: usize = 100;
/// Longest ETag or sync token written into a request or kept from an answer.
pub const MAX_TOKEN: usize = 1_024;
/// Largest resource body sent (a larger one is shown read-only and never written back).
pub const MAX_RESOURCE: usize = 1024 * 1024;
/// Most results one sync-collection page asks for.
pub const MAX_PAGE: u32 = 1_000;
/// The longest name of a created resource, without its `.ics` or `.vcf`.
pub const MAX_NAME: usize = 128;
/// Apple's namespace for the colour of a calendar.
pub const APPLE: &str = "http://apple.com/ns/ical/";
/// The calendar server extension's namespace (`getctag`).
pub const CALSERVER: &str = "http://calendarserver.org/ns/";

/// What a collection holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Events (iCalendar).
    Calendar,
    /// Contacts (vCard).
    AddressBook,
}

impl Kind {
    fn data(self) -> &'static str {
        match self {
            Self::Calendar => "<c:calendar-data/>",
            Self::AddressBook => "<c:address-data/>",
        }
    }
    fn content_type(self) -> &'static str {
        match self {
            Self::Calendar => "text/calendar; charset=utf-8",
            Self::AddressBook => "text/vcard; charset=utf-8",
        }
    }
    fn extension(self) -> &'static str {
        match self {
            Self::Calendar => "ics",
            Self::AddressBook => "vcf",
        }
    }
}

/// Why a request was not built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    /// An address that is not on the account's origin, or not an address, or off its collection.
    Href(HrefError),
    /// The origin is not one a request goes to.
    Endpoint(EndpointError),
    /// A token, ETag, name or time that is not one.
    Invalid,
    /// Nothing to ask, or more than [`MAX_MULTIGET`] hrefs.
    Count,
    /// A body larger than [`MAX_RESOURCE`].
    TooLarge,
}

impl From<HrefError> for RequestError {
    fn from(e: HrefError) -> Self {
        Self::Href(e)
    }
}
impl From<EndpointError> for RequestError {
    fn from(e: EndpointError) -> Self {
        Self::Endpoint(e)
    }
}
impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Href(e) => e.fmt(f),
            Self::Endpoint(e) => e.fmt(f),
            Self::Invalid => f.write_str("a value the server gave is not one that can be sent back"),
            Self::Count => f.write_str("too many or no addresses in one request"),
            Self::TooLarge => f.write_str("the document is too large to send"),
        }
    }
}
impl std::error::Error for RequestError {}

/// Escapes text for XML content or an attribute value.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// A start or end of a time range: `YYYYMMDDTHHMMSSZ`, UTC (RFC 4791 §9.9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp(String);

impl Stamp {
    /// Checks the form and the ranges of its fields.
    pub fn parse(text: &str) -> Result<Self, RequestError> {
        let b = text.as_bytes();
        let digits = |r: std::ops::Range<usize>| {
            b.get(r).filter(|s| s.iter().all(u8::is_ascii_digit)).map(|s| s.iter().fold(0u32, |a, d| a * 10 + u32::from(d - b'0')))
        };
        let ok = b.len() == 16
            && b.get(8) == Some(&b'T')
            && b.get(15) == Some(&b'Z')
            && digits(0..4).is_some()
            && digits(4..6).is_some_and(|m| (1..=12).contains(&m))
            && digits(6..8).is_some_and(|d| (1..=31).contains(&d))
            && digits(9..11).is_some_and(|h| h < 24)
            && digits(11..13).is_some_and(|m| m < 60)
            && digits(13..15).is_some_and(|s| s < 61);
        if ok { Ok(Self(text.to_owned())) } else { Err(RequestError::Invalid) }
    }
    /// The text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The account's DAV server as far as requests go: its origin and, when there is one, the
/// credential. `Debug` shows only whether a credential is held.
#[derive(Clone)]
pub struct Endpoint {
    origin: Origin,
    auth: Option<Secret>,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint").field("credential", &self.auth.is_some()).finish()
    }
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= MAX_TOKEN && !token.chars().any(char::is_control)
}

fn document(root: &str, body: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<{root} xmlns:d=\"{DAV}\" xmlns:c=\"{CALDAV}\" xmlns:card=\"{CARDDAV}\" xmlns:a=\"{APPLE}\" xmlns:cs=\"{CALSERVER}\">{body}</{root}>"
    )
    .into_bytes()
}

impl Endpoint {
    /// An endpoint on `origin` that sends `auth` with every request, or none.
    pub fn new(origin: Origin, auth: Option<Secret>) -> Self {
        Self { origin, auth }
    }
    /// An endpoint that sends no credential: a probe.
    pub fn anonymous(origin: Origin) -> Self {
        Self { origin, auth: None }
    }
    /// The same endpoint without a credential.
    #[must_use]
    pub fn without_credential(&self) -> Self {
        Self { origin: self.origin.clone(), auth: None }
    }
    /// The origin every request is held to.
    pub fn origin(&self) -> &Origin {
        &self.origin
    }
    /// Resolves `url` on this origin (see [`Origin::resolve`]).
    pub fn resolve(&self, url: &str) -> Result<String, RequestError> {
        Ok(self.origin.resolve(&self.origin.base(), url)?)
    }

    fn build(&self, method: Method, url: &str, fields: &[(&str, &str)], body: Vec<u8>) -> Result<HttpRequest, RequestError> {
        let url = self.resolve(url)?;
        let mut all: Vec<(&str, &str)> = fields.to_vec();
        if let Some(auth) = &self.auth {
            all.push(("Authorization", auth.expose()));
        }
        Ok(HttpRequest::new(method, &url, &all, body)?)
    }

    fn xml(&self, method: Method, url: &str, depth: Option<&str>, body: Vec<u8>) -> Result<HttpRequest, RequestError> {
        let mut fields = vec![("Content-Type", "application/xml; charset=utf-8"), ("Accept", "application/xml, text/xml")];
        if let Some(depth) = depth {
            fields.push(("Depth", depth));
        }
        self.build(method, url, &fields, body)
    }

    /// `OPTIONS`, whose `DAV` header says what the server is (RFC 4918 §10.1).
    pub fn options(&self, url: &str) -> Result<HttpRequest, RequestError> {
        self.build(Method::Options, url, &[], Vec::new()).map(|r| r.with_response_limit(64 * 1024))
    }

    /// `PROPFIND` Depth 0 for the current user's principal (RFC 5397) and the resource type.
    pub fn propfind_principal(&self, url: &str) -> Result<HttpRequest, RequestError> {
        let body = document("d:propfind", "<d:prop><d:current-user-principal/><d:resourcetype/></d:prop>");
        self.xml(Method::Propfind, url, Some("0"), body)
    }

    /// `PROPFIND` Depth 0 of a principal for its calendar and address book homes (RFC 4791 §6.2.1,
    /// RFC 6352 §7.1.1).
    pub fn propfind_homes(&self, principal: &str) -> Result<HttpRequest, RequestError> {
        let body = document("d:propfind", "<d:prop><c:calendar-home-set/><card:addressbook-home-set/><d:displayname/></d:prop>");
        self.xml(Method::Propfind, principal, Some("0"), body)
    }

    /// `PROPFIND` Depth 1 of a home for its collections and what they are and allow.
    pub fn propfind_collections(&self, home: &str) -> Result<HttpRequest, RequestError> {
        let body = document(
            "d:propfind",
            "<d:prop><d:resourcetype/><d:displayname/><a:calendar-color/><c:supported-calendar-component-set/>\
             <card:supported-address-data/><d:supported-report-set/><d:current-user-privilege-set/><cs:getctag/><d:sync-token/></d:prop>",
        );
        self.xml(Method::Propfind, home, Some("1"), body)
    }

    /// `PROPFIND` Depth 0 of a collection for its change tag and sync token: the cheap "has
    /// anything changed" question.
    pub fn propfind_state(&self, collection: &str) -> Result<HttpRequest, RequestError> {
        let body = document("d:propfind", "<d:prop><cs:getctag/><d:sync-token/></d:prop>");
        self.xml(Method::Propfind, collection, Some("0"), body)
    }

    /// `calendar-query` for the events that overlap `start`..`end`, asking for `getetag` only
    /// (RFC 4791 §7.8).
    pub fn calendar_query(&self, collection: &str, start: &Stamp, end: &Stamp) -> Result<HttpRequest, RequestError> {
        if start.as_str() >= end.as_str() {
            return Err(RequestError::Invalid);
        }
        let body = document(
            "c:calendar-query",
            &format!(
                "<d:prop><d:getetag/></d:prop><c:filter><c:comp-filter name=\"VCALENDAR\"><c:comp-filter name=\"VEVENT\"><c:time-range start=\"{}\" end=\"{}\"/></c:comp-filter></c:comp-filter></c:filter>",
                start.as_str(),
                end.as_str()
            ),
        );
        self.xml(Method::Report, collection, Some("1"), body)
    }

    /// `calendar-multiget` or `addressbook-multiget` of at most [`MAX_MULTIGET`] resources, each
    /// on this origin and inside `collection` (RFC 4791 §7.9, RFC 6352 §8.7).
    pub fn multiget(&self, kind: Kind, collection: &str, hrefs: &[&str]) -> Result<HttpRequest, RequestError> {
        if hrefs.is_empty() || hrefs.len() > MAX_MULTIGET {
            return Err(RequestError::Count);
        }
        let mut list = String::new();
        for href in hrefs {
            let url = self.resolve(href)?;
            if !self.origin.is_within(collection, &url) {
                return Err(HrefError::OffCollection.into());
            }
            let path = self.origin.path(&url)?;
            list.push_str(&format!("<d:href>{}</d:href>", escape(&path)));
        }
        let (root, body) = match kind {
            Kind::Calendar => ("c:calendar-multiget", format!("<d:prop><d:getetag/>{}</d:prop>{list}", kind.data())),
            Kind::AddressBook => ("card:addressbook-multiget", format!("<d:prop><d:getetag/>{}</d:prop>{list}", kind.data().replace("c:", "card:"))),
        };
        self.xml(Method::Report, collection, None, document(root, &body))
    }

    /// `sync-collection` with `token` (none: the first sync) and a page size (RFC 6578 §3.2).
    pub fn sync_collection(&self, collection: &str, token: Option<&str>, limit: Option<u32>) -> Result<HttpRequest, RequestError> {
        let token = match token {
            Some(t) if !valid_token(t) => return Err(RequestError::Invalid),
            Some(t) => escape(t),
            None => String::new(),
        };
        let limit = limit.map_or(String::new(), |n| format!("<d:limit><d:nresults>{}</d:nresults></d:limit>", n.clamp(1, MAX_PAGE)));
        let body = document(
            "d:sync-collection",
            &format!("<d:sync-token>{token}</d:sync-token><d:sync-level>1</d:sync-level>{limit}<d:prop><d:getetag/></d:prop>"),
        );
        self.xml(Method::Report, collection, Some("0"), body)
    }

    /// `GET` of a resource on this origin.
    pub fn get(&self, url: &str) -> Result<HttpRequest, RequestError> {
        self.build(Method::Get, url, &[("Accept", "text/calendar, text/vcard, */*;q=0.1")], Vec::new())
            .map(|r| r.with_response_limit(MAX_RESOURCE.saturating_mul(2)))
    }

    /// `PUT` that creates `<name>.ics`/`.vcf` in `collection` and nothing that exists
    /// (`If-None-Match: *`, RFC 9110 §13.1.2). `name` is one plain segment, never a path.
    pub fn put_create(&self, kind: Kind, collection: &str, name: &str, body: &str) -> Result<(String, HttpRequest), RequestError> {
        let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if name.is_empty() || name.len() > MAX_NAME || !name.chars().all(plain) || name.starts_with('.') {
            return Err(RequestError::Invalid);
        }
        let base = self.resolve(collection)?;
        let url = format!("{}/{name}.{}", base.trim_end_matches('/'), kind.extension());
        let url = self.resolve(&url)?;
        if !self.origin.is_within(collection, &url) {
            return Err(HrefError::OffCollection.into());
        }
        self.put(kind, &url, body, ("If-None-Match", "*")).map(|r| (url, r))
    }

    /// `PUT` that replaces `resource` (inside `collection`) only while it still has `etag`
    /// (`If-Match`).
    pub fn put_change(&self, kind: Kind, collection: &str, resource: &str, etag: &str, body: &str) -> Result<HttpRequest, RequestError> {
        let url = self.inside(collection, resource)?;
        if !valid_token(etag) {
            return Err(RequestError::Invalid);
        }
        self.put(kind, &url, body, ("If-Match", etag))
    }

    /// `DELETE` of `resource` (inside `collection`) only while it still has `etag`.
    pub fn delete(&self, collection: &str, resource: &str, etag: &str) -> Result<HttpRequest, RequestError> {
        let url = self.inside(collection, resource)?;
        if !valid_token(etag) {
            return Err(RequestError::Invalid);
        }
        self.build(Method::Delete, &url, &[("If-Match", etag)], Vec::new())
    }

    fn inside(&self, collection: &str, resource: &str) -> Result<String, RequestError> {
        let url = self.resolve(resource)?;
        if self.origin.is_within(collection, &url) { Ok(url) } else { Err(HrefError::OffCollection.into()) }
    }

    fn put(&self, kind: Kind, url: &str, body: &str, condition: (&str, &str)) -> Result<HttpRequest, RequestError> {
        if body.len() > MAX_RESOURCE {
            return Err(RequestError::TooLarge);
        }
        self.build(Method::Put, url, &[("Content-Type", kind.content_type()), condition], body.as_bytes().to_vec())
    }
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
