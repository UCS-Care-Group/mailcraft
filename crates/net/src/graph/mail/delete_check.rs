// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the server holds in a folder before it is asked to delete it for good (AU3-H7).
//!
//! `DELETE /me/mailFolders/{id}` takes with the folder whatever the server holds in it and under
//! it, including what this program hasn't brought down yet: mail that arrived since the last run,
//! a folder made elsewhere. So before it is asked, the folder's subfolders (as the listing walks
//! them: hidden folders and search folders, which are never shown, are left out with what lies
//! under them) and the ids of their messages are read, and set against what the owner had of them
//! when he deleted it ([`Shown`]). Anything more keeps the folder on the server.
//!
//! Every list is capped and every walk bounded; a check that can't be finished is no check, and
//! the folder isn't deleted.

use std::collections::HashSet;

use mailcraft_model::sync::{MAX_SHOWN_FOLDERS, MAX_SHOWN_MESSAGES, Shown};

use super::super::{ASK_TIMEOUT, Doing, Graph, MAX_PAGE};
use super::{FOLDER_FIELDS, FOLDER_PAGE, MAX_DEPTH, children_path, entry_of, folders, id_of};
use crate::NetError;
use crate::graph::pace::Kind;
use crate::http;

/// Messages asked for per page: ids alone, so the most Graph gives.
const MESSAGE_PAGE: &str = "1000";

/// What the check found.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Found {
    /// The server doesn't have the folder.
    Gone,
    /// How many folders and messages the server holds in it and under it that weren't shown.
    Unshown(u32),
    /// The check couldn't be finished (an answer that couldn't be read or used, or more than the
    /// caps): the folder isn't deleted.
    Unknown,
}

/// What the server holds in the folder `id` (a server id it wrote) beyond `shown`. An error is
/// one that stops the run's changes (a sign-in or a shared mailbox refused): the deletion then
/// waits, unsent.
pub(super) fn unshown(graph: &Graph<'_>, id: &str, shown: &Shown) -> Result<Found, NetError> {
    let (shown_folders, shown_messages) = shown.sets();
    // The folder itself: gone already, or there.
    let url = graph.url(&format!("{}/{}?{}", folders(graph), http::encode(id), http::form(&[("$select", "id")])));
    let reply = graph.call(Kind::Read, &url, "GET", &[], None, ASK_TIMEOUT, MAX_PAGE)?;
    super::refusing(graph, &reply)?;
    match reply.status {
        200 => {}
        404 | 410 => return Ok(Found::Gone),
        // No sign-in, or busy past the waits: the run's changes stop, and the deletion waits.
        401 | 429 | 503 | 504 => return Err(graph.refusal(Doing::Folders, &reply, "")),
        _ => {
            log::info!("graph folders: a folder couldn't be looked at before it was deleted (status {})", reply.status);
            return Ok(Found::Unknown);
        }
    }
    let folder_query = http::form(&[("$top", FOLDER_PAGE), ("$select", FOLDER_FIELDS)]);
    let message_query = http::form(&[("$top", MESSAGE_PAGE), ("$select", "id")]);
    let mut unshown = 0u32;
    let mut seen: HashSet<String> = HashSet::from([id.to_string()]);
    let mut messages = 0usize;
    // The folders whose messages and subfolders are read next, all of one depth.
    let mut level: Vec<String> = vec![id.to_string()];
    let mut depth = 0usize;
    while !level.is_empty() {
        if depth >= MAX_DEPTH {
            return Ok(Found::Unknown);
        }
        let mut next: Vec<String> = Vec::new();
        for folder in &level {
            // Its messages.
            let url = graph.url(&format!("{}/{}/messages?{message_query}", folders(graph), http::encode(folder)));
            let most = MAX_SHOWN_MESSAGES.saturating_sub(messages).saturating_add(1);
            let mut unreadable = false;
            let listed = graph.list(Doing::Mail, url, &[], most, &mut |value| {
                messages = messages.saturating_add(1);
                match value.get("id").and_then(serde_json::Value::as_str).and_then(id_of) {
                    Some(message) if shown_messages.contains(&Shown::key(&message)) => {}
                    Some(_) => unshown = unshown.saturating_add(1),
                    None => unreadable = true,
                }
            });
            match checked(listed)? {
                Some(true) if !unreadable && messages <= MAX_SHOWN_MESSAGES => {}
                _ => return Ok(Found::Unknown),
            }
            // Its subfolders.
            let url = graph.url(&children_path(graph, folder, &folder_query));
            let most = MAX_SHOWN_FOLDERS.saturating_sub(seen.len()).saturating_add(1);
            let mut found = Vec::new();
            let listed = graph.list(Doing::Folders, url, &[], most, &mut |value| found.push(entry_of(value)));
            match checked(listed)? {
                Some(true) => {}
                _ => return Ok(Found::Unknown),
            }
            for entry in found {
                let Some(entry) = entry else { return Ok(Found::Unknown) };
                // Never shown, and nor is what lies under it (as the listing has it); one that
                // came twice is the first one.
                if entry.unseen || !seen.insert(entry.id.clone()) {
                    continue;
                }
                if seen.len() > MAX_SHOWN_FOLDERS {
                    return Ok(Found::Unknown);
                }
                if !shown_folders.contains(&Shown::key(&entry.id)) {
                    unshown = unshown.saturating_add(1);
                }
                next.push(entry.id);
            }
        }
        level = next;
        depth += 1;
    }
    Ok(Found::Unshown(unshown))
}

/// A list's answer for the check: whether it was read to its end (`None`: it couldn't be had,
/// and the check is unknown), or the error that stops the run's changes: a sign-in or a shared
/// mailbox refused, a server busy past the waits or out of reach, after which the deletion
/// waits, unsent, to be checked afresh next time (spec R4).
fn checked(listed: Result<bool, NetError>) -> Result<Option<bool>, NetError> {
    match listed {
        Ok(whole) => Ok(Some(whole)),
        Err(e) if e.is_refused_sign_in() || e.is_unreachable() || matches!(e, NetError::SharedRefused(_) | NetError::Busy { .. }) => Err(e),
        // (Not the error's words: they may hold what the server said of the folder.)
        Err(_) => {
            log::info!("graph folders: what a folder holds couldn't be read before it was deleted");
            Ok(None)
        }
    }
}
