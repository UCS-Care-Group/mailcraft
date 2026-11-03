// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Exchange Web Services (EWS) without sockets and without browser APIs.
//!
//! The crate describes HTTP requests and judges the answers; the program performs each exchange
//! through an [`HttpExchange`] (an HTTP client where there are sockets, or, in the tests, the
//! in-process, strict server of `standin`, behind the `stand-in` feature). It has no I/O of its
//! own, no thread and no clock, so it builds for wasm32 unchanged.
//!
//! It is written from Microsoft's public documentation only: the EWS reference on Microsoft
//! Learn and the Open Specifications [MS-OXWSCDATA] (types), [MS-OXWSCORE] (the operations and
//! their messages) and [MS-OXWSFOLD] (folders).
//!
//! - [`soap`]: the request: a SOAP 1.1 envelope with `RequestServerVersion` and one operation,
//!   written from [`Element`]s. Every name, attribute and text is stripped of control characters
//!   (CR, LF and NUL among them) and XML-escaped.
//! - [`xml`]: the hostile reader. No DTD, no entity expansion, nesting and size and count capped,
//!   unknown elements skipped, namespaces resolved so that elements are found by namespace and
//!   local name and never by prefix.
//! - [`reply`], [`error`]: the answer: HTTP status, SOAP fault, response messages with their
//!   class and code, and `ErrorServerBusy` with a capped `BackOffMilliseconds` ([`Error::Busy`]).
//!   No error holds text a server wrote.
//! - [`autodiscover`]: finding the service's address from an e-mail address (MS-OXDSCLI), as a
//!   state machine the host drives; [`basic`]: the `Authorization: Basic` value; [`ntlm`]: NTLMv2
//!   (MS-NLMP) as three legs on one held connection; [`auth`]: NTLM or Basic, from what the
//!   server offers.
//! - [`folder`], [`client`]: `GetFolder` by distinguished name, and [`Client::check`], which
//!   tests an account with it.
//! - [`hierarchy`], [`items`], [`run`]: mail down (#252, E6): `SyncFolderHierarchy`,
//!   `SyncFolderItems` (headers and a preview first) and `GetItem` with the MIME content, each a
//!   request and a reader of its answer; and [`sync_mail`] and [`fetch_messages`], which run them
//!   over an [`HttpExchange`] and hand the run back in pieces.
//!
//! Everything a server sends is hostile: nothing indexes into what came over the wire, and a
//! server's ids stay behind types whose `Debug` shows nothing ([`FolderId`], [`Secret`]).
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod auth;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod autodiscover;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod basic;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod changes;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod client;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod compose;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod error;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod folder;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod hierarchy;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod http;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod items;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod ntlm;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod reply;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod run;
#[deny(missing_docs)]
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod soap;
mod up;
#[cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]
pub mod xml;

mod b64;

#[cfg(any(test, feature = "stand-in"))]
pub mod standin;
#[cfg(any(test, feature = "stand-in"))]
pub mod standin_autodiscover;
#[cfg(any(test, feature = "stand-in"))]
pub mod standin_mail;
#[cfg(any(test, feature = "stand-in"))]
pub mod standin_ntlm;
#[cfg(any(test, feature = "stand-in"))]
pub mod standin_write;

#[cfg(test)]
mod hostile_tests;
#[cfg(test)]
mod items_tests;
#[cfg(test)]
mod mail_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod up_tests;

pub use auth::{AuthError, Scheme};
pub use autodiscover::{Autodiscover, Consent, ConsentReason, Discovered, Failure, Outcome, SrvName, Step};
pub use basic::CredentialError;
pub use changes::{DeleteMode, ItemUpdate};
pub use client::Client;
pub use compose::Unsendable;
pub use error::{Error, PolicyReason, ResponseClass, ResponseCode};
pub use folder::{DistinguishedFolder, FolderId, FolderInfo, GetFolder};
pub use hierarchy::{HierarchyPage, SyncFolderHierarchy};
pub use http::{EndpointError, HttpExchange, HttpRequest, HttpResponse, Method, Secret, TransportError};
pub use items::{GetItemMime, ItemsPage, Shape, SyncFolderItems};
pub use ntlm::{Credentials as NtlmCredentials, NtlmError};
pub use reply::{Message, Reply};
pub use run::{failed_run, fetch_messages, sync_mail};
pub use soap::{Element, Version};
