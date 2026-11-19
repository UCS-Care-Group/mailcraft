// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What changed in the mailbox (`SyncRequest`): from a token, the messages new or changed since
//! (their ids) and the ids of those deleted, a page at a time, with the token to go on from.
//!
//! The first read of a mailbox asks without a token: the server answers with its whole tree and a
//! token, of which only the token is kept; the folders' messages are then read page by page
//! ([`crate::items::folder_page`]), and the next run goes on from the token.

use serde_json::json;

use crate::error::Error;
use crate::http::{HttpRequest, HttpResponse, Secret};
use crate::json;
use crate::soap;

/// Most changed and deleted ids one page of a delta holds.
pub const CHANGES: usize = 500;
const DELETES: usize = 1000;
/// Longest token taken.
const MAX_TOKEN: usize = 256;

/// What one page of a delta said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Delta {
    /// Where the next page, or the next run, goes on from.
    pub token: String,
    /// Messages new or changed (moved, marked, tagged) since the token asked from.
    pub changed: Vec<String>,
    /// Messages deleted since.
    pub deleted: Vec<String>,
    /// More changes wait: ask again from `token`.
    pub more: bool,
}

/// The request for what changed since `since`, or, with `since` empty, for a token to start from.
pub fn request(base: &str, token: &Secret, since: &str) -> Result<HttpRequest, Error> {
    let body =
        if since.is_empty() { json!({"typed": 1}) } else { json!({"token": since, "typed": 1, "changeLimit": CHANGES, "deleteLimit": DELETES}) };
    soap::request(base, "SyncRequest", soap::MAIL, body, Some(token))
}

/// What a `SyncResponse` says. Of a first read only the token is taken.
pub fn answer(response: &HttpResponse, first: bool) -> Result<Delta, Error> {
    let answer = soap::answer("SyncRequest", response)?;
    let token = json::raw(answer.get("token")).unwrap_or_default();
    let fit = !token.is_empty() && token.len() <= MAX_TOKEN && token.bytes().all(|b| b.is_ascii_alphanumeric() || b"-.:".contains(&b));
    if !fit {
        return Err(Error::Unreadable);
    }
    let mut delta = Delta { token, ..Delta::default() };
    if first {
        return Ok(delta);
    }
    delta.more = json::checked_on(answer.get("more")).ok_or(Error::Unreadable)?;
    if json::bad_list(answer.get("m")) || json::bad_list(answer.get("deleted")) {
        return Err(Error::Unreadable);
    }
    let changed = json::list(answer.get("m"));
    if changed.len() > CHANGES {
        // More than asked for is not a page but an answer that can't be trusted: the token it
        // comes with must never be kept, or what it dropped is gone for good.
        return Err(Error::Unreadable);
    }
    let mut changed_ids = Vec::with_capacity(changed.len());
    for m in changed.iter() {
        let id = m.get("id");
        match json::own_id(id) {
            Some(id) => changed_ids.push(id),
            // A change of another mailbox's item is passed over; an id that is no id at all
            // can't be told from one the page dropped — the token it comes with must never
            // be kept, or the change is gone for good.
            None if json::foreign_id(id) => {}
            None => return Err(Error::Unreadable),
        }
    }
    delta.changed = changed_ids;
    let mut deleted_ids = 0usize;
    for deleted in json::list(answer.get("deleted")) {
        if json::bad_list(deleted.get("m")) {
            return Err(Error::Unreadable);
        }
        for m in json::list(deleted.get("m")) {
            let room = DELETES.saturating_sub(delta.deleted.len());
            let Some(ids) = json::ids(m.get("ids"), room + 1) else { return Err(Error::Unreadable) };
            deleted_ids += ids.len();
            if deleted_ids > DELETES {
                return Err(Error::Unreadable);
            }
            delta.deleted.extend(ids);
        }
    }
    Ok(delta)
}
