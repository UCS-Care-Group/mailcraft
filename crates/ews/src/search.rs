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
//! - **A query string is checked once.** Grommunio takes a `QueryString` and doesn't look at it:
//!   every message of the folder is its answer (#1725). So the first time a query string finds
//!   anything, the same folder is asked for a word no message holds (random, made for the
//!   occasion). A server that finds something for that too is not searching: what it sent is
//!   dropped, and it is asked with a restriction from there on. The check is one request a
//!   search, and none for a search that finds nothing.
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

/// How the search is written to this server.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Mode {
    /// As a query string, which the server hasn't been seen to look at yet.
    #[default]
    Query,
    /// As a query string, which the server was seen to look at.
    Checked,
    /// As a restriction.
    Restriction,
}

/// Where a search goes on from: the folder, how many of its matches were read, and how the
/// search is written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Place {
    folder: String,
    offset: u32,
    mode: Mode,
}

impl Place {
    /// As [`GraphSearchResult::next`] carries it: the offset, `q`, `v` or `r`, and the folder's id.
    fn write(&self) -> String {
        let mode = match self.mode {
            Mode::Query => 'q',
            Mode::Checked => 'v',
            Mode::Restriction => 'r',
        };
        format!("{} {mode} {}", self.offset, self.folder)
    }

    /// What `next` says, when it is something [`Place::write`] wrote.
    fn read(next: &str) -> Option<Place> {
        let mut parts = next.splitn(3, ' ');
        let offset = parts.next()?.parse().ok()?;
        let mode = match parts.next()? {
            "q" => Mode::Query,
            "v" => Mode::Checked,
            "r" => Mode::Restriction,
            _ => return None,
        };
        let folder = parts.next().filter(|id| valid_id(id))?.to_owned();
        Some(Place { folder, offset, mode })
    }
}

/// A word no message holds: letters and digits, made anew each time.
fn nonsense() -> String {
    use std::hash::{BuildHasher, Hasher};
    let random = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    format!("zq{:016x}{:016x}", random(), random())
}

/// What a search that had to stop says of it.
fn stopped(out: &mut GraphSearchResult, stop: Stop) {
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
    let (mut at, mut offset, mut mode) = match Place::read(&request.next) {
        // A folder that isn't asked about any more: there is nothing to go on with.
        Some(place) => match folders.iter().position(|id| *id == place.folder) {
            Some(at) => (at, place.offset, place.mode),
            None => return out,
        },
        None => (0, 0, if asked.is_some() { Mode::Query } else { Mode::Restriction }),
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
        let search = match (mode, &asked, &restricted) {
            (Mode::Query | Mode::Checked, Some(query), _) => Search::Query(query.clone()),
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
                proven |= mode != Mode::Restriction;
                // The first time a query string finds anything: does the server look at it?
                if mode == Mode::Query && !page.messages.is_empty() {
                    asks = asks.saturating_add(1);
                    let check = FindItem::new(folder, Search::Query(nonsense()), 0, 1, run.shape);
                    match run.ask(&check.element(), &|reply| check.answer(reply)).await {
                        Ok(none) if none.messages.is_empty() => mode = Mode::Checked,
                        // It found something for a word nobody wrote: it isn't searching.
                        Ok(_) if restricted.is_some() => {
                            log::info!("ews search: the server doesn't look at a query string; asking with a restriction");
                            (mode, offset) = (Mode::Restriction, 0);
                            continue;
                        }
                        Ok(_) => {
                            out.errors.push("This server can't be asked this search.".into());
                            break;
                        }
                        Err(Asked::Stop(stop)) => {
                            stopped(&mut out, stop);
                            break;
                        }
                        // The check itself went wrong: this page is not shown, and is asked for again.
                        Err(_) => {
                            out.errors.push("Couldn't search this mailbox: the server's answer to the search can't be used.".into());
                            break;
                        }
                    }
                }
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
            Err(Asked::Code(_)) if mode != Mode::Restriction && !proven && restricted.is_some() => {
                log::info!("ews search: the server didn't take a query string; asking with a restriction");
                mode = Mode::Restriction;
            }
            Err(Asked::Code(_) | Asked::Bad | Asked::Lost | Asked::Unreadable) => {
                out.errors.push("Couldn't search this mailbox: the server's answer to the search can't be used.".into());
                break;
            }
            Err(Asked::Stop(stop)) => {
                stopped(&mut out, stop);
                break;
            }
        }
    }
    if let Some(folder) = folders.get(at) {
        out.next = Place { folder: (*folder).to_owned(), offset, mode }.write();
    }
    out.messages.sort_by_key(|m| std::cmp::Reverse(m.received));
    log::info!("ews search: {} found with {asks} requests, more to come: {}", out.messages.len(), !out.next.is_empty());
    out
}
