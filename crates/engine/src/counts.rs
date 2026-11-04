// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What each folder and each account holds: how many messages, and how many of them unread,
//! counted in one pass over the mail and kept until the mailbox changes.
//!
//! The folder pane shows a count beside every folder, the status bar the unread of the folder in
//! view, and `folder.list` each account's unread: before this, each worked them out again at
//! every frame by walking every message (two walks a frame, 326,000 messages each, took most of
//! a 60 ms frame on a mailbox of three Microsoft 365 accounts). [`Session::folder_counts`] counts
//! once per change ([`Session::rev`]) and hands the same counts to everyone until the next.

use std::collections::HashMap;
use std::sync::Arc;

use mailcraft_model::{Id, Mailbox};

use crate::Session;

/// Messages held, and unread among them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    pub total: usize,
    pub unread: usize,
}

impl Held {
    fn add(&mut self, read: bool) {
        self.total = self.total.saturating_add(1);
        if !read {
            self.unread = self.unread.saturating_add(1);
        }
    }
}

/// A folder's messages: what they come to, and which they are.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct InFolder {
    held: Held,
    /// The ids of its messages, in order of id.
    ids: Vec<Id>,
}

/// What every folder and every account holds, as of one pass over the mail, and which messages
/// are in each folder: the message list is built from a folder's own messages rather than from
/// a walk over every message of every account.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FolderCounts {
    folders: HashMap<Id, InFolder>,
    accounts: HashMap<Id, Held>,
    /// How many messages were counted: the mailbox's, when the counts are of it.
    messages: usize,
}

impl FolderCounts {
    /// Counted over `mb` now.
    pub fn of(mb: &Mailbox) -> FolderCounts {
        let mut folders: HashMap<Id, InFolder> = HashMap::with_capacity(mb.folders.len());
        let mut accounts: HashMap<Id, Held> = HashMap::with_capacity(mb.accounts.len());
        for m in mb.messages.values() {
            let f = folders.entry(m.folder).or_default();
            f.held.add(m.read);
            f.ids.push(m.id);
            accounts.entry(m.account).or_default().add(m.read);
        }
        FolderCounts { folders, accounts, messages: mb.messages.len() }
    }

    /// What `folder` holds (nothing, for a folder with no mail or none of that id).
    pub fn folder(&self, folder: Id) -> Held {
        self.folders.get(&folder).map(|f| f.held).unwrap_or_default()
    }

    /// The messages in `folder`, in order of id (none, for a folder with no mail or none of
    /// that id).
    pub fn ids(&self, folder: Id) -> &[Id] {
        self.folders.get(&folder).map_or(&[], |f| f.ids.as_slice())
    }

    /// Messages in `folder`, as [`Mailbox::count`] has them.
    pub fn total(&self, folder: Id) -> usize {
        self.folder(folder).total
    }

    /// Unread messages in `folder`, as [`Mailbox::unread_count`] has them.
    pub fn unread(&self, folder: Id) -> usize {
        self.folder(folder).unread
    }

    /// What the messages of `account` come to, whichever folders they are in.
    pub fn account(&self, account: Id) -> Held {
        self.accounts.get(&account).copied().unwrap_or_default()
    }

    /// How many messages were counted.
    pub fn messages(&self) -> usize {
        self.messages
    }

    /// A message of `folder`, in `account`, is now read (`read`) or unread: its folder's and its
    /// account's unread follow, for counts kept through read marks rather than counted again.
    pub fn read_changed(&mut self, folder: Id, account: Id, read: bool) {
        let folder = self.folders.get_mut(&folder).map(|f| &mut f.held);
        for held in [folder, self.accounts.get_mut(&account)].into_iter().flatten() {
            held.unread = if read { held.unread.saturating_sub(1) } else { held.unread.saturating_add(1) };
        }
    }
}

impl Session {
    /// What each folder and account holds, counted once per change to the mailbox and shared
    /// until the next: the same `Arc` comes back while nothing changed.
    pub fn folder_counts(&mut self) -> Arc<FolderCounts> {
        // (The message count beside the revision: a mailbox changed without a command noting it
        // still counts right when a message came or went.)
        let key = (self.rev(), self.mailbox.messages.len());
        if let Some((at, counts)) = &self.counts_cache
            && *at == key
        {
            return counts.clone();
        }
        let counts = Arc::new(FolderCounts::of(&self.mailbox));
        self.counts_cache = Some((key, counts.clone()));
        counts
    }

    /// The counts, when they are of the mailbox as it is now ([`Session::folder_counts`] was
    /// asked since it last changed); for a caller that can't count ([`Session::rows`] asks
    /// first, so that the list is built from them).
    pub fn fresh_counts(&self) -> Option<&FolderCounts> {
        let key = (self.rev(), self.mailbox.messages.len());
        self.counts_cache.as_ref().filter(|(at, _)| *at == key).map(|(_, counts)| counts.as_ref())
    }
}
