// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What is open in a conversation stacked in the reading pane (#1778), and every change to it,
//! apart from how the column is drawn ([`super::conversation`]).
//!
//! The rules, all here:
//!
//! - **Starting again.** The state is of one selected message. Another selection, or the same
//!   one after the column was off screen, starts afresh ([`Open::for_frame`]).
//! - **Shown whole.** A message is shown whole when the owner didn't close it and it is the
//!   selected one, one the owner opened, or one not read yet ([`Open::would_show`]). Of those, the
//!   selected one always, and at most [`MAX_OPEN`] others: those the owner opened last first, then
//!   the newest ([`Open::shown_whole`]); the others are rows.
//! - **Opening and closing.** A row opens its message, a click on an open one's header closes it,
//!   Expand All opens every message listed (the newest last, so they come first), Collapse All
//!   closes all but the selected one ([`Open::open`], [`Open::close`], [`Open::expand_all`],
//!   [`Open::collapse_all`]).
//! - **Read by selecting.** The messages a selection marked read weren't read until then, so they
//!   are opened, once per selection, even of the same message, and what the owner closed after
//!   stays closed ([`Open::take_in_reads`]).
//! - **Quoted text.** Each message's quoted earlier messages are hidden until the owner asks for
//!   them ([`Open::toggle_quoted`]).
//!
//! Nothing here changes the mailbox or the selection.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Weak};

use mailcraft_model::{Id, Message};

/// Most messages listed in the column (the selected one and the newest).
pub(crate) const MAX_STACKED: usize = 200;
/// Most messages shown whole at once besides the selected one.
pub(crate) const MAX_OPEN: usize = 48;

/// What is open in the column, for the message it was shown for.
#[derive(Clone, Default)]
pub(crate) struct Open {
    focus: Id,
    /// The frame it was last shown in.
    frame: u64,
    /// Opened by the owner or read by selecting, the last last; at most [`MAX_STACKED`].
    opened: Vec<Id>,
    /// [`mailcraft_engine::Session::read_on_open_count`] as last taken in.
    read_count: u64,
    /// Closed by the owner, though not read yet, or selected.
    closed: BTreeSet<Id>,
    /// Messages whose quoted earlier messages the owner asked to see.
    quoted: BTreeSet<Id>,
    /// The column was scrolled to the selected message once.
    pub(crate) scrolled: bool,
    /// The beginnings of the closed messages' text, by the message they were made of.
    pub(crate) previews: HashMap<Id, (Weak<Message>, Arc<str>)>,
}

impl Open {
    /// The state for frame `frame` of the column of `focus` in `thread`, from what was kept
    /// (`kept`): kept when it is of the same selection, shown in the frame before; else afresh,
    /// with the messages the selection read (`reads`: its count and its messages) open. Then the
    /// reads of a newer selection are taken in ([`Open::take_in_reads`]).
    pub(crate) fn for_frame(kept: Option<Open>, focus: Id, frame: u64, reads: (u64, &[Id]), thread: &[Id]) -> Open {
        let mut open = match kept {
            Some(kept) if kept.focus == focus && kept.frame.saturating_add(1) >= frame => kept,
            _ => {
                let opened = reads.1.iter().copied().filter(|id| thread.contains(id)).collect();
                Open { focus, opened, read_count: reads.0, ..Open::default() }
            }
        };
        open.take_in_reads(reads, thread);
        open.frame = frame;
        open
    }

    /// The messages a selection newer than the last taken in read (`reads`) are opened, once.
    pub(crate) fn take_in_reads(&mut self, (count, ids): (u64, &[Id]), thread: &[Id]) {
        if self.read_count == count {
            return;
        }
        self.read_count = count;
        for id in ids.iter().copied().filter(|id| thread.contains(id)) {
            self.open(id);
        }
    }

    /// `id` is opened (last), and no longer closed.
    pub(crate) fn open(&mut self, id: Id) {
        self.closed.remove(&id);
        self.opened.retain(|x| *x != id);
        self.opened.push(id);
        if self.opened.len() > MAX_STACKED {
            self.opened.remove(0);
        }
    }

    /// `id` is closed.
    pub(crate) fn close(&mut self, id: Id) {
        self.opened.retain(|x| *x != id);
        self.closed.insert(id);
    }

    /// Every message of `listed` is opened, the newest last.
    pub(crate) fn expand_all(&mut self, listed: &[Id]) {
        self.closed.clear();
        for id in listed {
            self.open(*id);
        }
    }

    /// Every message of `listed` but the selected one is closed.
    pub(crate) fn collapse_all(&mut self, listed: &[Id]) {
        self.opened.clear();
        self.closed = listed.iter().copied().filter(|id| *id != self.focus).collect();
    }

    /// Whether `m` would be shown whole, room aside.
    pub(crate) fn would_show(&self, m: &Message) -> bool {
        !self.closed.contains(&m.id) && (m.id == self.focus || self.opened.contains(&m.id) || !m.read)
    }

    /// Of `listed`, those shown whole: the selected one unless the owner closed it, and at most
    /// [`MAX_OPEN`] others that would be, those the owner opened last first, then the newest.
    /// `message` looks a message up.
    pub(crate) fn shown_whole<'a>(&self, listed: &[Id], message: impl Fn(Id) -> Option<&'a Message>) -> HashSet<Id> {
        let would = |id: &Id| *id != self.focus && message(*id).is_some_and(|m| self.would_show(m));
        let mut whole: HashSet<Id> = HashSet::new();
        let by_owner = self.opened.iter().rev().filter(|id| listed.contains(id) && would(id));
        let newest = listed.iter().rev().filter(|id| would(id));
        for id in by_owner.chain(newest) {
            if whole.len() >= MAX_OPEN {
                break;
            }
            whole.insert(*id);
        }
        if !self.closed.contains(&self.focus) {
            whole.insert(self.focus);
        }
        whole
    }

    /// Whether the owner asked to see the quoted earlier messages of `id`.
    pub(crate) fn quoted(&self, id: Id) -> bool {
        self.quoted.contains(&id)
    }

    /// The quoted earlier messages of `id` are shown, or hidden again.
    pub(crate) fn toggle_quoted(&mut self, id: Id) {
        if !self.quoted.remove(&id) {
            self.quoted.insert(id);
        }
    }
}

/// The messages of `thread` the column lists: the newest [`MAX_STACKED`], or, when the selected
/// one is older than those, it and the newest but one; oldest first.
pub(crate) fn listed(thread: &[Id], focus: Id) -> Vec<Id> {
    let newest = |n: usize| thread.get(thread.len().saturating_sub(n)..).unwrap_or(thread);
    if thread.len() <= MAX_STACKED || newest(MAX_STACKED).contains(&focus) {
        return newest(MAX_STACKED).to_vec();
    }
    let mut ids = vec![focus];
    ids.extend_from_slice(newest(MAX_STACKED - 1));
    ids
}
