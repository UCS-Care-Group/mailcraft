// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A Microsoft account's mailbox searched on its server (R6.2: More results on the server), one
//! page at a time, only when the owner asks for it.
//!
//! - **Asked of the server.** `GET /me/messages?$search="…"` for the whole mailbox, `GET
//!   /me/mailFolders/{id}/messages?$search="…"` for one folder; a shared mailbox's at
//!   `/users/{address}/…`, never at `/me` (#70), with the owner's token (`Mail.ReadWrite.Shared`,
//!   as for the rest of it). No permission is asked for that a run doesn't ask for already.
//! - **What goes.** The part of the owner's query that Graph's search understands
//!   ([`Query::for_server`]): words and phrases, `from:`, `to:`, `cc:`, `subject:`, `body:`,
//!   `attachment:`, `has:attachment` (Graph's `hasAttachments:true`) and `importance:high` or
//!   `low`, as clauses joined by `AND` in one value in double quotes, a phrase's own quotes
//!   written `\"` inside it (the syntax of "Use the $search query parameter"). What it doesn't
//!   understand (`-` in front of a term, `is:`, `category:`, dates, `importance:normal`) is
//!   checked by the engine against what comes back.
//! - **What is asked for.** `$select` the message list's fields ([`FIELDS`]), 50 to a page
//!   (`$top`), and nothing else: the reference shows neither `$filter` nor `$orderby` beside
//!   `$search` on messages, and `ConsistencyLevel` is for directory objects. The server sorts the
//!   results by when they were sent and gives at most 1,000; each page is sorted here newest
//!   first by arrival. The next page is the `@odata.nextLink` the server gave, followed on Graph
//!   alone, and for a shared mailbox never at `/me`.
//! - **Refusals by Graph's code**, in the words a run already has for them: a shared mailbox
//!   that won't be opened ([`NetError::SharedRefused`]), the sign-in, the permission, the server
//!   busy (the mailbox's gate pauses as for a run), the network gone (`offline`).
//! - **What comes back is hostile JSON**, read as a folder's delta is ([`super::down`]): capped,
//!   and what can't be read is passed over. The query, the address and what was found go into no
//!   log line and no error.

use std::sync::Arc;

use mailcraft_model::Importance;
use mailcraft_model::search::{Query, ServerField, ServerTerm};
use mailcraft_model::sync::{GraphSearchRequest, GraphSearchResult};
use serde_json::Value;

use super::super::pace::{Gate, Kind, Priority, Retry};
use super::super::{ASK_TIMEOUT, Doing, Graph, MAX_PAGE, graph_base};
use super::down::{FIELDS, message_of};
use super::{folders, id_of};
use crate::NetError;
use crate::http::{self, Url};

/// Results asked for to a page.
const PAGE: &str = "50";
/// Most entries taken from one page, whatever the server sends.
const MOST_ON_A_PAGE: usize = 200;
/// Longest search value sent (the clauses are few and short: [`Query::for_server`] caps them).
const MAX_VALUE: usize = 2_000;

/// One page of `request`'s search, as the owner of `token` (an access token for Graph that
/// covers the mailbox's mail).
pub fn search_messages(token: &str, request: &GraphSearchRequest) -> GraphSearchResult {
    search_messages_with(&graph_base(), token, request, Gate::for_mailbox(&request.email), Retry::STANDARD)
}

/// [`search_messages`] at `base`, through `gate`, asking again as `retry` says.
pub(crate) fn search_messages_with(base: &str, token: &str, request: &GraphSearchRequest, gate: Arc<Gate>, retry: Retry) -> GraphSearchResult {
    let mut out = GraphSearchResult { account: request.account, email: request.email.clone(), ticket: request.ticket, ..Default::default() };
    let shared = request.shared.then_some(request.email.as_str());
    let graph = match Graph::for_mail(base, token, gate, Priority::Owner, retry).and_then(|graph| graph.of_shared(shared)) {
        Ok(graph) => graph,
        Err(e) => {
            out.errors.push(format!("Couldn't reach Microsoft for the search: {e}"));
            return out;
        }
    };
    let url = if request.next.is_empty() {
        let Some(value) = search_value(&request.query) else {
            out.errors.push("Nothing of this search can be asked of Microsoft's server.".into());
            return out;
        };
        let path = if request.folder.is_empty() {
            graph.mail_path("/messages")
        } else {
            let Some(folder) = id_of(&request.folder) else {
                out.errors.push("That folder isn't one Microsoft's server can be asked about.".into());
                return out;
            };
            format!("{}/{}/messages", folders(&graph), http::encode(&folder))
        };
        let query = http::form(&[("$search", value.as_str()), ("$select", FIELDS), ("$top", PAGE)]);
        graph.url(&format!("{path}?{query}"))
    } else {
        match next_page(&graph, &request.next) {
            Some(url) => url,
            None => {
                log::info!("graph search: a next page that can't be followed was passed over");
                out.errors.push("Microsoft named a next page of results that can't be followed.".into());
                return out;
            }
        }
    };
    let reply = match graph.call(Kind::Read, &url, "GET", &[], None, ASK_TIMEOUT, MAX_PAGE) {
        Ok(reply) => reply,
        Err(e) => {
            stop(&mut out, &e);
            return out;
        }
    };
    if reply.status != 200 {
        // (`refusal` logs the status and Graph's code, never its words.)
        let e = graph.refusal(Doing::Mail, &reply, &request.query);
        stop(&mut out, &e);
        return out;
    }
    let json = serde_json::from_slice::<Value>(&reply.body).unwrap_or(Value::Null);
    let Some(entries) = json.get("value").and_then(Value::as_array) else {
        out.errors.push("Microsoft Graph's answer to the search isn't the JSON it should be.".into());
        return out;
    };
    let mut seen = std::collections::HashSet::new();
    for entry in entries.iter().take(MOST_ON_A_PAGE) {
        if let Some(message) = message_of(entry).filter(|m| seen.insert(m.server.id.clone())) {
            out.messages.push(message);
        }
    }
    out.messages.sort_by_key(|m| std::cmp::Reverse(m.received));
    if let Some(link) = json.get("@odata.nextLink").and_then(Value::as_str) {
        if next_page(&graph, link).is_some() {
            out.next = link.to_string();
        } else {
            log::info!("graph search: the server named a next page that can't be followed; the results end here");
        }
    }
    log::info!("graph search: {} found on a page of {}, more to come: {}", out.messages.len(), entries.len(), !out.next.is_empty());
    out
}

/// What a search that stopped says.
fn stop(out: &mut GraphSearchResult, e: &NetError) {
    match e {
        NetError::Busy { retry_after, .. } => out.busy_for = Some(retry_after.unwrap_or(0)),
        NetError::SharedRefused(why) => {
            out.refused = Some(*why);
            out.errors.push(format!("{}.", why.said()));
            return;
        }
        _ => {}
    }
    out.offline = e.is_unreachable();
    log::info!("graph search: the search stopped: {e}");
    out.errors.push(format!("Couldn't search on Microsoft's server: {e}"));
}

/// The next page as `link` names it, when it is one that may be asked: on Graph itself, where the
/// token belongs, and for a shared mailbox at `/users/…`, never at the owner's own `/me`.
fn next_page(graph: &Graph<'_>, link: &str) -> Option<Url> {
    let url = Url::parse_within(link, http::MAX_LONG_URL).ok().filter(|url| graph.base.same_server(url))?;
    let path = url.path.to_ascii_lowercase();
    let own = path.starts_with("/v1.0/me/") || path.starts_with("/v1.0/me?");
    match graph.shared() {
        true if own || !path.starts_with("/v1.0/users/") => None,
        _ => Some(url),
    }
}

/// The `$search` value for `query`: what of it Graph's search understands, as KQL clauses
/// joined by `AND` in double quotes. `None` when there is nothing of it to send.
pub(crate) fn search_value(query: &str) -> Option<String> {
    let parts = Query::parse(query).for_server();
    let clauses: Vec<String> = parts.terms.iter().filter_map(clause).collect();
    let value = format!("\"{}\"", clauses.join(" AND "));
    (!clauses.is_empty() && value.len() <= MAX_VALUE).then_some(value)
}

/// One term as a clause of Graph's search.
fn clause(term: &ServerTerm) -> Option<String> {
    match term {
        ServerTerm::Words { field, words } => {
            let property = match field {
                None => "",
                Some(ServerField::From) => "from:",
                Some(ServerField::To) => "to:",
                Some(ServerField::Cc) => "cc:",
                Some(ServerField::Subject) => "subject:",
                Some(ServerField::Body) => "body:",
                Some(ServerField::Attachment) => "attachment:",
            };
            // A word is letters, digits and `@._-'+` (no quote, backslash, colon, bracket or
            // star; `+` only in an address); one that is an operator of the syntax is written in
            // small letters, which it reads as a word.
            let plain = |word: &String| {
                let clean = !word.is_empty() && word.chars().all(|c| c.is_alphanumeric() || "@._-'+".contains(c));
                let operator = matches!(word.as_str(), "AND" | "OR" | "NOT" | "NEAR" | "ONEAR");
                clean.then(|| if operator { word.to_lowercase() } else { word.clone() })
            };
            let words: Vec<String> = words.iter().map(plain).collect::<Option<_>>()?;
            match words.as_slice() {
                [] => None,
                // The syntax reads `+` as an operator, and a word starting with a mark as query
                // syntax (`-ada@example.org`); in quotes they are text (#301).
                [one] if one.contains('+') || !one.starts_with(char::is_alphanumeric) => Some(format!("{property}\\\"{one}\\\"")),
                [one] => Some(format!("{property}{one}")),
                many => Some(format!("{property}\\\"{}\\\"", many.join(" "))),
            }
        }
        ServerTerm::HasAttachment => Some("hasAttachments:true".into()),
        ServerTerm::Importance(Importance::High) => Some("importance:high".into()),
        ServerTerm::Importance(Importance::Low) => Some("importance:low".into()),
        ServerTerm::Importance(Importance::Normal) => None,
    }
}
