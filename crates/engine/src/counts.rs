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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use mailcraft_model::{GmailCategory, Id, Mailbox, Message};

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

    fn take(&mut self, read: bool) {
        self.total = self.total.saturating_sub(1);
        if !read {
            self.unread = self.unread.saturating_sub(1);
        }
    }
}

/// Two lists of ids, each in order, as one in order.
fn merge(a: impl Iterator<Item = Id>, b: Vec<Id>) -> Vec<Id> {
    let mut out = Vec::new();
    let mut b = b.into_iter().peekable();
    for x in a {
        while let Some(y) = b.next_if(|y| *y < x) {
            out.push(y);
        }
        // (One listed twice, as a damaged mailbox might have it, is listed once.)
        let _ = b.next_if(|y| *y == x);
        out.push(x);
    }
    out.extend(b);
    out
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
    /// What each of Gmail's categories holds in each folder (#911): only messages that are in one.
    gmail: HashMap<(Id, GmailCategory), Held>,
    /// How many messages were counted: the mailbox's, when the counts are of it.
    messages: usize,
}

impl FolderCounts {
    /// Counted over `mb` now.
    pub fn of(mb: &Mailbox) -> FolderCounts {
        let mut folders: HashMap<Id, InFolder> = HashMap::with_capacity(mb.folders.len());
        let mut accounts: HashMap<Id, Held> = HashMap::with_capacity(mb.accounts.len());
        let mut gmail: HashMap<(Id, GmailCategory), Held> = HashMap::new();
        for m in mb.messages.values() {
            let f = folders.entry(m.folder).or_default();
            f.held.add(m.read);
            f.ids.push(m.id);
            accounts.entry(m.account).or_default().add(m.read);
            if let Some(category) = m.gmail_category {
                gmail.entry((m.folder, category)).or_default().add(m.read);
            }
        }
        FolderCounts { folders, accounts, gmail, messages: mb.messages.len() }
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

    /// What the messages of `folder` that Gmail put in `category` come to (#911).
    pub fn gmail(&self, folder: Id, category: GmailCategory) -> Held {
        self.gmail.get(&(folder, category)).copied().unwrap_or_default()
    }

    /// What the messages of `account` come to, whichever folders they are in.
    pub fn account(&self, account: Id) -> Held {
        self.accounts.get(&account).copied().unwrap_or_default()
    }

    /// How many messages were counted.
    pub fn messages(&self) -> usize {
        self.messages
    }

    /// The counts of a mailbox that has since changed in `changes` alone (each message as it
    /// was and as it is now, `None` where there was or is none): what they come to then, as
    /// [`FolderCounts::of`] would count it. Each folder touched has its list of messages put
    /// together once, however many of its messages changed. `false`, with the counts as they
    /// were, for a change to more messages than counting again takes (a large synchronisation).
    pub fn apply(&mut self, changes: &[(Id, Option<Arc<Message>>, Option<Arc<Message>>)]) -> bool {
        if changes.len() > (self.messages / 16).max(256) {
            return false;
        }
        let mut gone: HashMap<Id, Vec<Id>> = HashMap::new();
        let mut come: HashMap<Id, Vec<Id>> = HashMap::new();
        for (id, was, now) in changes {
            if let Some(w) = was {
                if let Some(f) = self.folders.get_mut(&w.folder) {
                    f.held.take(w.read);
                }
                if let Some(a) = self.accounts.get_mut(&w.account) {
                    a.take(w.read);
                }
                if let Some(g) = w.gmail_category.and_then(|c| self.gmail.get_mut(&(w.folder, c))) {
                    g.take(w.read);
                }
                self.messages = self.messages.saturating_sub(1);
            }
            if let Some(n) = now {
                self.folders.entry(n.folder).or_default().held.add(n.read);
                self.accounts.entry(n.account).or_default().add(n.read);
                if let Some(c) = n.gmail_category {
                    self.gmail.entry((n.folder, c)).or_default().add(n.read);
                }
                self.messages = self.messages.saturating_add(1);
            }
            match (was, now) {
                // In the same folder still: its list stands.
                (Some(w), Some(n)) if w.folder == n.folder => {}
                _ => {
                    if let Some(w) = was {
                        gone.entry(w.folder).or_default().push(*id);
                    }
                    if let Some(n) = now {
                        come.entry(n.folder).or_default().push(*id);
                    }
                }
            }
        }
        let touched: HashSet<Id> = gone.keys().chain(come.keys()).copied().collect();
        for folder in touched {
            let Some(f) = self.folders.get_mut(&folder) else { continue };
            let gone: HashSet<Id> = gone.remove(&folder).unwrap_or_default().into_iter().collect();
            let mut come = come.remove(&folder).unwrap_or_default();
            come.sort_unstable();
            let kept = f.ids.iter().copied().filter(|id| !gone.contains(id));
            f.ids = merge(kept, come);
        }
        self.folders.retain(|_, f| f.held.total > 0 || !f.ids.is_empty());
        self.accounts.retain(|_, a| a.total > 0);
        self.gmail.retain(|_, g| g.total > 0);
        true
    }

    /// A message of `folder`, in `account` (and in Gmail's `gmail` category, if any), is now read
    /// (`read`) or unread: its folder's, its account's and its category's unread follow, for counts
    /// kept through read marks rather than counted again.
    pub fn read_changed(&mut self, folder: Id, account: Id, gmail: Option<GmailCategory>, read: bool) {
        let category = gmail.and_then(|c| self.gmail.get_mut(&(folder, c)));
        let folder = self.folders.get_mut(&folder).map(|f| &mut f.held);
        for held in [folder, self.accounts.get_mut(&account), category].into_iter().flatten() {
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
