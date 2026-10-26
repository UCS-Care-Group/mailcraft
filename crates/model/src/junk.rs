// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The Safe Senders and Blocked Senders lists of Junk E-mail Options (#504).
//!
//! Each entry is an address (`ada@example.org`) or a domain (`@example.org`), kept in lower case.
//! An address matches that sender alone; a domain matches mail from that domain alone, not from
//! its subdomains (`@example.org` doesn't match `news.example.org`), as classic Outlook's lists
//! do. A sender on both lists is safe. The lists are the owner's, kept in the mailbox file on
//! this computer: they only ever move mail as it arrives ([`crate::Mailbox::sort_junk`]), and
//! they don't change which pictures are downloaded.

use serde::{Deserialize, Serialize};

use crate::MAX_ADDRESS;

/// The most entries each list keeps; a file with more is cut to this when it is repaired.
pub const MAX_JUNK_ENTRIES: usize = 1_000;

/// Which of the two lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JunkList {
    /// Safe Senders: never treated as junk.
    Safe,
    /// Blocked Senders: always junk.
    Blocked,
}

impl JunkList {
    /// `"safe"` or `"blocked"`, as commands name it.
    pub fn as_str(self) -> &'static str {
        match self {
            JunkList::Safe => "safe",
            JunkList::Blocked => "blocked",
        }
    }

    pub fn parse(s: &str) -> Option<JunkList> {
        match s.trim().to_ascii_lowercase().as_str() {
            "safe" => Some(JunkList::Safe),
            "blocked" => Some(JunkList::Blocked),
            _ => None,
        }
    }

    fn other(self) -> JunkList {
        match self {
            JunkList::Safe => JunkList::Blocked,
            JunkList::Blocked => JunkList::Safe,
        }
    }
}

/// What came of adding an entry ([`JunkLists::add`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Added {
    /// Added (and taken off the other list, if it was there), as it is kept.
    Added(String),
    /// On this list already.
    There(String),
    /// Not an address or a domain.
    Invalid,
    /// The list holds [`MAX_JUNK_ENTRIES`] already.
    Full,
}

/// The two lists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct JunkLists {
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::load_limit::capped_vec")]
    pub safe: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::load_limit::capped_vec")]
    pub blocked: Vec<String>,
}

impl JunkLists {
    pub fn is_empty(&self) -> bool {
        self.safe.is_empty() && self.blocked.is_empty()
    }

    pub fn list(&self, which: JunkList) -> &[String] {
        match which {
            JunkList::Safe => &self.safe,
            JunkList::Blocked => &self.blocked,
        }
    }

    fn list_mut(&mut self, which: JunkList) -> &mut Vec<String> {
        match which {
            JunkList::Safe => &mut self.safe,
            JunkList::Blocked => &mut self.blocked,
        }
    }

    /// Add `text` (an address or a domain, [`junk_entry`]) to `which`, taking it off the other
    /// list: an entry is on one list at most.
    pub fn add(&mut self, which: JunkList, text: &str) -> Added {
        let Some(entry) = junk_entry(text) else { return Added::Invalid };
        if self.list(which).contains(&entry) {
            return Added::There(entry);
        }
        if self.list(which).len() >= MAX_JUNK_ENTRIES {
            return Added::Full;
        }
        self.list_mut(which.other()).retain(|e| *e != entry);
        self.list_mut(which).push(entry.clone());
        Added::Added(entry)
    }

    /// Take `text` off `which`. Whether it was there.
    pub fn remove(&mut self, which: JunkList, text: &str) -> bool {
        let entry = junk_entry(text).unwrap_or_else(|| text.trim().to_lowercase());
        let list = self.list_mut(which);
        let before = list.len();
        list.retain(|e| *e != entry);
        list.len() != before
    }

    /// Which list the sender `address` is on, by its address or its domain; Safe when both.
    pub fn verdict(&self, address: &str) -> Option<JunkList> {
        if self.is_empty() {
            return None;
        }
        let address = address.trim();
        if address.is_empty() || address.len() > MAX_ADDRESS {
            return None;
        }
        let address = address.to_lowercase();
        let (_, domain) = address.rsplit_once('@')?;
        let on = |list: &[String]| list.iter().any(|e| *e == address || e.strip_prefix('@').is_some_and(|d| d == domain));
        if on(&self.safe) {
            Some(JunkList::Safe)
        } else if on(&self.blocked) {
            Some(JunkList::Blocked)
        } else {
            None
        }
    }

    /// Entries as they are kept, each once, on one list at most (Safe when on both), and no more
    /// than [`MAX_JUNK_ENTRIES`] a list. Whether anything changed.
    pub fn repair(&mut self) -> bool {
        let before = self.clone();
        let clean = |list: &[String]| {
            let mut out: Vec<String> = Vec::new();
            for e in list.iter().filter_map(|e| junk_entry(e)) {
                if out.len() >= MAX_JUNK_ENTRIES {
                    break;
                }
                if !out.contains(&e) {
                    out.push(e);
                }
            }
            out
        };
        self.safe = clean(&self.safe);
        let safe = self.safe.clone();
        self.blocked = clean(&self.blocked);
        self.blocked.retain(|e| !safe.contains(e));
        *self != before
    }
}

/// An entry of a list as it is kept, from what was typed: an address (`ada@example.org`) or a
/// domain (`@example.org`; `example.org` and `*@example.org` are read as that too), in lower
/// case, trimmed, without angle brackets. `None` when it is neither.
pub fn junk_entry(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_ADDRESS + 2 {
        return None;
    }
    let text = text.strip_prefix('<').and_then(|t| t.strip_suffix('>')).unwrap_or(text).trim();
    let text = text.strip_prefix('*').filter(|t| t.starts_with('@')).unwrap_or(text);
    let text = text.to_lowercase();
    if text.len() > MAX_ADDRESS || text.chars().any(|c| c.is_control() || c.is_whitespace() || "<>,;:\"()[]\\/?#".contains(c)) {
        return None;
    }
    let (local, domain) = match text.rsplit_once('@') {
        Some((local, domain)) => (local, domain),
        None => ("", text.as_str()),
    };
    let domain = domain.strip_suffix('.').unwrap_or(domain);
    let labels_ok = domain.contains('.') && domain.split('.').all(|l| !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-'));
    if !labels_ok || local.contains('@') {
        return None;
    }
    Some(format!("{local}@{domain}"))
}
