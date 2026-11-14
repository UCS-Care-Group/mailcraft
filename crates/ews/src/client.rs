// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client: an endpoint, a credential and a version, and the operations over any
//! [`HttpExchange`].
//!
//! **The version** asked for is Exchange 2013's unless told otherwise. A server older than that
//! (Exchange 2010 knows no version after `Exchange2010_SP2`) refuses the request whole with
//! `ErrorInvalidServerVersion` (#1349), before it does anything with it; the request is then made
//! once more, asking for [`Version::FALLBACK`], and the client asks for that from then on. That is
//! one step down and no more: a server that refuses that version too gets its answer as it was.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::error::{Error, ResponseCode};
use crate::folder::{DistinguishedFolder, FolderInfo, GetFolder};
use crate::http::{HttpExchange, HttpRequest, Secret, check_endpoint};
use crate::reply::Reply;
use crate::soap::{Element, Version, envelope};

/// Speaks to one Exchange Web Services endpoint.
pub struct Client {
    endpoint: String,
    authorization: Secret,
    /// The version asked for ([`Version::index`]): lowered once when the server doesn't know it.
    version: AtomicU8,
}

impl Clone for Client {
    fn clone(&self) -> Self {
        Self { endpoint: self.endpoint.clone(), authorization: self.authorization.clone(), version: AtomicU8::new(self.version().index()) }
    }
}

impl std::fmt::Debug for Client {
    /// The version alone: no address and no credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("version", &self.version()).finish()
    }
}

impl Client {
    /// A client for the service at `endpoint` (`https://mail.example.org/EWS/Exchange.asmx`; plain
    /// `http` only to this machine), sending `authorization` as the `Authorization` field.
    pub fn new(endpoint: &str, authorization: Secret) -> Result<Self, Error> {
        check_endpoint(endpoint)?;
        Ok(Self { endpoint: endpoint.to_owned(), authorization, version: AtomicU8::new(Version::default().index()) })
    }

    /// Asks for the schema of `version`.
    #[must_use]
    pub fn with_version(self, version: Version) -> Self {
        self.version.store(version.index(), Ordering::Relaxed);
        self
    }

    /// The version the client asks for now: the one it was made with, or [`Version::FALLBACK`]
    /// once the server has said it doesn't know that one.
    pub fn version(&self) -> Version {
        Version::from_index(self.version.load(Ordering::Relaxed))
    }

    /// The request for `operation`, ready to send.
    pub fn request(&self, operation: &Element) -> Result<HttpRequest, Error> {
        self.request_in(self.version(), operation)
    }

    fn request_in(&self, version: Version, operation: &Element) -> Result<HttpRequest, Error> {
        Ok(HttpRequest::soap(&self.endpoint, &self.authorization, envelope(version, operation))?)
    }

    /// Sends `operation` and judges the answer to the point of its response messages.
    pub async fn call<E: HttpExchange>(&self, exchange: &E, operation: &Element) -> Result<Reply, Error> {
        self.negotiated(exchange, operation, false).await
    }

    /// [`Client::call`] for an operation that makes or sends something (`CreateItem`, `SendItem`,
    /// a move): the request is marked so that the program never sends it twice
    /// ([`HttpRequest::once`]), and an answer lost after it went out is [`TransportError::Lost`].
    pub async fn call_once<E: HttpExchange>(&self, exchange: &E, operation: &Element) -> Result<Reply, Error> {
        self.negotiated(exchange, operation, true).await
    }

    /// Sends `operation` in the version asked for now, and once more in [`Version::FALLBACK`] when
    /// the server says it doesn't know that version. (Making it again is safe even for a request
    /// that is never sent twice: a server judges the version in the header before it reads the
    /// operation, so nothing of it was done.)
    async fn negotiated<E: HttpExchange>(&self, exchange: &E, operation: &Element, once: bool) -> Result<Reply, Error> {
        let asked = self.version();
        let request = |version| self.request_in(version, operation).map(|r| if once { r.once() } else { r });
        match self.send(exchange, request(asked)?).await {
            Err(Error::Server(ResponseCode::InvalidServerVersion)) if asked > Version::FALLBACK => {
                // (Another request of the same client may have lowered it already: either way it is the fallback.)
                if self.version.compare_exchange(asked.index(), Version::FALLBACK.index(), Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                    log::info!("ews: the server doesn't know {}; asking for {} from now on", asked.as_str(), Version::FALLBACK.as_str());
                }
                self.send(exchange, request(Version::FALLBACK)?).await
            }
            other => other,
        }
    }

    async fn send<E: HttpExchange>(&self, exchange: &E, request: HttpRequest) -> Result<Reply, Error> {
        let response = exchange.exchange(&request).await?;
        // A bare 429 or 503 to a request that is never sent twice is no word of the service: it
        // may have been done. Only a SOAP `ErrorServerBusy`, which comes before anything is done,
        // is waited out and asked again.
        if !request.is_repeatable() && matches!(response.status(), 429 | 503) && !matches!(Reply::parse(response.body()), Err(Error::Busy { .. })) {
            return Err(Error::Status(response.status()));
        }
        let reply = Reply::judge(&response, request.max_response_bytes())?;
        match reply.busy() {
            Some(busy) => Err(busy),
            None => Ok(reply),
        }
    }

    /// The folders `get` asks for, one result each.
    pub async fn get_folders<E: HttpExchange>(&self, exchange: &E, get: &GetFolder) -> Result<Vec<Result<FolderInfo, Error>>, Error> {
        get.answer(&self.call(exchange, &get.element()).await?)
    }

    /// Tests the account: the Inbox can be had. It is `GetFolder` of the Inbox by its
    /// distinguished name, and nothing is changed by it.
    pub async fn check<E: HttpExchange>(&self, exchange: &E) -> Result<FolderInfo, Error> {
        let get = GetFolder::new(&[DistinguishedFolder::Inbox]);
        self.get_folders(exchange, &get).await?.into_iter().next().ok_or(Error::Unexpected)?
    }
}
