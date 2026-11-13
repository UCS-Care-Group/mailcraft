// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Many messages removed, deleted or moved in one go (#1037): Empty Folder, Delete and Move of a
//! large selection. What waits for the server (a move or a deletion for an IMAP server, a
//! deletion or a copy for a Microsoft one) is looked up once for the whole lot rather than once
//! for each message, so that the work grows with the number of messages and not with its square:
//! emptying a folder of 200,000 messages took about an hour of the window's time when each
//! removal looked through every change the ones before it had added.
//!
//! What the servers are told is the same as one message at a time would tell them, in the same
//! order for the messages removed; a deletion moves every message it moves before it removes
//! any for good.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::{MailIdentity, Mailbox, ONLY_SERVER_FOLDERS, Pending, Queued, awaits_its_copy, invalid, kept_for_put_back, not_found};
use crate::{FolderKind, Id, Message, ModelError};

// How many times `Mailbox::move_each` was called on this thread: for the tests of what moves
// messages in one go, like `ThreadIndex::work`.
#[cfg(test)]
thread_local! {
    pub(crate) static MOVE_EACH_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What a removal of many messages looks up once rather than for each message.
pub(super) struct Removal {
    /// Where the first move waiting for each message lies in [`Mailbox::pending`] when the
    /// removal starts. A removal only turns a move into a deletion where it lies and adds
    /// deletions at the end, so these places hold until it ends ([`Mailbox::end_removal`]).
    moves: HashMap<Id, usize>,
    /// The deletions already queued for a Microsoft server, by account and the server's id of the
    /// message: worked out at the first one to be added.
    deletes: Option<HashSet<(Id, String)>>,
    /// Messages whose copy waiting to be made on a server (or copied there) is no longer to be.
    unplaced: HashSet<Id>,
    /// The messages removed.
    gone: HashSet<Id>,
}

impl Removal {
    pub(super) fn start(mb: &Mailbox) -> Removal {
        let mut moves = HashMap::new();
        for (at, p) in mb.pending.iter().enumerate() {
            if let Pending::Move { message, .. } = p {
                moves.entry(*message).or_insert(at);
            }
        }
        Removal { moves, deletes: None, unplaced: HashSet::new(), gone: HashSet::new() }
    }
}

/// `ids` without a message named twice, in their order.
fn distinct(ids: &[Id]) -> Vec<Id> {
    let mut seen = HashSet::with_capacity(ids.len());
    ids.iter().copied().filter(|id| seen.insert(*id)).collect()
}

impl Mailbox {
    /// [`Mailbox::remove_message`] for each of `ids`, in one pass: answers how many were removed.
    /// A message named twice is removed once. On an error the messages before it are gone and
    /// the rest are here (a command that fails is put back whole by its session).
    pub fn remove_messages(&mut self, ids: &[Id]) -> Result<usize, ModelError> {
        let ids = distinct(ids);
        let mut r = Removal::start(self);
        let mut done = Ok(ids.len());
        for id in &ids {
            if let Err(e) = self.remove_one(*id, &mut r) {
                done = Err(e);
                break;
            }
        }
        self.end_removal(r);
        done
    }

    /// [`Mailbox::delete_message`] for each of `ids`: each goes to its account's Deleted Items, or
    /// for good when it is in that folder already (or under it, or the account has none). Answers
    /// how many went for good. A message named twice is deleted once (not moved and then removed
    /// for good). The messages that move are moved first, then the others are removed.
    pub fn delete_messages(&mut self, ids: &[Id]) -> Result<usize, ModelError> {
        // Each account's Deleted Items and the folders under it, looked for once.
        let mut bins: HashMap<Id, Option<(Id, HashSet<Id>)>> = HashMap::new();
        let mut moves = Vec::new();
        let mut removals = Vec::new();
        for id in distinct(ids) {
            let (account, folder) = self.message(id).map(|m| (m.account, m.folder)).ok_or(not_found("message", id))?;
            let bin = bins
                .entry(account)
                .or_insert_with(|| self.folder_of_kind(account, FolderKind::Deleted).map(|d| (d, self.subtree(d).into_iter().collect())));
            match bin {
                Some((d, under)) if !under.contains(&folder) => moves.push((id, *d)),
                _ => removals.push(id),
            }
        }
        self.move_each(&moves)?;
        self.remove_messages(&removals)
    }

    /// Move each message of `moves` to its own folder ([`Mailbox::move_message`]), unless it is there
    /// already: answers the ones moved, each with the account it was in. What waits for a message
    /// (a move its server is to make, a copy to be put on a server) is looked for only for the
    /// messages something waited for before the first of them was moved: moving one adds nothing
    /// for another, so a big move takes time in proportion to its size rather than to its square.
    pub fn move_each(&mut self, moves: &[(Id, Id)]) -> Result<Vec<(Id, Id)>, ModelError> {
        #[cfg(test)]
        MOVE_EACH_CALLS.with(|c| c.set(c.get() + 1));
        let pending = self.pending.iter().filter_map(|p| match p {
            Pending::Move { message, .. } => Some(*message),
            Pending::Delete { .. } => None,
        });
        let queued = self.queued.iter().filter_map(|q| match q {
            Queued::PlaceMessage { message, .. } => Some(*message),
            _ => None,
        });
        let waited: HashSet<Id> = pending.chain(queued).collect();
        let mut moved = Vec::new();
        for (id, folder) in moves {
            let Some(account) = self.message(*id).filter(|m| m.folder != *folder).map(|m| m.account) else { continue };
            if self.folder(*folder).is_some() && self.stays_out_of(*id, *folder) {
                return Err(invalid(ONLY_SERVER_FOLDERS));
            }
            self.move_anyway(*id, *folder, !waited.contains(id))?;
            moved.push((*id, account));
        }
        Ok(moved)
    }

    /// Remove `id` for good as part of the removal `r` ([`Mailbox::remove_message`]); what is
    /// done for every message removed at once is left to [`Mailbox::end_removal`].
    pub(super) fn remove_one(&mut self, id: Id, r: &mut Removal) -> Result<Arc<Message>, ModelError> {
        let (was_in, account, uid, message_id, seen, awaiting) = {
            let m = self.message(id).ok_or(not_found("message", id))?;
            (m.folder, m.account, m.uid, m.message_id.clone(), (!m.message_id.is_empty()).then(|| MailIdentity::of(m)), awaits_its_copy(m, m.synced))
        };
        self.queue_graph_delete_in(id, true, r);
        // (Where the removal found it; should it not be there, it is looked for.)
        let waiting = match r.moves.get(&id) {
            Some(&at) if matches!(self.pending.get(at), Some(Pending::Move { message, .. }) if *message == id) => Some(at),
            Some(_) => self.waiting_move(id),
            None => None,
        };
        match waiting.and_then(|at| self.pending.get_mut(at)) {
            Some(op) => {
                if let Pending::Move { account, from, uid, .. } = &mut *op {
                    let delete = Pending::Delete { account: *account, from: std::mem::take(from), uid: *uid, message_id, seen };
                    *op = delete;
                }
            }
            None => {
                let from = self.folder(was_in).filter(|f| f.account == account && !f.remote.is_empty()).map(|f| f.remote.clone());
                // (Or one whose copy hasn't come down yet: deleted there once it is matched.)
                if let Some(from) = from.filter(|_| uid != 0 || (awaiting && seen.is_some())) {
                    self.pending.push(Pending::Delete { account, from, uid, message_id, seen });
                }
            }
        }
        let m = self.messages.remove(&id).ok_or(not_found("message", id))?;
        r.gone.insert(id);
        Ok(m)
    }

    /// What the server of an account whose deletions are queued (a Microsoft account, or one kept
    /// in step over JMAP: [`Mailbox::queues_deletions`]) is to be told when `id` goes for good here, as part
    /// of the removal `r`: its copy is deleted there (`own`; not when its folder's deletion takes
    /// it along), and so is the server's copy of one being moved out of the account, and a copy
    /// of it not yet made there is not made ([`Mailbox::end_removal`] takes those out).
    pub(super) fn queue_graph_delete_in(&mut self, id: Id, own: bool, r: &mut Removal) {
        let Some(m) = self.message(id) else { return };
        let mut deletes = Vec::new();
        if let Some(server) = m.server.as_ref().filter(|s| own && !s.gone && self.queues_deletions(m.account)) {
            deletes.push((m.account, server.id.clone()));
        }
        if let Some(out) = &m.moving_out {
            deletes.push((out.account, out.id.clone()));
        }
        r.unplaced.insert(id);
        if deletes.is_empty() {
            return;
        }
        let kept = kept_for_put_back(m);
        let queued = &self.queued;
        let known = r.deletes.get_or_insert_with(|| {
            queued
                .iter()
                .filter_map(|q| match q {
                    Queued::DeleteMessage { account, id, .. } => Some((*account, id.clone())),
                    _ => None,
                })
                .collect()
        });
        for (account, server_id) in deletes {
            if known.insert((account, server_id.clone())) {
                self.queued.push(Queued::DeleteMessage { account, id: server_id, kept: Some(Box::new(kept.clone())) });
            }
        }
    }

    /// The end of the removal `r`: the copies of the messages it took that waited to be made on a
    /// server aren't made, the moves still waiting for them are forgotten, and the to-dos made
    /// from them no longer name them.
    pub(super) fn end_removal(&mut self, r: Removal) {
        if !r.unplaced.is_empty() && !self.queued.is_empty() {
            self.queued.retain(|q| match q {
                Queued::CopyMessage { copy, .. } => !r.unplaced.contains(copy),
                Queued::PlaceMessage { message, .. } => !r.unplaced.contains(message),
                _ => true,
            });
        }
        if r.gone.is_empty() {
            return;
        }
        if !self.pending.is_empty() {
            self.pending.retain(|p| !matches!(p, Pending::Move { message, .. } if r.gone.contains(message)));
        }
        for t in self.tasks.values_mut().filter(|t| t.message.is_some_and(|m| r.gone.contains(&m))) {
            Arc::make_mut(t).message = None;
        }
    }
}

#[cfg(test)]
#[path = "mailbox_bulk_tests.rs"]
mod mailbox_bulk_tests;
