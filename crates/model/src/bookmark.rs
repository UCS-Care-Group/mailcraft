// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local bookmarks in messages and conversations (#951).
//!
//! A bookmark marks a place in a message (a paragraph or an anchor quote) so that the owner
//! can jump back to it. Bookmarks are stored locally on this computer in the mailbox file,
//! never change the message itself, and are never sent to a mail server.

use serde::{Deserialize, Serialize};

use crate::Id;
use crate::time::Timestamp;

/// The maximum number of bookmarks kept in a mailbox.
pub const MAX_BOOKMARKS: usize = 2_000;
/// The maximum length of a bookmark's custom or generated name (in characters).
pub const MAX_BOOKMARK_NAME: usize = 120;
/// The maximum length of a quote snippet saved as anchor text (in characters).
pub const MAX_BOOKMARK_QUOTE: usize = 200;
/// The maximum character/byte offset allowed for a bookmark in a message.
pub const MAX_BOOKMARK_OFFSET: usize = 10_000_000;

/// A local mark at a place in a message.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bookmark {
    /// Unique identifier of this bookmark.
    pub id: Id,
    /// The message this bookmark marks.
    pub message: Id,
    /// A short label or quote name for the bookmark.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// An excerpt or anchor quote from the message at this mark.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub quote: String,
    /// Character or paragraph offset in the message text.
    #[serde(skip_serializing_if = "is_zero")]
    pub offset: usize,
    /// When this mark was created.
    #[serde(skip_serializing_if = "is_zero_timestamp")]
    pub created: Timestamp,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_zero_timestamp(t: &Timestamp) -> bool {
    *t == 0
}

impl Bookmark {
    /// Create a new bookmark, sanitizing name and quote to length limits and removing control characters.
    pub fn new(id: Id, message: Id, name: &str, quote: &str, offset: usize, created: Timestamp) -> Self {
        let clean = |s: &str, cap: usize| -> String {
            s.trim().chars().filter(|c| !c.is_control() || *c == ' ' || *c == '\n' || *c == '\t').take(cap).collect::<String>().trim().to_string()
        };
        let mut name_clean = clean(name, MAX_BOOKMARK_NAME);
        let quote_clean = clean(quote, MAX_BOOKMARK_QUOTE);
        if name_clean.is_empty() && !quote_clean.is_empty() {
            // First line or snippet of the quote as default name
            let first_line = quote_clean.lines().next().unwrap_or("").trim();
            name_clean = first_line.chars().take(40).collect();
        }
        let offset = offset.min(MAX_BOOKMARK_OFFSET);
        Bookmark { id, message, name: name_clean, quote: quote_clean, offset, created }
    }
}
