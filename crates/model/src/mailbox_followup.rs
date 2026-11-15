// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Follow-up reminders (#948): a flag with a time, or "remind me if nobody replies". What a message
//! remembers of it is a [`FollowUp`]. It is kept in the mailbox file and nowhere else: the time
//! never leaves this computer (the flag itself goes to a server as any flag does).
//!
//! - **At a time** ([`FollowUp::if_no_reply`] off): the message is flagged now; at `at` the
//!   reminder is raised.
//! - **If nobody replies** (on; for a message the owner sent): nothing is flagged until `at`. If a
//!   message arrives in the conversation from one of the recipients first, the reminder is
//!   cancelled (gone, not hidden); if not, at `at` it is raised and the message flagged.
//!
//! A reminder is raised once ([`FollowUp::raised`]), also when its time came while MailCraft was
//! closed (the next [`Mailbox::settle_follow_ups`] raises it). Dismissing it, or clearing the flag
//! or marking the message complete, ends it. Conversations are [`crate::ThreadIndex`]'s. Everything
//! here reads mail, which is hostile: work is linear in the messages, and the time is bounded.

use std::collections::{HashMap, HashSet};

use mailcraft_time::Timestamp;
use serde::{Deserialize, Serialize};

use super::{Mailbox, invalid, not_found};
use crate::{FlagStatus, FolderKind, Id, Message, ModelError, ThreadIndex};

/// The furthest a reminder may lie from now, in seconds: five years. Beyond it, it is brought to it.
pub const MAX_FOLLOW_UP_AHEAD: i64 = 5 * 366 * 86_400;

/// What a message remembers of its follow-up reminder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FollowUp {
    /// When it is raised.
    pub at: Timestamp,
    /// Remind only if nobody has replied by `at` ([`Mailbox::set_follow_up`] allows it for mail
    /// the owner sent).
    #[serde(skip_serializing_if = "std::ops::Not::not", deserialize_with = "crate::sync::lenient_or_default")]
    pub if_no_reply: bool,
    /// The newest message id there was when it was set: only mail with a higher id can be the
    /// reply that cancels it. (Ids only ever grow; a date is a server's word.)
    pub after_id: Id,
    /// For a reminder set as "after so many days" in a message being written: how many (0 when it
    /// was a date), so that a draft kept and sent later counts them from the sending.
    #[serde(skip_serializing_if = "is_zero", deserialize_with = "crate::sync::lenient_or_default")]
    pub days: u16,
    /// When it was raised: the moment it was noticed that its time had come. `None`: it waits.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub raised: Option<Timestamp>,
    /// The owner dismissed it: it is raised no more.
    #[serde(skip_serializing_if = "std::ops::Not::not", deserialize_with = "crate::sync::lenient_or_default")]
    pub dismissed: bool,
    /// Put off until this time (Snooze in the reminders window): not shown before.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub postponed_until: Option<Timestamp>,
}

fn is_zero(n: &u16) -> bool {
    *n == 0
}

impl FollowUp {
    /// A reminder for `at`.
    pub fn new(at: Timestamp, if_no_reply: bool, after_id: Id) -> FollowUp {
        FollowUp { at, if_no_reply, after_id, ..Default::default() }
    }

    /// Whether it is raised, not dismissed and not put off at `now`: shown in the reminders window.
    pub fn showing(&self, now: Timestamp) -> bool {
        self.raised.is_some() && !self.dismissed && self.postponed_until.is_none_or(|t| t <= now)
    }
}

/// What [`Mailbox::settle_follow_ups`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FollowUpsSettled {
    /// Reminders raised now (their messages flagged, for "if nobody replies").
    pub raised: Vec<Id>,
    /// "If nobody replies" reminders cancelled because somebody did.
    pub cancelled: Vec<Id>,
    /// Reminders ended because the flag was cleared or the message completed elsewhere.
    pub ended: Vec<Id>,
}

impl FollowUpsSettled {
    /// Whether anything changed.
    pub fn any(&self) -> bool {
        !(self.raised.is_empty() && self.cancelled.is_empty() && self.ended.is_empty())
    }
}

/// An address as compared: trimmed, letter case aside.
fn norm(email: &str) -> String {
    email.trim().to_lowercase()
}

impl Mailbox {
    /// Whether any message has a reminder (cheap enough to ask every minute).
    pub fn has_follow_ups(&self) -> bool {
        self.messages.values().any(|m| m.follow_up.is_some())
    }

    /// Whether `m` is mail the owner sent (in Sent Items or waiting in the Outbox), with someone
    /// to reply: the only kind "if nobody replies" is for.
    fn is_own_sent(&self, m: &Message) -> bool {
        !m.draft
            && m.recipients().any(|r| !r.email.trim().is_empty())
            && self.folder(m.folder).is_some_and(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Outbox))
    }

    /// Why `ids` can't take "if nobody replies", when one can't.
    pub fn follow_up_refusal(&self, ids: &[Id]) -> Option<&'static str> {
        ids.iter().filter_map(|id| self.message(*id)).find(|m| !self.is_own_sent(m)).map(|_| "\"if nobody replies\" is for messages you sent")
    }

    /// Give each of `ids` a reminder for `at`. At a time: the message is flagged now (a completed
    /// one is flagged again) with `at` as its due time. If nobody replies: nothing is flagged until
    /// it is raised, and only mail you sent is allowed. `at` is brought to five years ahead.
    /// All or none. Answers how many were set.
    pub fn set_follow_up(&mut self, ids: &[Id], at: Timestamp, now: Timestamp, if_no_reply: bool, days: u16) -> Result<usize, ModelError> {
        for id in ids {
            self.message(*id).ok_or(not_found("message", *id))?;
        }
        if at <= now {
            return Err(invalid("the reminder time must be in the future"));
        }
        if if_no_reply && let Some(why) = self.follow_up_refusal(ids) {
            return Err(invalid(why));
        }
        let at = at.min(now.saturating_add(MAX_FOLLOW_UP_AHEAD));
        let watermark = self.messages.keys().next_back().copied().unwrap_or(0);
        let mut done = 0;
        for id in ids {
            let m = self.message_mut(*id)?;
            let mut f = FollowUp::new(at, if_no_reply, watermark);
            f.days = if if_no_reply { days } else { 0 };
            m.follow_up = Some(f);
            if !if_no_reply {
                m.flag = FlagStatus::Flagged;
                m.flag_due = Some(at);
            }
            done += 1;
        }
        Ok(done)
    }

    /// End the reminders of `ids` (the flag stays as it is). Answers how many had one.
    pub fn clear_follow_up(&mut self, ids: &[Id]) -> usize {
        let mut cleared = 0;
        for id in ids {
            if self.message(*id).is_some_and(|m| m.follow_up.is_some())
                && let Ok(m) = self.message_mut(*id)
            {
                m.follow_up = None;
                cleared += 1;
            }
        }
        cleared
    }

    /// The owner dismissed the reminders of `ids` that are raised. Answers how many.
    pub fn dismiss_follow_up(&mut self, ids: &[Id]) -> usize {
        let mut n = 0;
        for id in ids {
            if self.message(*id).and_then(|m| m.follow_up).is_some_and(|f| f.raised.is_some() && !f.dismissed)
                && let Ok(m) = self.message_mut(*id)
                && let Some(f) = m.follow_up.as_mut()
            {
                f.dismissed = true;
                n += 1;
            }
        }
        n
    }

    /// The owner put the raised reminders of `ids` off until `until`. Answers how many.
    pub fn postpone_follow_up(&mut self, ids: &[Id], until: Timestamp) -> usize {
        let mut n = 0;
        for id in ids {
            if self.message(*id).and_then(|m| m.follow_up).is_some_and(|f| f.raised.is_some() && !f.dismissed)
                && let Ok(m) = self.message_mut(*id)
                && let Some(f) = m.follow_up.as_mut()
            {
                f.postponed_until = Some(until);
                n += 1;
            }
        }
        n
    }

    /// The messages whose reminder is showing at `now` (raised, not dismissed, not put off), the
    /// earliest first, at most 100.
    pub fn due_follow_ups(&self, now: Timestamp) -> Vec<Id> {
        let mut due: Vec<(Timestamp, Id)> =
            self.messages.values().filter_map(|m| m.follow_up.filter(|f| f.showing(now)).map(|f| (f.at, m.id))).collect();
        due.sort_unstable();
        due.truncate(100);
        due.into_iter().map(|(_, id)| id).collect()
    }

    /// Whether somebody the message `m` went to has answered in its conversation since the reminder
    /// was set: mail that is not the owner's own, a draft or waiting to go, from a recipient, in
    /// the conversation ([`ThreadIndex`]: headers, or the server's conversation id).
    fn answered(&self, m: &Message, after: Id, threads: &ThreadIndex) -> bool {
        let to: HashSet<String> = m.recipients().map(|a| norm(&a.email)).filter(|e| !e.is_empty()).collect();
        let mine = self.account(m.account).map(|a| norm(&a.email)).unwrap_or_default();
        threads.members(m.id).into_iter().filter(|id| *id > after && *id != m.id).filter_map(|id| self.message(id)).any(|r| {
            let from = norm(&r.from.email);
            !r.draft
                && from != mine
                && to.contains(&from)
                && !self.folder(r.folder).is_some_and(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Drafts | FolderKind::Outbox))
        })
    }

    /// Bring the reminders up to date with `now` and with the mail: end those whose flag was
    /// cleared or completed elsewhere; cancel "if nobody replies" ones that somebody has replied
    /// to; raise those whose time has come (flagging the message for "if nobody replies"), once.
    /// A message not sent yet (a draft, in the Outbox) waits. Cancelling is looked at before
    /// raising, so a reply that arrived before the time is never missed.
    pub fn settle_follow_ups(&mut self, now: Timestamp) -> FollowUpsSettled {
        let mut settled = FollowUpsSettled::default();
        let with: Vec<Id> = self.messages.values().filter(|m| m.follow_up.is_some()).map(|m| m.id).collect();
        let mut threads: HashMap<Id, ThreadIndex> = HashMap::new();
        for id in with {
            let Some(m) = self.message(id) else { continue };
            let Some(f) = m.follow_up else { continue };
            if (!f.if_no_reply || f.raised.is_some()) && m.flag != FlagStatus::Flagged {
                settled.ended.push(id);
                continue;
            }
            if f.raised.is_some() || m.draft {
                continue;
            }
            let waiting = self.folder(m.folder).is_some_and(|x| matches!(x.kind, FolderKind::Drafts | FolderKind::Outbox));
            if waiting {
                continue;
            }
            if f.if_no_reply {
                let account = m.account;
                let index = threads
                    .entry(account)
                    .or_insert_with(|| ThreadIndex::build(self.messages.values().map(|x| x.as_ref()).filter(|x| x.account == account)));
                if self.answered(m, f.after_id, index) {
                    settled.cancelled.push(id);
                    continue;
                }
            }
            if f.at <= now {
                settled.raised.push(id);
            }
        }
        for id in &settled.ended {
            if let Ok(m) = self.message_mut(*id) {
                m.follow_up = None;
            }
        }
        for id in &settled.cancelled {
            if let Ok(m) = self.message_mut(*id) {
                m.follow_up = None;
            }
        }
        for id in &settled.raised {
            if let Ok(m) = self.message_mut(*id)
                && let Some(f) = m.follow_up.as_mut()
            {
                f.raised = Some(now);
                if f.if_no_reply {
                    m.flag = FlagStatus::Flagged;
                }
            }
        }
        settled
    }
}
