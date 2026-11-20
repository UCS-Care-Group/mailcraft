// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A search of a Microsoft account's mailbox on its server (R6.2): More results on the server.
//!
//! The engine says what to ask for ([`GraphSearchRequest`], one page of one mailbox), a host with
//! sockets asks Microsoft Graph (`mailcraft_net::graph::search_messages`), and the engine takes
//! in what came back ([`GraphSearchResult`]): server results, held in the session and never in
//! the mailbox until the owner opens one. Nothing is asked until the owner asks for it.
//!
//! The query is the owner's own words: it is never written into a log line, and neither request
//! nor result shows it, or anything of a message, in debug output.

use serde::{Deserialize, Serialize};

use super::{GraphMessage, SharedRefusal, is_false};
use crate::Id;

/// One page of a search of one mailbox on its server.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphSearchRequest {
    pub account: Id,
    /// The mailbox's address: the mailbox whose requests are counted together, and what the
    /// answer is checked against.
    pub email: String,
    /// `email` is a shared mailbox the owner of the token may open (#70): it is asked at
    /// `/users/{email}/…`, never `/me/…`.
    #[serde(skip_serializing_if = "is_false")]
    pub shared: bool,
    /// Which search of the session this is for: an answer for an earlier one is not taken in.
    pub ticket: u64,
    /// The server id of the folder searched; empty for the whole mailbox.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// The search as the owner typed it. What of it Microsoft's search understands goes to the
    /// server; the rest is checked here against what comes back
    /// ([`crate::search::Query::for_server`]).
    pub query: String,
    /// Where the next page is, as the server named it; empty for the first.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub next: String,
}

/// The query and the link stay out of debug output.
impl std::fmt::Debug for GraphSearchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphSearchRequest")
            .field("account", &self.account)
            .field("shared", &self.shared)
            .field("ticket", &self.ticket)
            .field("folder", &!self.folder.is_empty())
            .field("next", &!self.next.is_empty())
            .finish()
    }
}

/// What one page of a search brought back.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GraphSearchResult {
    pub account: Id,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    pub ticket: u64,
    /// The messages found, newest first, as a folder's delta gives them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<GraphMessage>,
    /// Where the next page is; empty when this was the last (or nothing came).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub next: String,
    /// What went wrong, in the words the owner is told; empty when all went well.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// The server couldn't be reached.
    #[serde(skip_serializing_if = "is_false")]
    pub offline: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub busy_for: Option<u32>,
    /// A shared mailbox that won't be opened for the owner, by Graph's own code.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "super::lenient")]
    pub refused: Option<SharedRefusal>,
    /// More of this mailbox is still being searched (another folder, on the same connection).
    /// The messages so far are already the ones to show. The last answer of the mailbox has this
    /// false.
    #[serde(skip_serializing_if = "is_false")]
    pub partial: bool,
    /// The search stopped at its limit while the server still had matches. The messages so far
    /// are what is shown, and the list says there may be more.
    #[serde(skip_serializing_if = "is_false")]
    pub capped: bool,
}

/// What came stays out of debug output: counts only.
impl std::fmt::Debug for GraphSearchResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphSearchResult")
            .field("account", &self.account)
            .field("ticket", &self.ticket)
            .field("messages", &self.messages.len())
            .field("next", &!self.next.is_empty())
            .field("errors", &self.errors.len())
            .field("offline", &self.offline)
            .field("busy_for", &self.busy_for)
            .field("refused", &self.refused)
            .field("partial", &self.partial)
            .field("capped", &self.capped)
            .finish()
    }
}

/// Most folders one search of an Exchange server account names, and most mailboxes one of a JMAP
/// account says an email may lie in.
pub const MAX_SEARCH_FOLDERS: usize = 1_000;

/// A search of an Exchange server account's mailbox on its server (#1725): `FindItem` in the
/// folders named, newest first, a page at a time. The counterpart of [`GraphSearchRequest`] for a
/// host that has to be told where the server is; what it brings back is a [`GraphSearchResult`].
/// The password goes beside the request and is not in it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EwsSearchRequest {
    pub account: Id,
    /// The account's address: what the answer is checked against.
    pub email: String,
    /// The address of the Exchange Web Services endpoint. Never logged.
    pub url: String,
    /// The name the password is sent with; empty means `email`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub username: String,
    /// The fingerprint of the one certificate the owner trusts for this account; empty when none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub trusted: String,
    /// Which search of the session this is for.
    pub ticket: u64,
    /// The server ids of the folders to search, in the order they are asked: at most
    /// [`MAX_SEARCH_FOLDERS`].
    pub folders: Vec<String>,
    /// The search as the owner typed it ([`crate::search::Query::for_server`] says what of it goes).
    pub query: String,
    /// Where the search goes on from, as the last answer said ([`GraphSearchResult::next`]);
    /// empty for the first page.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub next: String,
}

/// Nothing of the server, the owner or the search: counts only.
impl std::fmt::Debug for EwsSearchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwsSearchRequest")
            .field("account", &self.account)
            .field("ticket", &self.ticket)
            .field("folders", &self.folders.len())
            .field("next", &!self.next.is_empty())
            .finish()
    }
}

/// A mailbox of a JMAP account an email may be said to lie in, with the kind of folder it is here:
/// an email in several mailboxes is listed in the first of them, in the order a run uses.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchPlace {
    /// The mailbox's server id.
    pub id: String,
    pub kind: crate::FolderKind,
}

/// A search of a JMAP account's mailbox on its server (#1725): `Email/query` with a filter, newest
/// first, a page at a time (RFC 8621 §4.4). What it brings back is a [`GraphSearchResult`]. The
/// secret that opens the server goes beside the request and is not in it.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JmapSearchRequest {
    pub account: Id,
    /// The account's address: what the answer is checked against.
    pub email: String,
    /// The address of the server's session resource. It may carry the account's address, so it
    /// is never logged.
    pub session: String,
    /// The secret that goes beside this request is a bearer token; otherwise the password.
    #[serde(skip_serializing_if = "is_false")]
    pub bearer: bool,
    /// Which search of the session this is for.
    pub ticket: u64,
    /// The server id of the one mailbox searched; empty for the whole account.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub folder: String,
    /// For a search of the whole account: the mailboxes whose mail stays out of it (Deleted Items
    /// and Junk E-mail), by server id.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outside: Vec<String>,
    /// The mailboxes this computer has for the account, at most [`MAX_SEARCH_FOLDERS`]: an email
    /// that lies in none of them is passed over.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub places: Vec<SearchPlace>,
    /// The search as the owner typed it.
    pub query: String,
    /// Where the search goes on from ([`GraphSearchResult::next`]); empty for the first page.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub next: String,
}

/// Nothing of the server, the owner or the search: counts only.
impl std::fmt::Debug for JmapSearchRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JmapSearchRequest")
            .field("account", &self.account)
            .field("ticket", &self.ticket)
            .field("folder", &!self.folder.is_empty())
            .field("outside", &self.outside.len())
            .field("places", &self.places.len())
            .field("next", &!self.next.is_empty())
            .finish()
    }
}

#[cfg(test)]
#[path = "search_requests_tests.rs"]
mod requests_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_shows_nothing_of_the_search_or_of_what_it_found() {
        let request = GraphSearchRequest { query: "budget from:ada".into(), next: "https://graph.example/x?$skip=50".into(), ..Default::default() };
        let shown = format!("{request:?}");
        assert!(!shown.contains("budget") && !shown.contains("skip"), "{shown}");
        let found = GraphMessage { subject: "Quarterly budget".into(), ..Default::default() };
        let result = GraphSearchResult { messages: vec![found], ..Default::default() };
        assert!(!format!("{result:?}").contains("budget"));
        // And it goes through JSON as it is.
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(serde_json::from_str::<GraphSearchResult>(&json).unwrap(), result);
    }
}
