// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A signed-in JMAP account: the core's [`Client`] and the program's [`HttpExchange`] together, and the
//! requests the mail client makes through them.
//!
//! [`Connection::connect`] turns what the owner typed into the address of the session
//! ([`session_address`]: `https`, and the well-known path for a bare host), fetches the session, and keeps a
//! [`Client`] for the account the session names for mail. Beyond what the core checks of a session, the
//! mail client holds it to two rules of its own:
//!
//! - **One origin.** Every address the session names (the API, the downloads, the uploads) is on the origin
//!   the session is served from: the credentials go with each request, and they go to that server and to
//!   no other ([`Session::endpoints_share_origin`]).
//! - **Mail.** The session offers mail, and an account for it ([`Session::mail_account`]).
//!
//! The well-known address may send the client on (RFC 8620 §2.2): a redirect to another address on the
//! same origin is followed, [`MAX_REDIRECTS`] at most, and one to another origin comes back as
//! [`Error::Redirected`], for the caller to decide whether the credentials may go there. Nothing else
//! follows a redirect, and an [`HttpExchange`] follows none (see [`http`](crate::http)).
//!
//! Each request carries the most its answer can need ([`ApiRequest::set_response_limit`], the `most` of a
//! download), since what is parsed out of an answer takes many times its size. The exchange reads that limit
//! before the body ([`HttpRequest::max_response_bytes`](crate::http::HttpRequest::max_response_bytes)) and
//! stops as soon as the body passes it; either way the answer is refused, unread
//! ([`Unusable::BodyTooLarge`]).
//! What a server says is cleaned with the [`Redactor`] the caller gives, which knows at least what the
//! client's knows ([`Client::redactor`]) and, for a run, the account's address and the session's as the
//! owner typed it.
//!
//! A server that asks to be left alone gives a delay in seconds or a date ([`RetryAfter`]). Nothing here reads
//! the clock, so a date is no number of seconds here: an [`HttpExchange`] that can read the clock hands such
//! an answer over with the seconds until that date instead (RFC 9110 §10.2.3 allows either form).

use std::fmt;

use crate::address::Address;
use crate::classify::judge_response;
use crate::client::{Client, fetch_session};
use crate::credentials::Credentials;
use crate::discovery::session_address;
use crate::error::{BuildError, Error, Unusable};
use crate::http::HttpExchange;
use crate::id::Id;
use crate::redact::Redactor;
use crate::request::ApiRequest;
use crate::response::{ApiResponse, CallFailure};
use crate::retry::RetryAfter;
use crate::session::{CAPABILITY_MAIL, Session};

/// Redirects of the session to another address on its own origin that are followed.
pub const MAX_REDIRECTS: usize = 4;

/// Why a step of a run, or of a fetch, came to nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// The request as a whole, or the exchange it went in: what the core made of it.
    Request(Error),
    /// The request was answered, and this one call of it failed: the server answered it with an error, with
    /// nothing, or with something other than the method's answer.
    Call(CallFailure),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Request(error) => fmt::Display::fmt(error, f),
            Failure::Call(failure) => fmt::Display::fmt(failure, f),
        }
    }
}

impl std::error::Error for Failure {}

impl From<Error> for Failure {
    fn from(error: Error) -> Failure {
        Failure::Request(error)
    }
}

impl From<CallFailure> for Failure {
    fn from(failure: CallFailure) -> Failure {
        Failure::Call(failure)
    }
}

impl From<BuildError> for Failure {
    fn from(error: BuildError) -> Failure {
        Failure::Request(Error::Build(error))
    }
}

/// An answer the mail client can't use, in its own words.
pub(crate) fn unusable(what: &'static str) -> Error {
    Error::Unusable(Unusable::Shape(what))
}

/// How many seconds a busy server asked to be left alone, when `error` says it is busy and the server said
/// so in seconds (one at least). See the module documentation for a date.
pub fn busy_seconds(error: &Error) -> Option<u32> {
    match error.retry_after()? {
        RetryAfter::Delay(delay) => Some(u32::try_from(delay.as_secs()).unwrap_or(u32::MAX).max(1)),
        RetryAfter::At(_) => None,
    }
}

/// A signed-in JMAP account: the client of its session and the exchange it is reached through.
pub struct Connection<H> {
    client: Client,
    http: H,
}

/// The client's debug form: the kind of sign-in, how many capabilities and the limits. No address (a domain of
/// one's own names its owner), no account id (some servers make it of the account's address) and no
/// credential.
impl<H> fmt::Debug for Connection<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Connection").field("client", &self.client).finish_non_exhaustive()
    }
}

impl<H: HttpExchange> Connection<H> {
    /// Fetch the session at what the owner typed (`typed`, see [`session_address`]) through `http`, signed
    /// in with `credentials`, and keep a client for its mail account. See the module documentation for what
    /// the session must be.
    pub async fn connect(http: H, typed: &str, credentials: Credentials) -> Result<Connection<H>, Error> {
        let mut address = session_address(typed)?;
        let mut redirects = 0usize;
        let session = loop {
            match fetch_session(&http, &address, &credentials).await {
                Err(Error::Redirected { target, same_origin: true }) if redirects < MAX_REDIRECTS => {
                    redirects = redirects.saturating_add(1);
                    address = target;
                }
                fetched => break fetched?,
            }
        };
        Connection::with_session(http, session, credentials)
    }

    /// A connection for a session fetched already (from the address it says, [`Session::address`]).
    pub fn with_session(http: H, session: Session, credentials: Credentials) -> Result<Connection<H>, Error> {
        if !session.endpoints_share_origin() {
            return Err(unusable("the session sends the credentials to another server than its own"));
        }
        if !session.has_capability(CAPABILITY_MAIL) {
            return Err(Error::NoMailAccount);
        }
        let client = Client::new(session, credentials)?;
        log::debug!("jmap: signed in, the session offers mail");
        Ok(Connection { client, http })
    }

    /// The client of the session.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The exchange the account is reached through.
    pub fn http(&self) -> &H {
        &self.http
    }

    /// The address of the session.
    pub fn session_address(&self) -> &Address {
        self.client.session().address()
    }

    /// Send a request of method calls and read its answer, held to the request's own limit
    /// ([`ApiRequest::set_response_limit`]). What a server says in an error is cleaned with `redactor`.
    pub(crate) async fn call(&self, request: &ApiRequest, redactor: &Redactor) -> Result<ApiResponse, Error> {
        let sent = self.client.api_request(request)?;
        let answer = self.http.exchange(&sent).await?;
        judge_response(&sent, &answer, redactor)?;
        ApiResponse::parse(answer.body(), redactor)
    }

    /// Download the blob `blob` (RFC 8620 §6.2), asking for it as `media_type` and `name`: its octets, `most`
    /// of them at the most. A blob the server doesn't have is [`Error::NotFound`].
    pub(crate) async fn download(&self, blob: &Id, media_type: &str, name: &str, most: usize, redactor: &Redactor) -> Result<Vec<u8>, Error> {
        let sent = self.client.download_request(blob, media_type, name)?.with_response_limit(most);
        // The address names the account (a person's address, for some servers): it is hidden as the
        // session's is, should a server say it back.
        let mut redactor = redactor.clone();
        redactor.add_personal(sent.url());
        let answer = self.http.exchange(&sent).await?;
        judge_response(&sent, &answer, &redactor)?;
        Ok(answer.into_body())
    }
}

#[cfg(test)]
pub(crate) mod tests;
