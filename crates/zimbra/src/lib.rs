// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Zimbra Collaboration server's own web service, without sockets and without browser APIs.
//!
//! The crate describes HTTP requests and judges the answers; the program performs each exchange
//! through an [`HttpExchange`] (an HTTP client where there are sockets, or, in the tests, the
//! in-process, strict server of `standin`, behind the `stand-in` feature). It has no I/O of its
//! own, no thread and no clock, so it builds for wasm32 unchanged.
//!
//! It is written from Zimbra's public documentation only: the SOAP API reference
//! (`zimbraAccount` and `zimbraMail`) and the REST API reference on the Zimbra wiki. Nothing of
//! Zimbra's own code is used or read.
//!
//! - [`soap`]: the request, a SOAP envelope in JSON with the sign-in token in its header, and the
//!   reading of its answer, a fault by its code alone; [`error`]: what can go wrong, in kinds and
//!   in our own words; [`json`] (inside): the hostile reader's helpers.
//! - [`auth`]: signing in with a password (`AuthRequest`), which gives the token.
//! - [`folders`]: the folders of mail (`GetFolderRequest`), as a complete listing.
//! - [`delta`]: what changed in the mailbox since a token (`SyncRequest`).
//! - [`items`]: messages as headers and a preview (`SearchRequest`), by folder or by id.
//! - [`fetch`]: a message whole, from its REST address, the token in a cookie.
//! - [`run`]: mail down: [`sync_mail`] and [`fetch_messages`], which run the rest over an
//!   [`HttpExchange`] and hand the run back in pieces.
//!
//! Everything a server sends is hostile: nothing indexes into what came over the wire, every list
//! is capped, and the token stays behind a type whose `Debug` shows nothing ([`Secret`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

pub mod auth;
pub mod delta;
pub mod error;
pub mod fetch;
pub mod folders;
pub mod http;
pub mod items;
mod json;
pub mod run;
pub mod soap;

#[cfg(any(test, feature = "stand-in"))]
pub mod standin;

#[cfg(test)]
mod hostile_tests;
#[cfg(test)]
mod tests;

pub use error::{Error, Fault};
pub use http::{EndpointError, HttpExchange, HttpRequest, HttpResponse, Method, Secret, TransportError};
pub use run::{fetch_messages, sync_mail};
