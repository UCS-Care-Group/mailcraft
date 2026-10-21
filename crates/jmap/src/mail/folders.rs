// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mailboxes a server lists (`Mailbox/get`, RFC 8621 §2) as the folders the engine takes in.
//!
//! - **Kind.** A mailbox's role gives the folder its kind: `inbox`, `sent`, `drafts`, `trash` (Deleted),
//!   `junk` and `archive`; any other role, or none, is a custom folder. The roles `all`, `flagged`,
//!   `important` and `subscribed` name views, not places mail lies in, and are left out. A role a second
//!   mailbox claims is that mailbox's no more: it is custom.
//! - **Tree.** `parentId` gives the parent; one the server does not list (or does not give) is at the
//!   top. Parents come before the mailboxes in them. A mailbox that is its own ancestor is cut loose at
//!   the first place the walk meets it again, and one that would lie deeper than [`MAX_DEPTH`] levels
//!   goes to the top, so the tree is finite whatever the server says, and nothing here recurses on it.
//! - **Names** are cleaned (no control characters, trimmed, [`mailcraft_model::MAX_NAME`] characters at
//!   most). A mailbox with no id that can be used, or no name left, is passed over, and then the listing
//!   is not complete: nothing is removed here for being missing from it.
//! - **A list with no mailbox in it** (none, or only views) is not complete either: an account always has
//!   an Inbox, and a server that has lost its folders for a moment answers just so. The folders of an
//!   account are not removed on its word.
//! - **Counts** (`totalEmails`) are not part of the listing the engine takes in; [`listing_with_counts`] gives
//!   the ones the answer states with a number, for the windows to be held against (the first of an id that is
//!   listed twice, as the listing has it).

use std::collections::{HashMap, HashSet};

use mailcraft_model::sync::{FolderListing, GraphFolder, MAX_LISTED_FOLDERS};
use mailcraft_model::{FolderKind, clean_name};
use serde_json::Value;

use crate::Id;

/// Deepest a mailbox may lie in the tree (the model's own limit for a folder tree).
pub(super) const MAX_DEPTH: usize = 32;

/// The folder a role stands for: `None` for a role that names a view and no place.
fn kind_of(role: &str) -> Option<FolderKind> {
    const VIEWS: [&str; 4] = ["all", "flagged", "important", "subscribed"];
    const KINDS: [(&str, FolderKind); 6] = [
        ("inbox", FolderKind::Inbox),
        ("sent", FolderKind::Sent),
        ("drafts", FolderKind::Drafts),
        ("trash", FolderKind::Deleted),
        ("junk", FolderKind::Junk),
        ("archive", FolderKind::Archive),
    ];
    if VIEWS.iter().any(|view| role.eq_ignore_ascii_case(view)) {
        return None;
    }
    Some(KINDS.iter().find(|(name, _)| role.eq_ignore_ascii_case(name)).map_or(FolderKind::Custom, |(_, kind)| *kind))
}

/// A count the server gave, held to what a folder keeps.
fn count(entry: &Value, name: &str) -> u32 {
    entry.get(name).and_then(Value::as_u64).map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX))
}

/// The listing an answer to `Mailbox/get` (no ids: every mailbox) makes, or `None` when the answer has no
/// list of mailboxes at all.
#[cfg(test)]
pub(super) fn listing(answer: &Value) -> Option<FolderListing> {
    listing_with_counts(answer).map(|(listing, _)| listing)
}

/// [`listing`], and how many emails each mailbox of it says it holds, for those that say so with a number (one
/// that gives none, or something else, is not in the map: that is a count nobody knows, not an empty mailbox).
pub(super) fn listing_with_counts(answer: &Value) -> Option<(FolderListing, HashMap<String, u32>)> {
    let entries = answer.get("list")?.as_array()?;
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut complete = true;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut taken: Vec<GraphFolder> = Vec::new();
    let mut roles: HashSet<FolderKind> = HashSet::new();
    for (n, entry) in entries.iter().enumerate() {
        if n >= MAX_LISTED_FOLDERS {
            complete = false;
            break;
        }
        let kind = match entry.get("role").and_then(Value::as_str) {
            Some(role) => match kind_of(role) {
                Some(kind) => kind,
                None => continue,
            },
            None => FolderKind::Custom,
        };
        let id = entry.get("id").and_then(Value::as_str).filter(|id| Id::new(id).is_ok());
        let name = entry.get("name").and_then(Value::as_str).and_then(clean_name);
        let (Some(id), Some(name)) = (id, name) else {
            complete = false;
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        if let Some(total) = entry.get("totalEmails").and_then(Value::as_u64) {
            counts.insert(id.to_string(), u32::try_from(total).unwrap_or(u32::MAX));
        }
        // One mailbox to a role.
        let kind = if kind != FolderKind::Custom && !roles.insert(kind) { FolderKind::Custom } else { kind };
        let parent = entry.get("parentId").and_then(Value::as_str).filter(|id| Id::new(id).is_ok()).unwrap_or_default();
        taken.push(GraphFolder {
            id: id.to_string(),
            parent: parent.to_string(),
            name,
            kind,
            total: count(entry, "totalEmails"),
            unread: count(entry, "unreadEmails"),
        });
    }
    // (A list with nothing in it is no listing of the account's mailboxes.)
    Some((FolderListing { complete: complete && !taken.is_empty(), folders: arranged(taken) }, counts))
}

/// `folders` with parents before the mailboxes in them, no mailbox its own ancestor and none deeper than
/// [`MAX_DEPTH`]. Mailboxes at one level keep the order the server gave.
fn arranged(mut folders: Vec<GraphFolder>) -> Vec<GraphFolder> {
    let position: HashMap<&str, usize> = folders.iter().enumerate().map(|(n, f)| (f.id.as_str(), n)).collect();
    // The parent of each, by position; none for a mailbox at the top, one whose parent isn't listed, or
    // one that is its own parent.
    let mut up: Vec<Option<usize>> =
        folders.iter().enumerate().map(|(n, f)| position.get(f.parent.as_str()).copied().filter(|parent| *parent != n)).collect();
    // Every mailbox has at most one parent, so a walk up either ends or comes round to a mailbox it has
    // already passed: that one is cut loose.
    let mut walked = vec![0u8; folders.len()];
    for start in 0..folders.len() {
        let mut path = Vec::new();
        let mut at = Some(start);
        while let Some(n) = at {
            match walked.get(n).copied() {
                Some(0) => {
                    if let Some(mark) = walked.get_mut(n) {
                        *mark = 1;
                    }
                    path.push(n);
                    at = up.get(n).copied().flatten();
                }
                Some(1) => {
                    if let Some(parent) = up.get_mut(n) {
                        *parent = None;
                    }
                    break;
                }
                _ => break,
            }
        }
        for n in path {
            if let Some(mark) = walked.get_mut(n) {
                *mark = 2;
            }
        }
    }
    // Depth, from the top down: a mailbox that would be below the limit goes to the top.
    let mut depth = vec![0usize; folders.len()];
    for start in 0..folders.len() {
        let mut chain = Vec::new();
        let mut at = start;
        let above = loop {
            if depth.get(at).copied().unwrap_or(1) != 0 {
                break depth.get(at).copied().unwrap_or(1);
            }
            chain.push(at);
            match up.get(at).copied().flatten() {
                Some(parent) => at = parent,
                None => break 0,
            }
        };
        let mut parent_depth = above;
        while let Some(n) = chain.pop() {
            let mut here = if up.get(n).copied().flatten().is_some() { parent_depth.saturating_add(1) } else { 1 };
            if here > MAX_DEPTH {
                if let Some(parent) = up.get_mut(n) {
                    *parent = None;
                }
                here = 1;
            }
            if let Some(slot) = depth.get_mut(n) {
                *slot = here;
            }
            parent_depth = here;
        }
    }
    let ids: Vec<String> = folders.iter().map(|f| f.id.clone()).collect();
    for (n, folder) in folders.iter_mut().enumerate() {
        folder.parent = up.get(n).copied().flatten().and_then(|parent| ids.get(parent)).cloned().unwrap_or_default();
    }
    let mut order: Vec<usize> = (0..folders.len()).collect();
    order.sort_by_key(|n| (depth.get(*n).copied().unwrap_or(1), *n));
    let mut slots: Vec<Option<GraphFolder>> = folders.into_iter().map(Some).collect();
    order.into_iter().filter_map(|n| slots.get_mut(n).and_then(Option::take)).collect()
}

#[cfg(test)]
mod tests;
