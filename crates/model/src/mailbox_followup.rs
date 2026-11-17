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
use crate::{FlagStatus, FolderKind, Id, Message, ModelError};

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
    /// The owner dismissed it: it is over, and kept only so that undo of an unrelated step doesn't
    /// bring it back as one that is still to be raised. Nothing looks at it any more.
    #[serde(skip_serializing_if = "std::ops::Not::not", deserialize_with = "crate::sync::lenient_or_default")]
    pub dismissed: bool,
    /// When it was raised: the moment it was noticed that its time had come. `None`: it waits.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "crate::sync::lenient")]
    pub raised: Option<Timestamp>,
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

/// Whether two reminders are the same one: set the same way, whatever has become of it since
/// (raised, put off). Undo of an unrelated step keeps what has become of a reminder.
impl FollowUp {
    pub fn same_plan(&self, other: &FollowUp) -> bool {
        (self.at, self.if_no_reply, self.after_id, self.days) == (other.at, other.if_no_reply, other.after_id, other.days)
    }

    /// This reminder with what has become of it as `now` has it (raised, put off).
    pub fn carrying_state(mut self, now: &FollowUp) -> FollowUp {
        (self.raised, self.dismissed, self.postponed_until) = (now.raised, now.dismissed, now.postponed_until);
        self
    }

    /// `then` as undo brings it back, against `now`: as it was, but what has become of it since
    /// (raised, put off) stays when it is the same reminder.
    pub fn undone(then: Option<FollowUp>, now: Option<FollowUp>) -> Option<FollowUp> {
        match (then, now) {
            (Some(t), Some(n)) if t.same_plan(&n) => Some(t.carrying_state(&n)),
            (t, _) => t,
        }
    }
}

/// A reminder of a message that left the Outbox for the server's Sent Items, with the server's
/// copy still to come down: the copy is a message of its own, found by its `Message-ID`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PendingFollowUp {
    pub account: Id,
    pub message_id: String,
    pub follow_up: FollowUp,
    /// When it was sent.
    pub since: Timestamp,
}

/// How many such reminders are kept, and for how long (seconds) before one is let go of.
pub const MAX_PENDING_FOLLOW_UPS: usize = 200;
pub const PENDING_FOLLOW_UP_KEPT: i64 = 14 * 86_400;

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
        self.messages.values().any(|m| m.follow_up.is_some_and(|f| !f.dismissed))
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
        // A draft isn't mail yet: a reminder for a time can't be kept on it (saving it again
        // would drop it), and it never fires.
        if !if_no_reply && ids.iter().filter_map(|id| self.message(*id)).any(|m| m.draft) {
            return Err(invalid("a draft can't have a reminder: send it first, or use Follow Up in the message itself"));
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

    /// The owner dismissed the reminders of `ids` that are raised: they are over (the flag stays).
    /// Answers how many.
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

    /// Whether there is anything for [`Mailbox::settle_follow_ups`] to do at `now`: a reminder to
    /// raise, or to end (its flag was cleared elsewhere, its message deleted). A walk of the
    /// reminders alone, so that nothing is copied or looked through while nothing is due.
    pub fn follow_up_work(&self, now: Timestamp) -> bool {
        self.messages.values().any(|m| {
            m.follow_up.filter(|f| !f.dismissed).is_some_and(|f| {
                let ended = (!f.if_no_reply || f.raised.is_some()) && m.flag != FlagStatus::Flagged;
                ended || self.in_deleted_or_gone(m) || (f.raised.is_none() && f.at <= now && !self.unsent(m))
            })
        })
    }

    fn in_deleted_or_gone(&self, m: &Message) -> bool {
        self.folder(m.folder).is_none() || self.is_in_deleted(m.folder)
    }

    /// Not sent yet: a draft, or waiting in the Outbox.
    fn unsent(&self, m: &Message) -> bool {
        m.draft || self.folder(m.folder).is_some_and(|x| matches!(x.kind, FolderKind::Drafts | FolderKind::Outbox))
    }

    /// Which of the "if nobody replies" reminders `due` (message ids) somebody has answered. One
    /// look through the mail that arrived after the oldest of them: a message answers one by its
    /// headers (`In-Reply-To` or `References` name it, or a message that does: a chain), whatever
    /// its subject says and in whichever of the owner's accounts it arrived, or by the conversation
    /// id its server gave it and the message's account's server gave the message. It answers when it
    /// is from one of the people the message went to (not the owner's own address), is no draft, and
    /// is not in Sent Items, Drafts or the Outbox.
    fn answered(&self, due: &[(Id, Id)]) -> HashSet<Id> {
        let Some(oldest) = due.iter().map(|(_, after)| *after).min() else { return HashSet::new() };
        // What links to each reminded message so far: by Message-ID, and by conversation id.
        let mut by_id: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut by_thread: HashMap<(Id, &str), Vec<usize>> = HashMap::new();
        let mut people: Vec<HashSet<String>> = Vec::with_capacity(due.len());
        for (i, (id, _)) in due.iter().enumerate() {
            let Some(m) = self.message(*id) else {
                people.push(HashSet::new());
                continue;
            };
            people.push(m.recipients().map(|a| norm(&a.email)).filter(|e| !e.is_empty()).collect());
            if !m.message_id.is_empty() {
                by_id.entry(m.message_id.as_str()).or_default().push(i);
            }
            if let Some(thread) = m.server.as_ref().map(|x| x.thread.as_str()).filter(|t| !t.is_empty()) {
                by_thread.entry((m.account, thread)).or_default().push(i);
            }
        }
        let mine: HashSet<String> = self.accounts.iter().map(|a| norm(&a.email)).filter(|e| !e.is_empty()).collect();
        let mut chained: HashMap<String, Vec<usize>> = HashMap::new();
        let mut answered = HashSet::new();
        let Some(from) = oldest.checked_add(1) else { return answered };
        for (_, r) in self.messages.range(from..) {
            let mut hits: Vec<usize> = Vec::new();
            let links = Some(r.in_reply_to.as_str())
                .filter(|l| !l.is_empty())
                .into_iter()
                .chain(r.references.iter().rev().take(crate::thread_index::MAX_REFERENCES).map(String::as_str));
            for link in links {
                hits.extend(by_id.get(link).into_iter().flatten());
                hits.extend(chained.get(link).into_iter().flatten());
            }
            if let Some(thread) = r.server.as_ref().map(|x| x.thread.as_str()).filter(|t| !t.is_empty()) {
                hits.extend(by_thread.get(&(r.account, thread)).into_iter().flatten());
            }
            hits.sort_unstable();
            hits.dedup();
            if hits.is_empty() {
                continue;
            }
            // A chain goes on through this message, whoever wrote it.
            if !r.message_id.is_empty() && chained.len() < 100_000 {
                chained.entry(r.message_id.clone()).or_default().extend(hits.iter().copied());
            }
            let from = norm(&r.from.email);
            let counts = !r.draft
                && !mine.contains(&from)
                && !self.folder(r.folder).is_some_and(|f| matches!(f.kind, FolderKind::Sent | FolderKind::Drafts | FolderKind::Outbox));
            if counts {
                for i in hits {
                    if due.get(i).is_some_and(|(_, after)| r.id > *after) && people.get(i).is_some_and(|p| p.contains(&from)) {
                        answered.insert(due[i].0);
                    }
                }
            }
        }
        answered
    }

    /// Bring the reminders up to date with `now` and with the mail: end those whose flag was
    /// cleared or completed elsewhere, or whose message was deleted; cancel "if nobody replies"
    /// ones that somebody has replied to; raise those whose time has come (flagging the message
    /// for "if nobody replies"), once. A message not sent yet (a draft, in the Outbox) waits.
    /// Cancelling is looked at before raising, so a reply that arrived before the time is never
    /// missed. Changes nothing, and copies nothing, when there is nothing to do.
    pub fn settle_follow_ups(&mut self, now: Timestamp) -> FollowUpsSettled {
        let mut settled = FollowUpsSettled::default();
        if !self.follow_up_work(now) {
            return settled;
        }
        let with: Vec<Id> = self.messages.values().filter(|m| m.follow_up.is_some_and(|f| !f.dismissed)).map(|m| m.id).collect();
        // Due, "if nobody replies": looked through the mail once, for all of them.
        let mut asking: Vec<(Id, Id)> = Vec::new();
        let mut due: Vec<Id> = Vec::new();
        for id in with {
            let Some(m) = self.message(id) else { continue };
            let Some(f) = m.follow_up else { continue };
            if ((!f.if_no_reply || f.raised.is_some()) && m.flag != FlagStatus::Flagged) || self.in_deleted_or_gone(m) {
                settled.ended.push(id);
                continue;
            }
            if f.raised.is_some() || f.at > now || self.unsent(m) {
                continue;
            }
            if f.if_no_reply {
                asking.push((id, f.after_id));
            }
            due.push(id);
        }
        let answered = self.answered(&asking);
        for id in due {
            if answered.contains(&id) {
                settled.cancelled.push(id);
            } else {
                settled.raised.push(id);
            }
        }
        for id in settled.ended.iter().chain(&settled.cancelled) {
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

    /// The message `id` has just been sent (it is out of the Outbox, or the server has it in Sent
    /// Items): a reminder set as "after so many days" counts them from now.
    pub fn follow_up_sent(&mut self, id: Id, now: Timestamp) {
        if let Ok(m) = self.message_mut(id)
            && let Some(f) = m.follow_up.as_mut().filter(|f| f.days > 0 && f.raised.is_none())
        {
            f.at = now.saturating_add(i64::from(f.days).saturating_mul(86_400));
        }
    }

    /// The message `id` is about to be forgotten here because the server files its copy in Sent
    /// Items, to come down as a message of its own: its reminder waits for that copy, by `Message-ID`.
    pub fn keep_follow_up_for_copy(&mut self, id: Id, now: Timestamp) {
        let Some(m) = self.message(id) else { return };
        let Some(f) = m.follow_up.filter(|f| f.if_no_reply && f.raised.is_none()) else { return };
        if m.message_id.is_empty() {
            return;
        }
        let entry = PendingFollowUp { account: m.account, message_id: m.message_id.clone(), follow_up: f, since: now };
        self.follow_ups_pending
            .retain(|p| now.saturating_sub(p.since) < PENDING_FOLLOW_UP_KEPT && !(p.account == entry.account && p.message_id == entry.message_id));
        if self.follow_ups_pending.len() < MAX_PENDING_FOLLOW_UPS {
            self.follow_ups_pending.push(entry);
        }
    }

    /// A message that has come down takes the reminder kept for it, if any: only a message in
    /// Sent Items of that account, from one of the owner's addresses, dated within the time a
    /// reminder is kept of when it was sent (a copy in the Inbox, a list's echo or a forged one
    /// takes nothing).
    pub(super) fn take_pending_follow_up(&mut self, m: &mut Message) {
        if self.follow_ups_pending.is_empty() || m.message_id.is_empty() || m.follow_up.is_some() {
            return;
        }
        let in_sent = self.folder(m.folder).is_some_and(|f| f.account == m.account && f.kind == FolderKind::Sent);
        let own = self.accounts.iter().any(|a| !norm(&a.email).is_empty() && norm(&a.email) == norm(&m.from.email));
        if !in_sent || !own {
            return;
        }
        let Some(i) = self
            .follow_ups_pending
            .iter()
            .position(|p| p.account == m.account && p.message_id == m.message_id && m.date.abs_diff(p.since) < PENDING_FOLLOW_UP_KEPT.unsigned_abs())
        else {
            return;
        };
        m.follow_up = Some(self.follow_ups_pending.remove(i).follow_up);
    }

    /// Let go of reminders kept too many, too long or with a `Message-ID` too long (a file can
    /// say anything); answers whether any went.
    pub(super) fn repair_follow_ups_pending(&mut self) -> bool {
        let n = self.follow_ups_pending.len();
        self.follow_ups_pending.retain(|p| !p.message_id.is_empty() && p.message_id.len() <= 998);
        self.follow_ups_pending.sort_by_key(|p| std::cmp::Reverse(p.since));
        self.follow_ups_pending.truncate(MAX_PENDING_FOLLOW_UPS);
        n != self.follow_ups_pending.len()
    }

    /// Follow Up with a date was chosen for `ids` (`due`: when the flag is due): a reminder for a
    /// time that was set moves to it, when it is ahead, and otherwise ends: the later choice wins,
    /// and the flag and the reminder never name two times. "If nobody replies" ones are not about
    /// the flag's date and stay.
    pub fn follow_up_due_chosen(&mut self, ids: &[Id], due: Timestamp, now: Timestamp) {
        for id in ids {
            if self.message(*id).and_then(|m| m.follow_up).is_none_or(|f| f.if_no_reply) {
                continue;
            }
            if let Ok(m) = self.message_mut(*id) {
                m.follow_up = (due > now).then(|| FollowUp { at: due, ..FollowUp::new(due, false, 0) });
            }
        }
    }
}
