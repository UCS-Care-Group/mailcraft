// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! DAV collection metadata and resource snapshots. No credentials or I/O live here.
use crate::{Contact, Event, Id};
use serde::{Deserialize, Serialize};

/// One discovered calendar or address book. Permissions default to read-only until known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Collection {
    pub account: Id,
    pub href: String,
    pub name: String,
    pub contacts: bool,
    pub read_only: bool,
}

impl Default for Collection {
    fn default() -> Self {
        Self { account: 0, href: String::new(), name: String::new(), contacts: false, read_only: true }
    }
}

/// The downloaded document and the parsed fields it produced. Keeping both lets a later edit
/// change known properties without rewriting unknown ones. An ETag is opaque, including quotes.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Resource {
    pub account: Id,
    pub collection: String,
    pub href: String,
    pub etag: String,
    pub raw: String,
    pub contacts: Vec<Contact>,
    pub events: Vec<Event>,
}

#[cfg(test)]
#[path = "dav_tests.rs"]
mod tests;
