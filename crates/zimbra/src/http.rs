// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Descriptions of one HTTP exchange, without performing it.
//!
//! The crate builds an [`HttpRequest`]; the program performs it however it can (a native HTTP
//! client, a test double) and describes what came back in an [`HttpResponse`]. The program must:
//!
//! - send the method, address, header fields and body exactly as given;
//! - follow no redirect itself (a redirect would carry the sign-in somewhere else);
//! - read [`HttpRequest::max_response_bytes`] before reading the body, and stop reading as soon as
//!   the body passes it, reporting [`TransportError::BodyTooLarge`];
//! - copy the response's header fields into the response ([`HttpResponse::with_field`] keeps a
//!   small, capped set).

use std::fmt;
use std::future::Future;

/// The most an answer may weigh, in octets: the largest message fetched whole, and far more than
/// any SOAP answer the crate asks for.
pub const RESPONSE_LIMIT: usize = 32 * 1024 * 1024;
/// The most a SOAP answer may weigh.
pub const SOAP_LIMIT: usize = 16 * 1024 * 1024;
/// The longest address a request goes to.
const MAX_ADDRESS: usize = 2_048;
/// The longest header field value the response keeps; longer ones are dropped.
pub const MAX_FIELD_VALUE_LEN: usize = 8 * 1024;
/// Most header fields a request or a response holds.
pub const MAX_FIELDS: usize = 32;
/// The fields of a response that are read whatever else came: how long to wait, and where a
/// redirect goes.
pub const READ_FIELDS: [&str; 2] = ["retry-after", "location"];
/// Longest header field name kept.
const MAX_FIELD_NAME_LEN: usize = 64;

/// A secret: the sign-in token, or a header value made of it. Never shown, in `Debug` output or
/// anywhere else.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Keeps `value`. Control characters are taken out, so that it can never end a field.
    pub fn new(value: &str) -> Self {
        Self(value.chars().filter(|c| !c.is_control()).collect())
    }

    /// The value, for whoever sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether there is nothing in it.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Why an address is not one a request goes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointError {
    /// Not an `http` or `https` address, or too long, or with a character no address has.
    Malformed,
    /// Plain `http` to anything but the machine itself: an unencrypted connection carries
    /// nothing of the owner's.
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

/// Checks the transport rule on `url`: `https`, or plain `http` to `localhost`, `127.0.0.0/8` or
/// `[::1]` only (for the tests). Nothing is fetched from it.
pub fn check_endpoint(url: &str) -> Result<(), EndpointError> {
    if url.len() > MAX_ADDRESS || url.chars().any(|c| c.is_control() || c.is_whitespace() || !c.is_ascii() || c == '\\') {
        return Err(EndpointError::Malformed);
    }
    let (secure, rest) = match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => (true, rest),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => (false, rest),
        _ => return Err(EndpointError::Malformed),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(EndpointError::Malformed);
    }
    if authority.contains('@') {
        return Err(EndpointError::Credentials);
    }
    if secure {
        return Ok(());
    }
    let host = match authority.strip_prefix('[') {
        Some(inside) => inside.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    let loopback = host.eq_ignore_ascii_case("localhost") || host == "::1" || {
        let mut parts = host.split('.');
        parts.next() == Some("127")
            && parts.clone().count() == 3
            && parts.all(|part| part.parse::<u8>().is_ok() && !part.starts_with('+') && part.bytes().all(|b| b.is_ascii_digit()))
    };
    if loopback { Ok(()) } else { Err(EndpointError::NotEncrypted) }
}

/// The method of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `GET`
    Get,
    /// `POST`
    Post,
}

impl Method {
    /// The method's name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// A header field name: a token of the characters RFC 9110 allows, lower-cased; none when it is
/// empty, too long or holds anything else.
fn field_name(name: &str) -> Option<String> {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    (!name.is_empty() && name.len() <= MAX_FIELD_NAME_LEN && name.chars().all(token)).then(|| name.to_ascii_lowercase())
}

/// A request to send.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpRequest {
    method: Method,
    url: String,
    /// Names as given (a name that is no token is left out), values without control characters.
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    max_response: usize,
    /// Doing it twice is the same as doing it once (a read, a set of a mark). False for a request
    /// that makes or sends something: if its answer is lost after it went out, it may have
    /// happened, and it is never sent again.
    repeatable: bool,
}

impl HttpRequest {
    /// A request of `method` to `url` with the header `fields` (at most [`MAX_FIELDS`]; a name that
    /// is no token is left out, and control characters are taken out of a value) and `body`.
    pub fn new(method: Method, url: &str, fields: &[(&str, &str)], body: Vec<u8>) -> Result<Self, EndpointError> {
        check_endpoint(url)?;
        let headers = fields
            .iter()
            .filter(|(name, _)| field_name(name).is_some())
            .take(MAX_FIELDS)
            .map(|(name, value)| ((*name).to_owned(), value.chars().filter(|c| !c.is_control()).collect()))
            .collect();
        Ok(Self { method, url: url.to_owned(), headers, body, max_response: RESPONSE_LIMIT, repeatable: true })
    }

    /// Lowers the largest response body accepted to `bytes` (it can only go down).
    #[must_use]
    pub fn with_response_limit(mut self, bytes: usize) -> Self {
        self.max_response = bytes.min(self.max_response);
        self
    }

    /// Marks the request as one that is never sent twice: when its answer is lost after it went
    /// out, the program must not send it again ([`TransportError::Lost`]).
    #[must_use]
    pub fn once(mut self) -> Self {
        self.repeatable = false;
        self
    }

    /// Whether the program may send the request again after a failure of the connection.
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
        self.headers.iter().map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// The value of the first header field called `name` (without regard to case).
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields().find(|(have, _)| have.eq_ignore_ascii_case(name)).map(|(_, value)| value)
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
    /// The method and the body's length: no address, no token, no content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("method", &self.method)
            .field("body_len", &self.body.len())
            .field("max_response_bytes", &self.max_response)
            .finish()
    }
}

/// What came back, as the program describes it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct HttpResponse {
    status: u16,
    /// Names in lower case, in the order they came; at most [`MAX_FIELDS`].
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpResponse {
    /// A response with a status code and a body.
    pub fn new(status: u16, body: Vec<u8>) -> Self {
        Self { status, body, ..Self::default() }
    }

    /// Adds a header field. The set kept is small: at most [`MAX_FIELDS`] fields, a name that is a
    /// token of at most 64 characters (lower-cased), a value of at most [`MAX_FIELD_VALUE_LEN`]
    /// bytes; anything else is dropped. The fields the crate reads ([`READ_FIELDS`]) are kept
    /// whatever came before them, two of each at most.
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
        if held < if read { 2 } else { MAX_FIELDS } {
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
        let name = name.to_ascii_lowercase();
        self.headers.iter().find(|(have, _)| *have == name).map(|(_, value)| value.as_str())
    }

    /// The `Retry-After` field value, as seconds when it is a number.
    pub fn retry_after(&self) -> Option<u32> {
        self.header("retry-after").and_then(|value| value.trim().parse::<u32>().ok())
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

/// Why the program could not complete an exchange; no free text, so nothing private can travel in
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TransportError {
    /// No answer in the time the program allows.
    TimedOut,
    /// The host could not be reached (name resolution, connection).
    Unreachable,
    /// TLS failed, a certificate among the causes.
    Tls,
    /// The body was longer than [`HttpRequest::max_response_bytes`].
    BodyTooLarge,
    /// The server redirected the request and the program, as asked, did not follow.
    RedirectRefused,
    /// The exchange was cancelled.
    Aborted,
    /// The request went out (or may have) and its answer was lost; the request is not
    /// [repeatable](HttpRequest::is_repeatable), so it was not sent again and may have happened.
    Lost,
    /// Anything else.
    Other,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TimedOut => "the server did not answer in time; check the network connection and try again",
            Self::Unreachable => "the server could not be reached; check its address and the network connection",
            Self::Tls => "no secure connection could be set up, for example because the server's certificate is not trusted",
            Self::BodyTooLarge => "the answer passed the size limit set for this request and was not read further",
            Self::RedirectRefused => "the server redirected the request elsewhere and the redirect was not followed",
            Self::Aborted => "the request was cancelled before it completed",
            Self::Lost => "the connection failed after the request went out, so it may have been done",
            Self::Other => "the connection failed in a way the transport did not classify",
        })
    }
}

impl std::error::Error for TransportError {}

/// Performs one HTTP exchange.
///
/// The returned future need not be `Send`, so that a browser transport could implement it.
pub trait HttpExchange {
    /// Sends `request` and describes the response.
    fn exchange(&self, request: &HttpRequest) -> impl Future<Output = Result<HttpResponse, TransportError>>;
}
