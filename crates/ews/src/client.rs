// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The client: an endpoint, a credential and a version, and the operations over any
//! [`HttpExchange`].

use crate::error::Error;
use crate::folder::{DistinguishedFolder, FolderInfo, GetFolder};
use crate::http::{HttpExchange, HttpRequest, Secret, check_endpoint};
use crate::reply::Reply;
use crate::soap::{Element, Version, envelope};

/// Speaks to one Exchange Web Services endpoint.
#[derive(Clone)]
pub struct Client {
    endpoint: String,
    authorization: Secret,
    version: Version,
}

impl std::fmt::Debug for Client {
    /// The version alone: no address and no credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("version", &self.version).finish()
    }
}

impl Client {
    /// A client for the service at `endpoint` (`https://mail.example.org/EWS/Exchange.asmx`; plain
    /// `http` only to this machine), sending `authorization` as the `Authorization` field.
    pub fn new(endpoint: &str, authorization: Secret) -> Result<Self, Error> {
        check_endpoint(endpoint)?;
        Ok(Self { endpoint: endpoint.to_owned(), authorization, version: Version::default() })
    }

    /// Asks for the schema of `version`.
    #[must_use]
    pub fn with_version(mut self, version: Version) -> Self {
        self.version = version;
        self
    }

    /// The request for `operation`, ready to send.
    pub fn request(&self, operation: &Element) -> Result<HttpRequest, Error> {
        Ok(HttpRequest::soap(&self.endpoint, &self.authorization, envelope(self.version, operation))?)
    }

    /// Sends `operation` and judges the answer to the point of its response messages.
    pub async fn call<E: HttpExchange>(&self, exchange: &E, operation: &Element) -> Result<Reply, Error> {
        self.send(exchange, self.request(operation)?).await
    }

    /// [`Client::call`] for an operation that makes or sends something (`CreateItem`, `SendItem`,
    /// a move): the request is marked so that the program never sends it twice
    /// ([`HttpRequest::once`]), and an answer lost after it went out is [`TransportError::Lost`].
    pub async fn call_once<E: HttpExchange>(&self, exchange: &E, operation: &Element) -> Result<Reply, Error> {
        self.send(exchange, self.request(operation)?.once()).await
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
