// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's data model: everything a mailbox holds, and the primitives that edit it.
//!
//! A [`Mailbox`] owns accounts, folders, messages, contacts, calendars, events, tasks, categories,
//! rules and signatures. Items are `Arc`-shared, so a clone of the mailbox (an undo snapshot) is
//! cheap. Nothing here does I/O: file formats live in `mailcraft-mime`, `-ical` and `-vcard`,
//! servers in `mailcraft-proto`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod address;
mod calendar;
mod contact;
mod jmap_server;
mod load_limit;
mod mailbox;
mod message;
mod offline;
pub mod provider;
pub mod remote;
pub mod rich;
mod rule;
pub mod search;
pub mod signature;
pub mod sync;
mod task;
pub mod update;
pub mod words;
pub mod zoned;

use serde::{Deserialize, Serialize};

pub use address::{Address, MAX_ADDRESS, format_list, format_list_capped, mailbox_address};
pub use calendar::{
    Attendee, Calendar, CalendarOwner, Event, Freq, MAX_JOIN_LINK, MAX_OPENED_CALENDARS, Occurrence, OnlineMeeting, OnlineProvider, OpenedCalendar,
    Proposal, Recurrence, Response, ResponseOptions, Rights, ShowAs,
};
pub use contact::{Contact, Labeled, PostalAddress};
pub use jmap_server::JmapServer;
pub use load_limit::{MIN_ITEM_BYTES, from_json};
pub use mailbox::{
    Account, AccountKind, AsWritten, BETWEEN_MAILBOXES, Category, Directory, DownloadInFull, Folder, FolderCounts, FolderKind, GraphSwitch, IMPORTED,
    INTO_GRAPH, MAX_RULES, MAX_SHARED, Mailbox, NOT_SENDING_YET, OAuth, OAuthProvider, Pending, Queued, STAYS_LOCAL, Security, Server,
    ServerFavorites, ServerFolder,
};
pub use mailcraft_time as time;
pub use message::{
    Answers, Attachment, Body, ContentRef, FlagStatus, Importance, KeptBody, Message, NOT_DOWNLOADED, normalize_subject, size_label, size_label_in,
    text_hash,
};
pub use offline::{KeepOffline, OfflineWindow};
pub use remote::Origin;
pub use rich::{RichBody, RichText};
pub use rule::{Action, Condition, Rule};
pub use signature::Signature;
pub use task::Task;

/// An item's identity inside one mailbox. 0 means "not assigned yet".
pub type Id = u64;

/// Longest name a folder, category, rule or signature may have.
pub const MAX_NAME: usize = 120;

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ModelError {
    #[error("no {0} with id {1}")]
    NotFound(&'static str, Id),
    #[error("{0}")]
    Invalid(String),
}

/// An sRGB colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// From `#RRGGBB` or `RRGGBB`.
    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 || !s.is_ascii() {
            return None;
        }
        let p = |i: usize| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
        Some(Rgb(p(0)?, p(2)?, p(4)?))
    }

    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }
}

/// A name the user typed: trimmed, without control characters, at most [`MAX_NAME`] characters;
/// `None` when nothing is left.
pub fn clean_name(s: &str) -> Option<String> {
    let s: String = s.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Case-insensitive "contains", for search and rules.
pub fn contains_ci(haystack: &str, needle: &str) -> bool {
    needle.is_empty() || haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Bytes as base64 text in JSON.
pub(crate) mod b64 {
    use std::sync::Arc;

    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Arc<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&mailcraft_charset::base64::STANDARD.encode(v.as_slice()))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<u8>>, D::Error> {
        let text = String::deserialize(d)?;
        mailcraft_charset::base64::STANDARD.decode(text.trim()).map(Arc::new).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod calendar_rights_tests;
#[cfg(test)]
mod mailbox_tests;
#[cfg(test)]
mod recurrence_tests;
#[cfg(test)]
mod search_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_names() {
        assert_eq!(Rgb::parse("#3B5BDB"), Some(Rgb(0x3B, 0x5B, 0xDB)));
        assert_eq!(Rgb::parse("3b5bdb").map(Rgb::hex).as_deref(), Some("#3B5BDB"));
        assert_eq!(Rgb::parse("#12345"), None);
        assert_eq!(Rgb::parse("#12345é"), None);
        assert_eq!(clean_name("  Pro\u{7}jects \n").as_deref(), Some("Projects"));
        assert_eq!(clean_name(" \t "), None);
        assert_eq!(clean_name(&"x".repeat(500)).map(|s| s.len()), Some(MAX_NAME));
        assert!(contains_ci("Quarterly REPORT", "report"));
        assert!(!contains_ci("Quarterly", "report"));
    }
}
