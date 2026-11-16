// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Descriptions of one HTTP exchange, without performing it (the shape of `mailcraft-ews`).
//!
//! This crate builds an [`HttpRequest`]; the host performs it and describes what came back in an
//! [`HttpResponse`]. The host must send the method, address, fields and body exactly as given,
//! follow **no** redirect itself (a redirect is a decision of the discovery logic, because it
//! carries the owner's credentials), read [`HttpRequest::max_response_bytes`] before reading the
//! body and stop as soon as it is passed, and copy the response's header fields with
//! [`HttpResponse::with_field`].

use std::fmt;

/// The most an answer may weigh, in octets (`xml::MAX_BYTES`).
pub const RESPONSE_LIMIT: usize = crate::xml::MAX_BYTES;
/// The longest address a request goes to, and the longest `href` that is followed.
pub const MAX_ADDRESS: usize = 2_048;
/// The longest header field value a response keeps; longer ones are dropped.
pub const MAX_FIELD_VALUE_LEN: usize = 8 * 1024;
/// Most header fields a request or a response holds (the fields in [`READ_FIELDS`] come on top).
pub const MAX_FIELDS: usize = 32;
/// Fields of a response that are read whatever else came.
pub const READ_FIELDS: [&str; 5] = ["retry-after", "www-authenticate", "location", "dav", "etag"];
const MAX_FIELD_NAME_LEN: usize = 64;

/// A credential: a header value that is never shown, in `Debug` output or anywhere else.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Keeps `value` (`Basic …`) for the `Authorization` field, control characters taken out so
    /// that it can never end the field.
    pub fn new(value: &str) -> Self {
        Self(value.chars().filter(|c| !c.is_control()).collect())
    }
    /// `Basic <base64(user:password)>` (RFC 7617, UTF-8). A `:` in the user name is refused
    /// (RFC 7617 §2: the first colon ends it).
    pub fn basic(user: &str, password: &str) -> Result<Self, CredentialError> {
        if user.contains(':') {
            return Err(CredentialError);
        }
        const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let raw = format!("{user}:{password}");
        let mut out = String::from("Basic ");
        for chunk in raw.as_bytes().chunks(3) {
            let n = chunk.iter().enumerate().fold(0u32, |a, (i, b)| a | (u32::from(*b) << (16 - 8 * i)));
            for i in 0..4 {
                if i <= chunk.len() {
                    let index = usize::try_from((n >> (18 - 6 * i)) & 63).unwrap_or(0);
                    out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
                } else {
                    out.push('=');
                }
            }
        }
        Ok(Self::new(&out))
    }

    /// The value, for whoever sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// A user name that cannot be sent with Basic (it holds a `:`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialError;

impl fmt::Display for CredentialError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a user name with a colon cannot be used with Basic sign-in")
    }
}
impl std::error::Error for CredentialError {}

/// Why an address is not one a request goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointError {
    /// Not an `http` or `https` address, too long, or with a character no address has.
    Malformed,
    /// Plain `http` to anything but the machine itself.
    NotEncrypted,
    /// A user name or password written into the address.
    Credentials,
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "the server's address is not a web address",
            Self::NotEncrypted => "the server's address must start with https://",
            Self::Credentials => "the server's address must not hold a user name or a password",
        })
    }
}
impl std::error::Error for EndpointError {}

/// Checks the transport rule on `url`: `https`, or plain `http` to loopback only (for tests).
pub fn check_endpoint(url: &str) -> Result<(), EndpointError> {
    let origin = crate::href::Origin::parse(url)?;
    if origin.secure() || origin.is_loopback() { Ok(()) } else { Err(EndpointError::NotEncrypted) }
}

/// The methods this crate sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `GET`
    Get,
    /// `PUT`
    Put,
    /// `DELETE`
    Delete,
    /// `OPTIONS`
    Options,
    /// `PROPFIND` (RFC 4918)
    Propfind,
    /// `REPORT` (RFC 3253 §3.6, used by 4791, 6352 and 6578)
    Report,
}

impl Method {
    /// The method's name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Options => "OPTIONS",
            Self::Propfind => "PROPFIND",
            Self::Report => "REPORT",
        }
    }
}

fn field_name(name: &str) -> Option<String> {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    (!name.is_empty() && name.len() <= MAX_FIELD_NAME_LEN && name.chars().all(token)).then(|| name.to_ascii_lowercase())
}

/// A request to send.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    max_response: usize,
    repeatable: bool,
}

impl HttpRequest {
    /// A request of `method` to `url` with `fields` (at most [`MAX_FIELDS`]; a name that is no
    /// token is left out; control characters are taken out of a value) and `body`. Whether it may
    /// be sent again follows the method: everything but `PUT` and `DELETE` may.
    pub(crate) fn new(method: Method, url: &str, fields: &[(&str, &str)], body: Vec<u8>) -> Result<Self, EndpointError> {
        check_endpoint(url)?;
        let headers = fields
            .iter()
            .filter(|(name, _)| field_name(name).is_some())
            .take(MAX_FIELDS)
            .map(|(name, value)| ((*name).to_owned(), value.chars().filter(|c| !c.is_control()).collect()))
            .collect();
        let repeatable = !matches!(method, Method::Put | Method::Delete);
        Ok(Self { method, url: url.to_owned(), headers, body, max_response: RESPONSE_LIMIT, repeatable })
    }

    /// Lowers the largest response body accepted to `bytes` (it can only go down).
    #[must_use]
    pub fn with_response_limit(mut self, bytes: usize) -> Self {
        self.max_response = bytes.min(RESPONSE_LIMIT);
        self
    }

    /// Whether the host may send the request again after the connection failed. A change that
    /// may already have happened is never sent twice.
    pub fn is_repeatable(&self) -> bool {
        self.repeatable
    }
    /// The method.
    pub fn method(&self) -> Method {
        self.method
    }
    /// The address.
    pub fn url(&self) -> &str {
        &self.url
    }
    /// The header fields to send, in order.
    pub fn fields(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }
    /// The value of the first field called `name` (without regard to case).
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields().find(|(have, _)| have.eq_ignore_ascii_case(name)).map(|(_, v)| v)
    }
    /// The body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
    /// The most body octets to read from the response.
    pub fn max_response_bytes(&self) -> usize {
        self.max_response
    }
}

impl fmt::Debug for HttpRequest {
    /// The method and the body's length: no address, no credential, no content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest").field("method", &self.method).field("body_len", &self.body.len()).finish()
    }
}

/// What came back, as the host describes it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HttpResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    /// A response with a status code and a body.
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body, ..Self::default() }
    }

    /// Adds a header field. At most [`MAX_FIELDS`] others, a name that is a token of at most 64
    /// characters (lower-cased), a value of at most [`MAX_FIELD_VALUE_LEN`] bytes; anything else
    /// is dropped. A field that comes twice is kept twice (several `WWW-Authenticate`s). The
    /// fields in [`READ_FIELDS`] are kept (up to sixteen of each) whatever came before them.
    #[must_use]
    pub fn with_field(mut self, name: &str, value: &str) -> Self {
        let value = value.trim_matches([' ', '\t']);
        if value.len() > MAX_FIELD_VALUE_LEN {
            return self;
        }
        let Some(name) = field_name(name) else { return self };
        let read = READ_FIELDS.contains(&name.as_str());
        let held = if read {
            self.headers.iter().filter(|(have, _)| *have == name).count()
        } else {
            self.headers.iter().filter(|(have, _)| !READ_FIELDS.contains(&have.as_str())).count()
        };
        if held < if read { 16 } else { MAX_FIELDS } {
            let value = value.chars().map(|c| if c.is_control() && c != '\t' { ' ' } else { c }).collect();
            self.headers.push((name, value));
        }
        self
    }

    /// The status code.
    pub fn status(&self) -> u16 {
        self.status
    }
    /// The value of the first field called `name` (without regard to case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers_named(name).next()
    }
    /// The values of every field called `name`, in the order they came.
    pub fn headers_named<'r>(&'r self, name: &str) -> impl Iterator<Item = &'r str> {
        let name = name.to_ascii_lowercase();
        self.headers.iter().filter(move |(have, _)| *have == name).map(|(_, v)| v.as_str())
    }
    /// The body.
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

impl fmt::Debug for HttpResponse {
    /// Status and body length: the body may hold someone's mail.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse").field("status", &self.status).field("fields", &self.headers.len()).field("body_len", &self.body.len()).finish()
    }
}

/// Why the host could not complete an exchange; no free text, so nothing private can travel in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// The server could not be reached.
    Unreachable,
    /// The connection failed or timed out; for a request that may have happened this is `Lost`.
    Failed,
    /// The answer's body passed the request's limit.
    BodyTooLarge,
    /// A change went out and its answer was lost: it may have happened, and is not sent again.
    Lost,
    /// A certificate the system does not trust.
    Certificate,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unreachable => "the server could not be reached",
            Self::Failed => "the connection failed",
            Self::BodyTooLarge => "the answer was larger than is read",
            Self::Lost => "the answer to a change was lost",
            Self::Certificate => "the server's certificate is not trusted",
        })
    }
}
impl std::error::Error for TransportError {}

/// Something that performs one exchange: a native client, the browser's Fetch, or the stand-in.
pub trait HttpExchange {
    /// Performs `request` and describes the answer.
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError>;
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
