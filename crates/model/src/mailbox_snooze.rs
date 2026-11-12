// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snooze (#949): a message put out of the way until a Return time. It waits in its account's
//! Snoozed folder and then goes back to the folder it came from. What a message remembers of it
//! is a [`Snooze`]; the account remembers the folder by its id ([`Account::snoozed_folder`]), so a
//! rename or another language makes no second one.
//!
//! A JMAP account's server isn't told of moves yet ([`NOT_MOVING_YET`](super::NOT_MOVING_YET)), so
//! a message it has can't be snoozed; mail only this computer has in such an account can.
//!
//! The moves are the mailbox's own ([`Mailbox::move_each`]), so every kind of account tells its
//! server what it tells for any move. Nothing here is I/O.

use std::collections::HashSet;
use std::sync::Arc;

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Mailbox, invalid, not_found};
use crate::{FolderKind, Id, ModelError};

/// The furthest a Return time may lie from now, in seconds: five years. A time beyond it is
/// brought back to it.
pub const MAX_SNOOZE_AHEAD: i64 = 5 * 366 * 86_400;

/// What a message remembers of its snooze. Kept in the mailbox file and nowhere else: the Return
/// time never leaves this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Snooze {
    /// The Return time.
    pub until: Timestamp,
    /// When it was snoozed: mail of its conversation that arrived after this brings it back at
    /// once.
    pub since: Timestamp,
    /// The folder the message came from, which it goes back to (the Inbox when that is gone).
    pub from: Id,
    /// `Some(when)`: the message has come back, at that moment (which may be later than `until`:
    /// MailCraft was closed at the Return time). It stays marked, and pinned at the top of a
    /// date-sorted list, until the owner has opened it. `None`: it is waiting in the Snoozed
    /// folder.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub returned: Option<Timestamp>,
}

impl Snooze {
    /// Whether the message waits for its Return time (it hasn't come back).
    pub fn waiting(&self) -> bool {
        self.returned.is_none()
    }
}

impl Mailbox {
    /// Whether `id` can be the Snoozed folder of `account`: one of its own ordinary folders, not
    /// one that holds the Inbox's or the Outbox's place, and not in Deleted Items (a Snoozed folder
    /// the owner deleted goes there with its id: it is gone, and mail filed into it would be lost
    /// when Deleted Items is emptied).
    fn fits_snoozed(&self, account: Id, id: Id) -> bool {
        self.folder(id).is_some_and(|f| f.account == account && f.kind == FolderKind::Custom && !self.is_send_queue(id) && !self.is_in_deleted(id))
    }

    /// The account's Snoozed folder, when it has one that is there.
    pub fn snoozed_folder(&self, account: Id) -> Option<Id> {
        let id = self.account(account)?.snoozed_folder?;
        self.fits_snoozed(account, id).then_some(id)
    }

    /// The Snoozed folder `account` has, or would take for it: a top-level folder that already
    /// has `name` (another device or MailCraft install made it).
    fn snoozed_candidate(&self, account: Id, name: &str) -> Option<Id> {
        if let Some(id) = self.snoozed_folder(account) {
            return Some(id);
        }
        let wanted = name.trim().to_lowercase();
        self.folders
            .iter()
            .find(|f| f.account == account && f.parent.is_none() && f.kind == FolderKind::Custom && f.name.to_lowercase() == wanted)
            .map(|f| f.id)
    }

    /// The account's Snoozed folder, made when it has none. `name` is what to call a new one (the
    /// interface's language); a folder at the top level that already has that name is taken for it
    /// rather than a second one made. Made through the same path as any folder the owner makes
    /// ([`Mailbox::add_folder`]), so its server hears of it the way it hears of that.
    pub fn ensure_snoozed_folder(&mut self, account: Id, name: &str) -> Result<Id, ModelError> {
        if let Some(id) = self.snoozed_folder(account) {
            return Ok(id);
        }
        let acc = self.account(account).ok_or(not_found("account", account))?;
        if acc.is_shared() {
            return Err(invalid("a shared mailbox has no Snoozed folder"));
        }
        let id = match self.snoozed_candidate(account, name) {
            Some(id) => id,
            None => self.add_folder(account, None, name)?,
        };
        self.account_mut(account)?.snoozed_folder = Some(id);
        Ok(id)
    }

    /// Snooze each of `ids` until `until`, at `now`: moved, in one go, into its account's Snoozed
    /// folder (made with `name` when missing), remembering where it came from. A message already
    /// waiting there gets the new time and keeps where it first came from. Refused, with nothing
    /// changed: a draft, mail in an Outbox, mail a JMAP server has, and mail that already lies in
    /// the Snoozed folder without having been snoozed (it came from another device). Answers how
    /// many were snoozed.
    pub fn snooze_messages(&mut self, ids: &[Id], until: Timestamp, now: Timestamp, name: &str) -> Result<usize, ModelError> {
        let mut seen = HashSet::new();
        let ids: Vec<Id> = ids.iter().copied().filter(|id| seen.insert(*id)).collect();
        for id in &ids {
            let m = self.message(*id).ok_or(not_found("message", *id))?;
            if m.draft || self.is_send_queue(m.folder) {
                return Err(invalid("a message being written or waiting to be sent can't be snoozed"));
            }
            if self.stays_on_server(*id) {
                return Err(invalid(super::NOT_MOVING_YET));
            }
            if !m.snooze.is_some_and(|s| s.waiting()) && self.snoozed_candidate(m.account, name) == Some(m.folder) {
                return Err(invalid("this message is in the Snoozed folder already"));
            }
        }
        let mut moves = Vec::with_capacity(ids.len());
        let mut snoozes = Vec::with_capacity(ids.len());
        for id in &ids {
            let (account, was_in, kept) = {
                let m = self.message(*id).ok_or(not_found("message", *id))?;
                (m.account, m.folder, m.snooze)
            };
            let snoozed = self.ensure_snoozed_folder(account, name)?;
            // Already waiting: only its time changes.
            let from = match kept {
                Some(s) if s.waiting() && was_in == snoozed => s.from,
                _ => was_in,
            };
            moves.push((*id, snoozed));
            snoozes.push((*id, Snooze { until, since: now, from, returned: None }));
        }
        self.move_each(&moves)?;
        for (id, snooze) in &snoozes {
            self.message_mut(*id)?.snooze = Some(*snooze);
        }
        Ok(snoozes.len())
    }

    /// Whether `m` waits in its account's Snoozed folder and could come back.
    fn waits(&self, m: &crate::Message) -> bool {
        m.snooze.is_some_and(|s| s.waiting()) && self.snoozed_folder(m.account) == Some(m.folder) && !self.stays_on_server(m.id)
    }

    /// Messages waiting in a Snoozed folder whose Return time is `now` or earlier, the earliest
    /// first. Only those that could come back.
    pub fn due_snoozes(&self, now: Timestamp) -> Vec<Id> {
        let mut due: Vec<(Timestamp, Id)> =
            self.messages.values().filter(|m| self.waits(m)).filter_map(|m| m.snooze.filter(|s| s.until <= now).map(|s| (s.until, m.id))).collect();
        due.sort_unstable();
        due.into_iter().map(|(_, id)| id).collect()
    }

    /// Whether any message waits for its Return time.
    pub fn has_snoozed(&self) -> bool {
        self.messages.values().any(|m| m.snooze.is_some_and(|s| s.waiting()))
    }

    /// Bring each of `ids` that waits in a Snoozed folder back, at `now`, in one move: to the
    /// folder it came from, or the Inbox when that is gone (or is in another account, or has been
    /// deleted), unread, marked as having come back. The received date is as it was. A message
    /// that can't be moved stays where it is, still waiting. Answers the ones that came back.
    pub fn return_snoozed(&mut self, ids: &[Id], now: Timestamp) -> Vec<Id> {
        let mut seen = HashSet::new();
        let mut plan: Vec<(Id, Id, Snooze)> = Vec::new();
        for id in ids {
            if !seen.insert(*id) {
                continue;
            }
            let Some(m) = self.message(*id).filter(|m| self.waits(m)) else { continue };
            let Some(snooze) = m.snooze else { continue };
            let account = m.account;
            let snoozed = self.snoozed_folder(account);
            let to = self
                .folder(snooze.from)
                .filter(|f| f.account == account && Some(f.id) != snoozed && self.fits_back(f.id))
                .map(|f| f.id)
                .or_else(|| self.folder_of_kind(account, FolderKind::Inbox));
            if let Some(to) = to {
                plan.push((*id, to, snooze));
            }
        }
        let moves: Vec<(Id, Id)> = plan.iter().map(|(id, to, _)| (*id, *to)).collect();
        if self.move_each(&moves).is_err() {
            // One that can't move mustn't hold the others back (a part of the moves may have gone).
            for (id, to) in &moves {
                if self.message(*id).is_some_and(|m| m.folder != *to) {
                    let _ = self.move_each(&[(*id, *to)]);
                }
            }
        }
        let mut back = Vec::new();
        for (id, to, snooze) in plan {
            if self.message(id).is_some_and(|m| m.folder == to)
                && let Ok(m) = self.message_mut(id)
            {
                m.read = false;
                m.snooze = Some(Snooze { returned: Some(now), ..snooze });
                back.push(id);
            }
        }
        back
    }

    /// Whether mail may be returned into the folder `id`: not Deleted Items or under it.
    fn fits_back(&self, id: Id) -> bool {
        !self.is_in_deleted(id)
    }

    /// Whether the folder `id` is Deleted Items or lies under it (a folder the owner deleted goes
    /// there as it is).
    fn is_in_deleted(&self, id: Id) -> bool {
        let mut at = Some(id);
        for _ in 0..=crate::MAX_FOLDER_DEPTH {
            let Some(f) = at.and_then(|id| self.folder(id)) else { return false };
            if f.kind == FolderKind::Deleted {
                return true;
            }
            at = f.parent;
        }
        false
    }

    /// Put the snoozes right with where the mail is: the snooze of every message that waits but
    /// isn't in its account's Snoozed folder any more is called off (the owner moved it out, here
    /// or on another device, or the folder is gone); a message marked as come back that still lies
    /// in the Snoozed folder (the server refused the move back) waits again, and so comes back at
    /// the next step. Answers how many snoozes were changed.
    pub fn settle_snoozes(&mut self) -> usize {
        let fix: Vec<(Id, bool)> = self
            .messages
            .values()
            .filter_map(|m| {
                let s = m.snooze?;
                let in_snoozed = self.snoozed_folder(m.account) == Some(m.folder);
                match (s.waiting(), in_snoozed) {
                    (true, false) => Some((m.id, true)),
                    (false, true) => Some((m.id, false)),
                    _ => None,
                }
            })
            .collect();
        for (id, call_off) in &fix {
            if let Some(m) = self.messages.get_mut(id) {
                let m = Arc::make_mut(m);
                m.snooze = if *call_off { None } else { m.snooze.map(|s| Snooze { returned: None, ..s }) };
            }
        }
        fix.len()
    }

    /// Part of [`Mailbox::repair`]: an account's Snoozed folder that can't be one is forgotten
    /// (it isn't there, is the Inbox or the Outbox, or is in Deleted Items), and the snoozes are
    /// put right ([`Mailbox::settle_snoozes`]). Answers how many snoozes were changed.
    pub(super) fn repair_snoozes(&mut self) -> usize {
        let known: Vec<(Id, Option<Id>)> = self.accounts.iter().map(|a| (a.id, a.snoozed_folder)).collect();
        for (account, folder) in known {
            if folder.is_some()
                && self.snoozed_folder(account).is_none()
                && let Ok(a) = self.account_mut(account)
            {
                a.snoozed_folder = None;
            }
        }
        self.settle_snoozes()
    }
}
