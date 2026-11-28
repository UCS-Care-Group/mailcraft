// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Descriptions of one HTTP exchange, without performing it: the ones the web-service crates
//! share ([`mailcraft_ews::http`]), plus this crate's own limits on what an answer may weigh.

pub use mailcraft_ews::http::{EndpointError, HttpExchange, HttpRequest, HttpResponse, Method, Secret, TransportError, check_endpoint};

/// The most an answer may weigh, in octets: the largest message fetched whole, and far more than
/// any SOAP answer the crate asks for.
pub const RESPONSE_LIMIT: usize = 32 * 1024 * 1024;
/// The most a SOAP answer may weigh.
pub const SOAP_LIMIT: usize = 16 * 1024 * 1024;
