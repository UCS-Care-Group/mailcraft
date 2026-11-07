// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What the server holds in a folder before it is asked to delete it for good (AU3-H7).
//!
//! `DeleteFolder` takes with the folder whatever the server holds in it and under it, including
//! what this program hasn't brought down yet: mail that arrived since the last run, a folder made
//! elsewhere. So before it is asked, the folder list is read from its start (`SyncFolderHierarchy`
//! with no state), the folders under the one to delete are found in it (mail folders only, as the
//! list has them), and each one's items are read from their start (`SyncFolderItems` with no
//! state), and set against what the owner had of them when he deleted it ([`Shown`]). Anything
//! more keeps the folder on the server.
//!
//! Every list is capped and every walk bounded; a check that can't be finished is no check, and
//! the folder isn't deleted. Nothing read here is kept: the states the server gives are let go.

use std::collections::{HashMap, HashSet};

use mailcraft_model::sync::{MAX_SHOWN_FOLDERS, MAX_SHOWN_MESSAGES, Shown};

use crate::error::ResponseCode;
use crate::hierarchy::SyncFolderHierarchy;
use crate::http::HttpExchange;
use crate::items::{Shape, SyncFolderItems};
use crate::run::{Asked, PAGE, Run};

/// Most pages of the folder list, and of all the items, the check reads.
const MAX_LIST_PAGES: usize = 50;
const MAX_ITEM_PAGES: usize = MAX_SHOWN_MESSAGES / PAGE as usize + 1;
/// Deepest a folder is followed.
const MAX_DEPTH: usize = 64;

/// What the check found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// The server doesn't have the folder.
    Gone,
    /// How many folders and messages the server holds in it and under it that weren't shown.
    Unshown(u32),
    /// The check couldn't be finished: the folder isn't deleted.
    Unknown,
}

impl<E: HttpExchange> Run<'_, E> {
    /// What the server holds in the folder `id` beyond `shown`. An error is one that ends the run
    /// (called off, the sign-in refused, the server busy or out of reach): the deletion then
    /// waits, unsent.
    pub(crate) async fn unshown(&mut self, id: &str, shown: &Shown) -> Result<Found, Asked> {
        let (shown_folders, shown_messages) = shown.sets();
        // Every mail folder the server has, by the folder it lies in.
        let mut under: HashMap<String, Vec<String>> = HashMap::new();
        let mut known: HashSet<String> = HashSet::new();
        let mut unreadable: HashSet<String> = HashSet::new();
        let mut state = String::new();
        let mut ended = false;
        for _ in 0..MAX_LIST_PAGES {
            let op = SyncFolderHierarchy::new(&state);
            let page = match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                Ok(page) => page,
                Err(stop @ Asked::Stop(_)) => return Err(stop),
                Err(_) => return Ok(Found::Unknown),
            };
            for folder in page.changed {
                if known.len() >= MAX_SHOWN_FOLDERS.saturating_mul(4) {
                    return Ok(Found::Unknown);
                }
                if known.insert(folder.id.clone()) {
                    under.entry(folder.parent).or_default().push(folder.id);
                }
            }
            // Mail folders the list couldn't read (no usable id or name), by where they lie.
            for parent in page.unreadable {
                if unreadable.len() >= MAX_SHOWN_FOLDERS {
                    return Ok(Found::Unknown);
                }
                unreadable.insert(parent);
            }
            state = page.state;
            if page.last {
                ended = true;
                break;
            }
        }
        if !ended {
            return Ok(Found::Unknown);
        }
        if !known.contains(id) {
            return Ok(Found::Gone);
        }
        // The folder and those under it.
        let mut unshown = 0u32;
        let mut walked: Vec<String> = vec![id.to_owned()];
        let mut seen: HashSet<String> = HashSet::from([id.to_owned()]);
        let mut level: Vec<String> = vec![id.to_owned()];
        for _ in 0..MAX_DEPTH {
            let mut next = Vec::new();
            for parent in &level {
                for child in under.get(parent).map(Vec::as_slice).unwrap_or_default() {
                    if !seen.insert(child.clone()) {
                        continue;
                    }
                    if seen.len() > MAX_SHOWN_FOLDERS {
                        return Ok(Found::Unknown);
                    }
                    if !shown_folders.contains(&Shown::key(child)) {
                        unshown = unshown.saturating_add(1);
                    }
                    next.push(child.clone());
                    walked.push(child.clone());
                }
            }
            if next.is_empty() {
                level = next;
                break;
            }
            level = next;
        }
        if !level.is_empty() {
            return Ok(Found::Unknown);
        }
        // A mail folder in it that couldn't be read (or one whose place couldn't be): what it
        // holds can't be told, and the deletion would take it unseen.
        if unreadable.contains("") || walked.iter().any(|f| unreadable.contains(f)) {
            return Ok(Found::Unknown);
        }
        // The items of each.
        let mut pages = 0usize;
        let mut messages = 0usize;
        for folder in &walked {
            let mut state = String::new();
            loop {
                pages = pages.saturating_add(1);
                if pages > MAX_ITEM_PAGES {
                    return Ok(Found::Unknown);
                }
                let op = SyncFolderItems::new(folder, &state, PAGE, Shape::Lean);
                let page = match self.ask(&op.element(), &|reply| op.answer(reply)).await {
                    Ok(page) => page,
                    // (Gone meanwhile: the folder, when it is the one to delete; else nothing of it.)
                    Err(Asked::Code(ResponseCode::FolderNotFound)) if folder == id => return Ok(Found::Gone),
                    Err(Asked::Code(ResponseCode::FolderNotFound)) => break,
                    Err(stop @ Asked::Stop(_)) => return Err(stop),
                    Err(_) => return Ok(Found::Unknown),
                };
                for message in &page.messages {
                    messages = messages.saturating_add(1);
                    if messages > MAX_SHOWN_MESSAGES {
                        return Ok(Found::Unknown);
                    }
                    if !shown_messages.contains(&Shown::key(&message.server.id)) {
                        unshown = unshown.saturating_add(1);
                    }
                }
                state = page.state;
                if page.last {
                    break;
                }
            }
        }
        Ok(Found::Unshown(unshown))
    }
}
