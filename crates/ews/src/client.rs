// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client: an endpoint, a credential and a version, and the operations over any
//! [`HttpExchange`].

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::{Error, ResponseCode};
use crate::folder::{DistinguishedFolder, FolderInfo, GetFolder};
use crate::http::{HttpExchange, HttpRequest, Secret, check_endpoint};
use crate::reply::Reply;
use crate::soap::{Element, Version, envelope};

/// Speaks to one Exchange Web Services endpoint.
///
/// A server that doesn't know the version asked for (`ErrorInvalidServerVersion`: Exchange 2010
/// knows none after `Exchange2010_SP2`) is asked again with the version before it, and the
/// version it took is kept for every later request, by this client and its clones.
#[derive(Clone)]
pub struct Client {
    endpoint: String,
    authorization: Secret,
    /// The place in [`Version::ALL`] of the version asked for; it only goes down.
    version: Arc<AtomicUsize>,
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
        Ok(Self { endpoint: endpoint.to_owned(), authorization, version: Arc::new(AtomicUsize::new(Version::default().position())) })
    }

    /// Asks for the schema of `version` (and, when the server doesn't know it, the ones before).
    #[must_use]
    pub fn with_version(mut self, version: Version) -> Self {
        self.version = Arc::new(AtomicUsize::new(version.position()));
        self
    }

    /// The version requests ask for now.
    pub fn version(&self) -> Version {
        Version::ALL.get(self.version.load(Ordering::Relaxed)).copied().unwrap_or_default()
    }

    /// The request for `operation`, ready to send.
    pub fn request(&self, operation: &Element) -> Result<HttpRequest, Error> {
        self.request_as(self.version(), operation)
    }

    fn request_as(&self, version: Version, operation: &Element) -> Result<HttpRequest, Error> {
        Ok(HttpRequest::soap(&self.endpoint, &self.authorization, envelope(version, operation))?)
    }

    /// Sends `operation` and judges the answer to the point of its response messages.
    pub async fn call<E: HttpExchange>(&self, exchange: &E, operation: &Element) -> Result<Reply, Error> {
        self.call_as(exchange, operation, false).await
    }

    /// [`Client::call`] for an operation that makes or sends something (`CreateItem`, `SendItem`,
    /// a move): the request is marked so that the program never sends it twice
    /// ([`HttpRequest::once`]), and an answer lost after it went out is [`TransportError::Lost`].
    pub async fn call_once<E: HttpExchange>(&self, exchange: &E, operation: &Element) -> Result<Reply, Error> {
        self.call_as(exchange, operation, true).await
    }

    /// The call, asked again with an older version while the server doesn't know the one asked
    /// for. That fault comes from the request's header, before the operation is looked at, so
    /// nothing was done, and even a request that is never sent twice may go again with another
    /// version: it is another request. At most once for each version there is.
    async fn call_as<E: HttpExchange>(&self, exchange: &E, operation: &Element, once: bool) -> Result<Reply, Error> {
        let mut version = self.version();
        for _ in 0..Version::ALL.len() {
            let request = self.request_as(version, operation)?;
            let request = if once { request.once() } else { request };
            match self.send(exchange, request).await {
                Err(Error::Server(ResponseCode::InvalidServerVersion)) => match version.older() {
                    Some(older) => {
                        // Kept for every later request; a clone that already went lower stays there.
                        self.version.fetch_min(older.position(), Ordering::Relaxed);
                        version = older;
                    }
                    None => return Err(Error::Server(ResponseCode::InvalidServerVersion)),
                },
                answer => return answer,
            }
        }
        Err(Error::Server(ResponseCode::InvalidServerVersion))
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
