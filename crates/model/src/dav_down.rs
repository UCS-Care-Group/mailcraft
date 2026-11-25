// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a host asks of a calendar server, and what it brings back (#209, spec C5, ADR 0026): plain
//! data between the engine (`sync.davRequest`, `sync.applyDav`) and a host with sockets
//! (`mailcraft-net`), as [`crate::remote`] is for Microsoft Graph.
//!
//! Everything in a [`DavDownload`] is hostile: it comes from a server through a host, or from a
//! command's parameters. The engine cuts it to size and checks every address against the
//! collection it claims to be in before any of it is kept. `Debug` shows counts, never an
//! address, a name or a document.

use serde::{Deserialize, Serialize};

use crate::{Id, Rgb};

/// Most resources one collection is taken to hold (spec Q18); what is beyond is left out and said.
pub const MAX_RESOURCES: usize = 20_000;
/// Most resources of all of an account's collections together.
pub const MAX_ACCOUNT_RESOURCES: usize = 100_000;
/// Largest document whose raw text is kept (ADR 0026); a larger one is shown from its parsed fields
/// and never written back.
pub const MAX_RAW: usize = 1024 * 1024;
/// Largest document that is read at all (a read-only event is shown from it).
pub const MAX_READ: usize = 2 * MAX_RAW;
/// Most raw text kept for one account (ADR 0026).
pub const MAX_ACCOUNT_RAW: u64 = 64 * 1024 * 1024;
/// Most changes one collection's answer carries.
pub const MAX_CHANGES: usize = 5_000;
/// Longest address kept: what an item's `Origin::id` can hold.
pub const MAX_HREF: usize = crate::remote::MAX_KEY;
/// Most known resources a request names (a collection read without a token).
pub const MAX_KNOWN: usize = MAX_RESOURCES;
/// Most bytes of event text (titles, places, notes, people, categories, addresses) an account's
/// calendars from its server hold here, all together: what is over is not taken in, and said.
pub const MAX_ACCOUNT_TEXT: usize = 64 * 1024 * 1024;

/// Which collection to ask what, as the engine says it: the state kept at the last complete read,
/// and what is here already (so that a read, or a read that goes on after one that stopped short,
/// fetches only what is new or changed).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavAskCollection {
    /// The collection's address, absolute.
    pub href: String,
    /// Where its changes go on from (RFC 6578); empty before the first complete read.
    pub sync_token: String,
    /// Its change tag, for a server without sync tokens.
    pub ctag: String,
    /// Each resource that is here, by address, with the ETag it was read at: one under the same
    /// ETag is not fetched again. (One the server gave no ETag for is left out once a day, so that
    /// a read fetches it again: nothing else would show that it changed.)
    pub known: Vec<DavKnown>,
}

/// A resource that is in the mailbox already.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavKnown {
    pub href: String,
    /// The ETag the resource's events were kept under. Empty for a resource the server gave none
    /// for: a writer (D6) that sends `If-Match` must know it has none, never send an empty one.
    pub etag: String,
}

/// What a host is to ask an account's calendar server (`sync.davRequest`).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavAsk {
    pub account: Id,
    /// The account's address: a download is not taken in by another account that has the id.
    pub email: String,
    /// The name that signs in (the settings', else the account's address).
    pub username: String,
    /// The SHA-256 fingerprint of the one certificate the owner chose to trust, or empty.
    pub trusted: String,
    /// The calendar homes of the settings.
    pub homes: Vec<String>,
    /// List the homes' collections before reading them (first run, or when it is time to look for
    /// calendars that came or went).
    pub list_homes: bool,
    /// The settings the read is made under ([`crate::DavServer::identity`]), handed back in the
    /// download: one read under settings that have changed since is not taken in.
    pub server: String,
    /// The collections already known, with their state: only those that have a calendar here.
    pub collections: Vec<DavAskCollection>,
    /// The mailbox has no room for another calendar: a listing reads only `collections`, never
    /// one it finds that isn't among them.
    pub no_room: bool,
    /// The account's settings were put away or changed this many times this session: handed
    /// back in the download, so that one read before an off and on again is not taken in.
    pub generation: u64,
}

impl std::fmt::Debug for DavAsk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavAsk")
            .field("homes", &self.homes.len())
            .field("list_homes", &self.list_homes)
            .field("collections", &self.collections.len())
            .finish()
    }
}

/// A calendar the server lists.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavCalendar {
    pub href: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    /// The owner may not change what is in it (`current-user-privilege-set`); a server that says
    /// nothing is taken as read-only.
    pub read_only: bool,
    /// What it holds (`VEVENT`), when it said.
    pub components: Vec<String>,
}

/// The calendars found in the homes.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavListing {
    pub calendars: Vec<DavCalendar>,
    /// Every home was listed to its end. Only then does a calendar that isn't listed go.
    pub complete: bool,
    /// More collections came than are kept.
    pub capped: bool,
}

/// One resource that changed, or is gone.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavChange {
    /// Its address, absolute, inside its collection.
    pub href: String,
    /// The server's mark of this version.
    pub etag: String,
    /// The document as the server sent it; none for a resource that is gone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
    /// The server says it is no longer there.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub gone: bool,
}

/// What a read of one collection came to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DavReadState {
    /// Nothing was asked about it (the run ended first), or it couldn't be read: state stays.
    #[default]
    Skipped,
    /// Its token or change tag is what is kept: nothing changed.
    Unchanged,
    /// What changed, or all of it, is in `changes`.
    Read,
}

/// One collection's part of a download.
///
/// **What may remove a resource here.** Only two things are evidence that the server no longer
/// has one: a change that says it is `gone` (the server's own 404 in a `sync-collection`), and
/// `members` (a listing of the whole collection that went to its end with nothing left out).
/// Nothing else is: not a read that stopped short, not a time range, not a recurrence worked out
/// here.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavRead {
    pub href: String,
    pub state: DavReadState,
    /// Every new or changed resource is in `changes`: the change tag may be kept.
    pub complete: bool,
    /// What to keep to go on from: a token consistent with `changes`, empty for none.
    pub sync_token: String,
    /// The change tag, at the end of a complete read.
    pub ctag: String,
    pub changes: Vec<DavChange>,
    /// The address of every resource the collection holds, from a listing of all of it
    /// (`PROPFIND`, Depth 1) that was whole: not capped, not cut short, not redirected, and with
    /// every member understood and on the collection. What is here and isn't in it (or in
    /// `changes`) is gone. `None` when there was no such listing: then nothing is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<String>>,
}

/// Why a run stopped short, as a code the owner can act on. Never a server's own words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DavProblem {
    #[default]
    None,
    /// The server asked for a password and there is none to send.
    NeedsPassword,
    /// The server refused the password.
    WrongPassword,
    /// The server asks for a kind of sign-in this version doesn't do.
    UnsupportedSignIn,
    /// The server couldn't be reached, or the connection failed.
    Unreachable,
    /// The server's certificate isn't one the system or the owner trusts.
    Certificate,
    /// The server is busy: wait.
    Busy,
    /// It sent the run to another server, or too many times round.
    Redirected,
    /// What it said isn't a calendar server's answer.
    NotCalendarServer,
    /// It refused (403 and the like).
    Refused,
    /// The run reached its limits (requests, bytes or time) and goes on at the next.
    Limits,
    /// The server answered some requests with errors (or answers that are none): what it holds
    /// may be out of date here. Asked again at the next run.
    ServerErrors,
    /// The mailbox holds as much from this server as it may (events, their text, or calendars):
    /// what is over is not taken in. Said by the engine, never by a host.
    Full,
}

/// What a host read from an account's calendar server (`sync.applyDav`).
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DavDownload {
    pub account: Id,
    pub email: String,
    /// The settings the read was made under, as the request said ([`DavAsk::server`]).
    pub server: String,
    /// As the request said ([`DavAsk::generation`]).
    pub generation: u64,
    /// The homes' collections, when they were listed.
    pub listing: Option<DavListing>,
    pub collections: Vec<DavRead>,
    pub problem: DavProblem,
    /// Seconds the server asked to be left alone for (`Retry-After`), at most ten minutes.
    pub wait: u64,
    /// The origins that took the credential: it is known good there, for this session.
    pub signed_in: Vec<String>,
    /// Addresses of the server's that were left unread: off its origin, or not inside the
    /// collection they were named under.
    pub skipped: u32,
}

impl std::fmt::Debug for DavDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DavDownload")
            .field("listing", &self.listing.as_ref().map(|l| l.calendars.len()))
            .field("collections", &self.collections.len())
            .field("changes", &self.collections.iter().map(|c| c.changes.len()).sum::<usize>())
            .field("problem", &self.problem)
            .finish()
    }
}

macro_rules! counts_only {
    ($($t:ty),*) => {$(impl std::fmt::Debug for $t {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(stringify!($t))
        }
    })*};
}
counts_only!(DavAskCollection, DavKnown, DavCalendar, DavListing, DavChange, DavRead);

#[cfg(test)]
#[path = "dav_down_tests.rs"]
mod dav_down_tests;
