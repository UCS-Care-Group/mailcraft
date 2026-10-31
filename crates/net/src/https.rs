// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One HTTPS exchange with an Exchange server (#252): Autodiscover and Exchange Web Services.
//!
//! The rules are those of every connection of this crate (TLS, or plain only to this machine;
//! no redirect followed, no cookie, nothing logged of the request, answers read within limits),
//! with one addition for the servers organisations run themselves, whose certificates the system
//! often doesn't know: [`Request::trusted`] may hold the SHA-256 fingerprint of the one
//! certificate the owner chose to trust for the account. The certificate is then accepted when
//! the system trusts it **or** when it is exactly that one, in date and for the server's name
//! ([`crate::pinned`]). It is never "ignore errors":
//!
//! - an expired, not-yet-valid or wrong-name certificate is refused whatever the fingerprint,
//!   and is not offered for trust ([`CertificateProblem::Unacceptable`]);
//! - a certificate the system doesn't trust and the account doesn't know is refused with what
//!   setup shows the owner ([`CertificateProblem::NotTrusted`]: fingerprint, subject, issuer,
//!   dates), before any request, so no password has gone;
//! - a different certificate than the trusted one is refused and asks again
//!   ([`CertificateProblem::Changed`]).
//!
//! The call blocks; a host calls it on a background thread.
//!
//! [`send`] makes a connection for each exchange. NTLM (#252, E4) signs a **connection** in, in
//! three legs, so there is also a [`Connection`]: one TLS connection that carries several
//! requests one after another (keep-alive), under the same rules, and that says the hash of the
//! server's certificate for channel binding ([`Connection::end_point_hash`]). Mail, pictures and
//! everything else keep a connection for each exchange.

use std::time::Duration;

use crate::NetError;
use crate::cert::{self, CertificateProblem};
use crate::http::{self, Request as HttpRequest, Url};

/// How long a request has to connect and to be answered.
const PATIENCE: Duration = Duration::from_secs(30);

/// A request to an Exchange server.
#[derive(Clone, Copy)]
pub struct Request<'a> {
    /// `GET` or `POST`.
    pub method: &'a str,
    /// An `https://` address (`http://` only to this machine, for tests). No user name in it.
    pub url: &'a str,
    /// The header fields to send, `Authorization` among them. Sent exactly; nothing here logs them.
    pub fields: &'a [(&'a str, &'a str)],
    /// The body.
    pub body: &'a [u8],
    /// The most body octets to read of the answer.
    pub max_response: usize,
    /// The SHA-256 fingerprint (64 hexadecimal digits) of the one certificate the owner chose to
    /// trust for this account although the system doesn't; empty for none.
    pub trusted: &'a str,
}

impl std::fmt::Debug for Request<'_> {
    /// The method and the body's length: no address, no credential, no content.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request").field("method", &self.method).field("body_len", &self.body.len()).finish()
    }
}

/// An answer: the status, the few header fields that are read, and the body.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// The body (within [`Request::max_response`]).
    pub body: Vec<u8>,
    /// `Location`, as written.
    pub location: Option<String>,
    /// Every `WWW-Authenticate`, in order (at most 16, each short).
    pub challenges: Vec<String>,
    /// `Retry-After`.
    pub retry_after: Option<String>,
    /// `X-EWS-Policy-Reason`.
    pub policy_reason: Option<String>,
}

impl std::fmt::Debug for Response {
    /// Status and body length: the body may hold someone's mail.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response").field("status", &self.status).field("body_len", &self.body.len()).finish()
    }
}

/// Performs `request` and returns the answer, whatever its status.
///
/// An error is a [`NetError`]: [`NetError::Certificate`] when the server's certificate is not
/// accepted (nothing of the request went), [`NetError::Config`] for an address or a fingerprint
/// that is no such thing, and the usual connection and protocol errors otherwise.
pub fn send(request: &Request<'_>) -> Result<Response, NetError> {
    let url = Url::parse(request.url)?;
    let pin = pin_of(request.trusted)?;
    let http_request =
        HttpRequest { method: request.method, headers: request.fields, body: Some(request.body), patience: PATIENCE, max_body: request.max_response };
    if !url.secure {
        return http::request(&url, &http_request).map(response);
    }
    let (config, verifier) = crate::tls::pinned_config(pin).map_err(|reason| NetError::Tls { host: url.host.clone(), reason })?;
    match http::request_with(&url, &http_request, Some(config)) {
        Ok(reply) => Ok(response(reply)),
        // A handshake that stopped at the certificate says which, and why; any other failure is the usual one.
        Err(error @ NetError::Tls { .. }) => Err(verifier.take_problem().map_or(error, |problem| NetError::Certificate(Box::new(problem)))),
        Err(other) => Err(other),
    }
}

/// One connection to an Exchange server that carries several requests, one after another: the
/// three legs of an NTLM sign-in, and the requests after them, which the server ties to the
/// connection.
///
/// It is made as [`send`] makes its own (TLS, or plain only to this machine; the owner's trusted
/// certificate; no redirect followed), and the certificate is checked before the first request
/// goes. A request on it goes only to the server it was made for. It stays open as long as each
/// answer says where it ends and the server doesn't close; after an answer that doesn't allow
/// another request, [`Connection::is_open`] is false, [`Connection::send`] says the connection is
/// closed ([`is_closed`]), and the host makes a new one (and signs in again). A server may also
/// close an idle connection: the next request then fails the same way.
///
/// **Retrying.** A host that reconnects after a failure must retry only a request that is the same
/// however often it is made, or one for which nothing was written. A request whose answer was lost
/// after it went out (`CreateItem`, `SendItem`) may have happened: it is never sent again here.
pub struct Connection {
    stream: crate::transport::Stream<'static>,
    url: Url,
    open: bool,
}

impl std::fmt::Debug for Connection {
    /// Whether it is open and encrypted: no address, nothing of what went over it.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection").field("open", &self.open).field("secure", &self.url.secure).finish()
    }
}

/// The SHA-256 fingerprint `trusted` names, if any.
fn pin_of(trusted: &str) -> Result<Option<[u8; 32]>, NetError> {
    match trusted.trim() {
        "" => Ok(None),
        fingerprint => cert::parse_fingerprint(fingerprint)
            .map(Some)
            .ok_or_else(|| NetError::Config("the trusted certificate's fingerprint isn't a SHA-256 fingerprint".into())),
    }
}

impl Connection {
    /// Connects to `url`'s server (an `https://` address; `http://` only to this machine) and, for
    /// TLS, shakes hands: a certificate that isn't accepted is [`NetError::Certificate`], as for
    /// [`send`], before anything is sent. `trusted` is the fingerprint the owner chose to trust
    /// ([`Request::trusted`]), or empty.
    pub fn open(url: &str, trusted: &str) -> Result<Connection, NetError> {
        let url = Url::parse(url)?;
        let pin = pin_of(trusted)?;
        if !url.secure {
            return Ok(Connection { stream: http::connect(&url, PATIENCE)?, url, open: true });
        }
        let (config, verifier) = crate::tls::pinned_config(pin).map_err(|reason| NetError::Tls { host: url.host.clone(), reason })?;
        match http::connect_with(&url, PATIENCE, Some(config)) {
            Ok(stream) => Ok(Connection { stream, url, open: true }),
            Err(error @ NetError::Tls { .. }) => Err(verifier.take_problem().map_or(error, |problem| NetError::Certificate(Box::new(problem)))),
            Err(other) => Err(other),
        }
    }

    /// Whether the connection may carry another request.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The `tls-server-end-point` hash (RFC 5929) of the certificate the server showed, for
    /// channel binding; none on a connection that isn't TLS, or for a certificate signed in a way
    /// that names no hash (the host then sends no binding).
    pub fn end_point_hash(&self) -> Option<Vec<u8>> {
        cert::end_point_hash(&self.stream.server_certificate()?)
    }

    /// Sends `request` on this connection and returns the answer, whatever its status. The
    /// request's address must be this connection's server (its path is the request's own);
    /// [`Request::trusted`] is not used (the connection was made with it).
    pub fn send(&mut self, request: &Request<'_>) -> Result<Response, NetError> {
        if !self.open {
            return Err(http::closed());
        }
        let url = Url::parse(request.url)?;
        if !url.same_server(&self.url) {
            return Err(NetError::Config("a held connection carries requests to its own server alone".into()));
        }
        let http_request = HttpRequest {
            method: request.method,
            headers: request.fields,
            body: Some(request.body),
            patience: PATIENCE,
            max_body: request.max_response,
        };
        // Whatever goes wrong, the connection is not used again.
        self.open = false;
        let (reply, reusable) = http::exchange(&mut self.stream, &url, &http_request, true)?;
        self.open = reusable;
        Ok(response(reply))
    }
}

/// Whether `error` says that the server closed the connection (or never took the request): a
/// held connection that did needs a new one.
pub fn is_closed(error: &NetError) -> bool {
    http::is_closed(error)
}

/// Eight random bytes from the system's generator: the client challenge of an NTLM sign-in.
pub fn client_challenge() -> Result<[u8; 8], NetError> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 8];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| NetError::Config("this computer couldn't make the random numbers a sign-in is protected with".into()))?;
    Ok(bytes)
}

fn response(reply: http::Reply) -> Response {
    Response {
        status: reply.status,
        body: reply.body,
        location: reply.location,
        challenges: reply.challenges,
        retry_after: reply.retry_after,
        policy_reason: reply.policy_reason,
    }
}

/// What a person is shown about a certificate (re-exported for hosts).
pub use crate::cert::CertificateInfo;

/// The problem with the server's certificate behind `error`, when that is what it is.
pub fn certificate_problem(error: &NetError) -> Option<&CertificateProblem> {
    match error {
        NetError::Certificate(problem) => Some(problem.as_ref()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "https_tests.rs"]
mod https_tests;
