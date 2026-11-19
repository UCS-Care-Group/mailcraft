// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A mailbox searched on its server (#1725: More results on the server), a page at a time, only
//! when the owner asks for it.
//!
//! - **Folder by folder.** `FindItem` searches one folder; the folders the request names are
//!   asked one after another, each from its newest match, until a page is full
//!   ([`PAGE`] messages), the folders are through, or [`MAX_ASKS`] requests were made. The answer
//!   says where to go on ([`GraphSearchResult::next`]: the folder, how far into it, and how the
//!   search is written), and the next request goes on from there.
//! - **How the search is written.** As a `QueryString` first ([`crate::find::query_string`]).
//!   A server that turns that down before it has answered one is asked with a `Restriction`
//!   instead ([`crate::find::restriction`]), for the rest of the search; one that turns both
//!   down is said in words. A server that doesn't know the preview and the flag is asked without.
//! - **A folder that is gone** (or that the server won't open) is passed over; the others are
//!   still asked.
//! - **What stops a search** (the network, the sign-in, a busy server) is said in a line that
//!   names neither the server nor the owner, with where to go on from, so that asking again
//!   doesn't start over.
//!
//! The query, the folders' ids and what was found go into no log line and no error.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

use mailcraft_model::search::Query;
use mailcraft_model::sync::{EwsSearchRequest, GraphSearchResult, MAX_SEARCH_FOLDERS};

use crate::client::Client;
use crate::error::ResponseCode;
use crate::find::{FindItem, MAX_FOUND, Search, query_string, restriction};
use crate::folder::valid_id;
use crate::http::HttpExchange;
use crate::items::Shape;
use crate::run::{Asked, Run, Stop};

/// Messages one answer brings, about: a page of a folder is asked for whole, so a few more may come.
pub const PAGE: usize = 50;
/// Most `FindItem` requests one answer is made of.
pub const MAX_ASKS: usize = 40;

/// Where a search goes on from: the folder, how many of its matches were read, and whether the
/// search is written as a restriction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Place {
    folder: String,
    offset: u32,
    restricted: bool,
}

impl Place {
    /// As [`GraphSearchResult::next`] carries it: the offset, `q` or `r`, and the folder's id.
    fn write(&self) -> String {
        format!("{} {} {}", self.offset, if self.restricted { 'r' } else { 'q' }, self.folder)
    }

    /// What `next` says, when it is something [`Place::write`] wrote.
    fn read(next: &str) -> Option<Place> {
        let mut parts = next.splitn(3, ' ');
        let offset = parts.next()?.parse().ok()?;
        let restricted = match parts.next()? {
            "q" => false,
            "r" => true,
            _ => return None,
        };
        let folder = parts.next().filter(|id| valid_id(id))?.to_owned();
        Some(Place { folder, offset, restricted })
    }
}

/// One page of `request`'s search. `cancel` is read before every request; `wait` sleeps the
/// milliseconds a busy server asks for and says whether to go on.
pub async fn search_messages<E: HttpExchange>(
    client: &Client,
    exchange: &E,
    request: &EwsSearchRequest,
    cancel: &AtomicBool,
    wait: &mut dyn FnMut(u32) -> bool,
) -> GraphSearchResult {
    let mut out = GraphSearchResult { account: request.account, email: request.email.clone(), ticket: request.ticket, ..Default::default() };
    let terms = Query::parse(&request.query).for_server().terms;
    let (asked, restricted) = (query_string(&terms), restriction(&terms));
    if asked.is_none() && restricted.is_none() {
        out.errors.push("Nothing of this search can be asked of the server.".into());
        return out;
    }
    // Each folder once, in the order given; an id that is none names nothing there.
    let mut seen = HashSet::new();
    let folders: Vec<&str> =
        request.folders.iter().map(String::as_str).filter(|id| valid_id(id) && seen.insert(*id)).take(MAX_SEARCH_FOLDERS).collect();
    let (mut at, mut offset, mut by_restriction) = match Place::read(&request.next) {
        // A folder that isn't asked about any more: there is nothing to go on with.
        Some(place) => match folders.iter().position(|id| *id == place.folder) {
            Some(at) => (at, place.offset, place.restricted),
            None => return out,
        },
        None => (0, 0, asked.is_none()),
    };
    let mut run = Run::new(client, exchange, cancel, wait);
    if !client.version().has_preview() {
        run.shape = Shape::Lean;
    }
    // A query string the server has answered is one it knows: a refusal after that is the folder's.
    let (mut proven, mut asks) = (false, 0usize);
    let mut ids = HashSet::new();
    while let Some(folder) = folders.get(at).copied() {
        if out.messages.len() >= PAGE || asks >= MAX_ASKS {
            break;
        }
        let search = match (by_restriction, &asked, &restricted) {
            (false, Some(query), _) => Search::Query(query.clone()),
            (_, _, Some(restriction)) => Search::Restriction(restriction.clone()),
            _ => {
                out.errors.push("This server can't be asked this search.".into());
                break;
            }
        };
        asks = asks.saturating_add(1);
        let find = FindItem::new(folder, search, offset, MAX_FOUND.min(u32::try_from(PAGE).unwrap_or(MAX_FOUND)), run.shape);
        match run.ask(&find.element(), &|reply| find.answer(reply)).await {
            Ok(page) => {
                proven |= !by_restriction;
                for mut message in page.messages {
                    if ids.insert(message.server.id.clone()) {
                        message.server.folder = folder.to_owned();
                        out.messages.push(message);
                    }
                }
                if page.last {
                    (at, offset) = (at.saturating_add(1), 0);
                } else {
                    offset = page.next_offset;
                }
            }
            // The folder is gone, or isn't the owner's to open: the others are still asked.
            Err(Asked::Code(ResponseCode::FolderNotFound | ResponseCode::ItemNotFound | ResponseCode::AccessDenied)) => {
                (at, offset) = (at.saturating_add(1), 0);
            }
            // A server without the preview and the flag (it says so, or its schema is one from
            // before them): asked again without them.
            Err(Asked::Code(code @ (ResponseCode::InvalidPropertyRequest | ResponseCode::SchemaValidation)))
                if run.shape == Shape::Full && (code != ResponseCode::SchemaValidation || !client.version().has_preview()) =>
            {
                run.shape = Shape::Lean;
            }
            // A server that won't take the query string, before it has answered one: asked with
            // a restriction from here on.
            Err(Asked::Code(_)) if !by_restriction && !proven && restricted.is_some() => {
                log::info!("ews search: the server didn't take a query string; asking with a restriction");
                by_restriction = true;
            }
            Err(Asked::Code(_) | Asked::Bad | Asked::Lost | Asked::Unreadable) => {
                out.errors.push("Couldn't search this mailbox: the server's answer to the search can't be used.".into());
                break;
            }
            Err(Asked::Stop(stop)) => {
                if let Stop::Busy(seconds) = stop {
                    out.busy_for = Some(seconds.max(1));
                }
                out.offline = matches!(stop, Stop::Unreachable);
                let line = match stop {
                    Stop::Cancelled => None,
                    Stop::Unreachable => Some("couldn't reach the server"),
                    Stop::Auth => Some("the server turned down the sign-in"),
                    Stop::Busy(_) => Some("the server is busy; try again in a moment"),
                    Stop::Tls => Some("couldn't set up a secure connection with the server"),
                    Stop::Line(text) => Some(text),
                };
                if let Some(line) = line {
                    out.errors.push(format!("Couldn't search this mailbox: {line}"));
                }
                break;
            }
        }
    }
    if let Some(folder) = folders.get(at) {
        out.next = Place { folder: (*folder).to_owned(), offset, restricted: by_restriction }.write();
    }
    out.messages.sort_by_key(|m| std::cmp::Reverse(m.received));
    log::info!("ews search: {} found with {asks} requests, more to come: {}", out.messages.len(), !out.next.is_empty());
    out
}
