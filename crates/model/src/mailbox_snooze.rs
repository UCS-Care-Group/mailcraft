// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snooze (#949): a message put out of the way until a Return time. It waits in its account's
//! Snoozed folder and then goes back to the folder it came from. What a message remembers of it
//! is a [`Snooze`]; the account remembers the folder by its id ([`Account::snoozed_folder`]), so a
//! rename or another language makes no second one.
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
    /// The account's Snoozed folder, when it has one that is there.
    pub fn snoozed_folder(&self, account: Id) -> Option<Id> {
        let id = self.account(account)?.snoozed_folder?;
        self.folder(id).filter(|f| f.account == account).map(|f| f.id)
    }

    /// The account's Snoozed folder, made when it has none. `name` is what to call a new one (the
    /// interface's language); a folder at the top level that already has that name is taken for it
    /// rather than a second one made (another device or MailCraft install made it). Made through
    /// the same path as any folder the owner makes ([`Mailbox::add_folder`]), so its server hears
    /// of it the way it hears of that.
    pub fn ensure_snoozed_folder(&mut self, account: Id, name: &str) -> Result<Id, ModelError> {
        if let Some(id) = self.snoozed_folder(account) {
            return Ok(id);
        }
        let acc = self.account(account).ok_or(not_found("account", account))?;
        if acc.is_shared() {
            return Err(invalid("a shared mailbox has no Snoozed folder"));
        }
        let wanted = name.trim().to_lowercase();
        let existing = self
            .folders
            .iter()
            .find(|f| f.account == account && f.parent.is_none() && f.kind == FolderKind::Custom && f.name.to_lowercase() == wanted)
            .map(|f| f.id);
        let id = match existing {
            Some(id) => id,
            None => self.add_folder(account, None, name)?,
        };
        self.account_mut(account)?.snoozed_folder = Some(id);
        Ok(id)
    }

    /// Snooze each of `ids` until `until`: moved into its account's Snoozed folder (made with
    /// `name` when missing), remembering where it came from. A message already waiting there
    /// gets the new time and keeps where it first came from; one that is a draft, or lies in an
    /// Outbox, is refused. Answers how many were snoozed.
    pub fn snooze_messages(&mut self, ids: &[Id], until: Timestamp, name: &str) -> Result<usize, ModelError> {
        // Everything is checked before anything is moved, so a refusal leaves the mailbox as it was.
        for id in ids {
            let m = self.message(*id).ok_or(not_found("message", *id))?;
            if m.draft || self.is_send_queue(m.folder) {
                return Err(invalid("a message being written or waiting to be sent can't be snoozed"));
            }
            if self.stays_on_server(*id) {
                return Err(invalid(super::NOT_MOVING_YET));
            }
        }
        let mut count = 0usize;
        let mut seen = HashSet::new();
        for id in ids {
            if !seen.insert(*id) {
                continue;
            }
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
            self.move_each(&[(*id, snoozed)])?;
            self.message_mut(*id)?.snooze = Some(Snooze { until, from, returned: None });
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    /// Messages waiting in a Snoozed folder whose Return time is `now` or earlier, the earliest
    /// first.
    pub fn due_snoozes(&self, now: Timestamp) -> Vec<Id> {
        let mut due: Vec<(Timestamp, Id)> =
            self.messages.values().filter_map(|m| m.snooze.filter(|s| s.waiting() && s.until <= now).map(|s| (s.until, m.id))).collect();
        due.sort_unstable();
        due.into_iter().map(|(_, id)| id).collect()
    }

    /// Whether any message waits in a Snoozed folder.
    pub fn has_snoozed(&self) -> bool {
        self.messages.values().any(|m| m.snooze.is_some_and(|s| s.waiting()))
    }

    /// Bring each of `ids` that waits in a Snoozed folder back, at `now`: to the folder it came
    /// from, or the Inbox when that is gone (or is in another account), unread, marked as having
    /// come back. The received date is as it was. A message that can't be moved stays where it
    /// is, still waiting. Answers the ones that came back.
    pub fn return_snoozed(&mut self, ids: &[Id], now: Timestamp) -> Vec<Id> {
        let mut back = Vec::new();
        let mut seen = HashSet::new();
        for id in ids {
            if !seen.insert(*id) {
                continue;
            }
            let Some((account, folder, snooze)) = self.message(*id).and_then(|m| m.snooze.filter(|s| s.waiting()).map(|s| (m.account, m.folder, s)))
            else {
                continue;
            };
            let snoozed = self.snoozed_folder(account);
            if snoozed != Some(folder) {
                continue;
            }
            let to = self
                .folder(snooze.from)
                .filter(|f| f.account == account && Some(f.id) != snoozed)
                .map(|f| f.id)
                .or_else(|| self.folder_of_kind(account, FolderKind::Inbox));
            let Some(to) = to else { continue };
            if self.move_each(&[(*id, to)]).is_err() {
                continue;
            }
            if let Ok(m) = self.message_mut(*id) {
                m.read = false;
                m.snooze = Some(Snooze { returned: Some(now), ..snooze });
                back.push(*id);
            }
        }
        back
    }

    /// Call off the snooze of every message that waits but isn't in its account's Snoozed folder
    /// any more (the owner moved it out, here or on another device, or the folder is gone). A
    /// message that came back keeps its mark. Answers how many were called off.
    pub fn call_off_strays(&mut self) -> usize {
        let strays: Vec<Id> = self
            .messages
            .values()
            .filter(|m| m.snooze.is_some_and(|s| s.waiting()) && self.snoozed_folder(m.account) != Some(m.folder))
            .map(|m| m.id)
            .collect();
        for id in &strays {
            if let Some(m) = self.messages.get_mut(id) {
                Arc::make_mut(m).snooze = None;
            }
        }
        strays.len()
    }

    /// Part of [`Mailbox::repair`]: an account's Snoozed folder that isn't one of its folders is
    /// forgotten, and a snooze whose message left the Snoozed folder is dropped. Answers how many
    /// snoozes were dropped.
    pub(super) fn repair_snoozes(&mut self) -> usize {
        let known: Vec<(Id, Option<Id>)> = self.accounts.iter().map(|a| (a.id, a.snoozed_folder)).collect();
        for (account, folder) in known {
            if folder.is_some() && self.snoozed_folder(account).is_none() {
                if let Ok(a) = self.account_mut(account) {
                    a.snoozed_folder = None;
                }
            }
        }
        // A returned mark on a message of nobody's is left to go with the message.
        self.call_off_strays()
    }
}
