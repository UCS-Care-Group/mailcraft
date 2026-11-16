// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send later and Undo Send (#950, #1363).
//!
//! A message in an Outbox may carry a **Send time** ([`Message::send_at`]). The runs that send
//! from an Outbox leave out what waits for it ([`Message::waits_until`], asked with the clock's
//! `now` at each run), for every kind of account; nothing else decides. So a message waits across
//! a restart, goes at the next run after its time when MailCraft wasn't running then, and never
//! goes early: a clock set back only makes it wait longer.
//!
//! - `compose.delay` (Options › Delay Delivery) gives the message being written a time;
//!   `compose.sendLater` gives it one and sends (Send Later…).
//! - `mail.setSendTime` changes the time of a queued message, `mail.cancelSend` takes it out of
//!   the Outbox into Drafts, and Open Draft does the same to write it again.
//! - **Undo Send** is the same mechanism: with `sync.waitBeforeSending` on, Send gives the message a
//!   time a few seconds ahead, and `compose.undoSend` takes it back while it has not come.
//!
//! A Microsoft account's message waits here too and is sent by MailCraft at its time: the server
//! is not asked to hold it. Nothing here touches a server.

use mailcraft_model::{FolderKind, Id, Message};
use serde_json::{Value, json};

use super::{bad, fail, needs_compose};
use crate::{CmdError, CmdResult, CommandSpec, Prefs, Session, p};

/// How soon a Send time may be: a minute from now.
pub const MIN_AHEAD: i64 = 60;
/// How far ahead a Send time may be: a year (and a day).
pub const MAX_AHEAD: i64 = 366 * 24 * 3600;
/// A message that goes this long after its Send time went late (MailCraft wasn't running, or the
/// computer slept): the status bar says so.
pub const LATE_AFTER: i64 = 120;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("compose.delay", "Delay Delivery", "Options › More Options", delay)
            .pure()
            .params(r#"{"at": number | null} — the message being written is not sent before this time (seconds since 1970, in the future and within a year); null takes the delay off. Kept with its draft. Answers {"sendAt": number | null}"#)
            .when_writing(needs_compose),
        CommandSpec::new("compose.sendLater", "Send Later", "Message", send_later)
            .params(r#"{"at": number} — Send, but not before that time (seconds since 1970, in the future and within a year): the message waits in the Outbox, can be changed or cancelled there, and goes at its time if MailCraft is running, else at the next run after it. Answers as Send does, with "sendAt""#)
            .when_writing(needs_compose),
        CommandSpec::new("mail.setSendTime", "Change Send Time", "Mail", set_time)
            .params(r#"{"id": number, "at": number | null} — a message waiting in the Outbox goes not before `at` (seconds since 1970, in the future and within a year); null lets it go at the next run. One that is being sent, held, or that the server delivered is refused. Answers {"id", "sendAt"}"#),
        CommandSpec::new("mail.cancelSend", "Cancel Send", "Mail", cancel)
            .params(r#"{"id": number} — a message waiting in the Outbox goes back to Drafts, without its time, and is not sent. One that is being sent, held, or that the server delivered is refused. Answers {"id", "draft": true}"#),
        CommandSpec::new("compose.undoSend", "Undo Send", "Message", undo_send)
            .params(r#"{} — takes back the message that Send put in the Outbox for the wait before sending (sync.waitBeforeSending), while its time has not come: it goes back to Drafts and opens to be written again. Refused when nothing waits or it is on its way. Answers as Open Draft does"#),
        CommandSpec::new("sync.waitBeforeSending", "Wait Before Sending", "Send / Receive › Preferences", wait_before_sending)
            .pure()
            .params(r#"{"seconds": 0 | 5 | 10 | 15 | 20 | 30} — how long a message waits in the Outbox after Send, with Undo Send; 0 (as it is until the owner turns it on) sends at once. Kept between runs; not an undo step. Answers {"seconds"}"#),
    ]
}

/// A Send time named by `at`: in the future and within [`MAX_AHEAD`]; `None` for `null` when
/// `allow_none` (no time at all), else refused. A time that is not a whole number is refused.
fn time_of(s: &Session, v: &Value, allow_none: bool) -> Result<Option<i64>, CmdError> {
    match v.get("at") {
        Some(Value::Null) if allow_none => Ok(None),
        None if allow_none => Err(bad("`at` is the time, or null for none")),
        Some(_) => {
            let at = p::i64(v, "at").ok_or_else(|| bad("`at` is a time in seconds since 1970"))?;
            let now = s.now();
            if at <= now {
                return Err(fail("that time has passed: choose a time in the future"));
            }
            if at < now.saturating_add(MIN_AHEAD) {
                return Err(fail("that is too soon: pick a time at least a minute from now (or send it now)"));
            }
            if at > now.saturating_add(MAX_AHEAD) {
                return Err(fail("choose a time within a year"));
            }
            Ok(Some(at))
        }
        None => Err(bad("`at` is required")),
    }
}

fn delay(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["at"])?;
    let at = time_of(s, v, true)?;
    let c = s.compose.as_mut().ok_or_else(|| fail("no message is being written"))?;
    if c.send_at != at {
        c.send_at = at;
        c.dirty = true;
    }
    Ok(json!({"sendAt": at}))
}

fn send_later(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["at"])?;
    let at = time_of(s, v, false)?;
    let c = s.compose.as_mut().ok_or_else(|| fail("no message is being written"))?;
    c.send_at = at;
    super::compose::send_now(s)
}

/// Whether `m` is a message of its account's Outbox that waits there for the owner's Send time
/// or the next run: queued here by Send (a message the server delivered into that folder, or one
/// moved in and held, is not), and not handed to a server.
pub fn waits_in_outbox(s: &Session, m: &Message) -> bool {
    let mb = &s.mailbox;
    mb.folder(m.folder).is_some_and(|f| f.kind == FolderKind::Outbox) && !m.sending && m.held.is_empty() && !super::graph_send::from_server(m)
}

/// The queued message `id`, or why it can't be changed.
fn queued(s: &Session, id: Id) -> Result<Message, CmdError> {
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    if m.sending {
        return Err(fail("that message is being sent now"));
    }
    if !waits_in_outbox(s, m) {
        return Err(fail("that message isn't waiting in the Outbox"));
    }
    Ok(Message::clone(m))
}

/// A strictly read `id` of one message.
fn one(v: &Value) -> Result<Id, CmdError> {
    p::message_strict(v)?.ok_or_else(|| bad("`id` is required"))
}

fn set_time(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "at"])?;
    let id = one(v)?;
    let at = time_of(s, v, true)?;
    queued(s, id)?;
    let now = s.now();
    let m = s.mailbox.message_mut(id)?;
    m.send_at = at;
    // Its Date is the time it goes (the same octets at every run); one let go now is dated now.
    (m.date, m.received) = (at.unwrap_or(now), at.unwrap_or(now));
    if at.is_none() && s.undo_send == Some(id) {
        s.undo_send = None;
    }
    s.mailbox_changed();
    Ok(json!({"id": id, "sendAt": at}))
}

/// Take the message `id` out of its Outbox into its account's Drafts, to be written again or left
/// there: without its time, and never sent unless Send is pressed again. Public for Open Draft.
pub fn cancel_send(s: &mut Session, id: Id) -> Result<(), CmdError> {
    let m = queued(s, id)?;
    let drafts = s.mailbox.folder_of_kind(m.account, FolderKind::Drafts).ok_or_else(|| fail("this account has no Drafts folder"))?;
    s.mailbox.move_message(id, drafts)?;
    let now = s.now();
    let m = s.mailbox.message_mut(id)?;
    // No time stays on the draft (it can be given one again), and it is dated now.
    m.draft = true;
    // Written again it is an ordinary message, whose text is the emoji (#1544).
    m.reaction = false;
    m.held.clear();
    (m.send_at, m.date, m.received) = (None, now, now);
    if s.undo_send == Some(id) {
        s.undo_send = None;
    }
    s.mailbox_changed();
    Ok(())
}

fn cancel(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id"])?;
    let id = one(v)?;
    cancel_send(s, id)?;
    Ok(json!({"id": id, "draft": true}))
}

fn undo_send(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &[])?;
    let (id, _) = s.undo_send_wait().ok_or_else(|| fail("nothing is waiting to be sent: it is on its way already"))?;
    cancel_send(s, id)?;
    super::compose::open_draft(s, id)
}

fn wait_before_sending(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["seconds"])?;
    let n = p::u64(v, "seconds").and_then(|n| u8::try_from(n).ok()).filter(|n| Prefs::UNDO_SEND_CHOICES.contains(n));
    let n = n.ok_or_else(|| bad("`seconds` is 0, 5, 10, 15, 20 or 30"))?;
    s.prefs.undo_send_secs = n;
    Ok(json!({"seconds": n}))
}

/// What the status bar says of delayed mail ([`Session::send_later_note`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendLaterNote {
    /// The earliest Send time that has not come, and how many messages wait for one.
    pub next: Option<i64>,
    pub waiting: usize,
    /// Messages whose time came more than [`LATE_AFTER`] seconds ago and that haven't gone yet:
    /// MailCraft wasn't running, or the computer slept, and they go now.
    pub late: usize,
    /// The account of one of the late messages, for saying why it didn't go.
    pub late_account: Option<Id>,
}

impl Session {
    /// The message that Send put in the Outbox for the wait before sending, and its time, while
    /// the time has not come and it is not being sent: what the Undo bar offers.
    pub fn undo_send_wait(&self) -> Option<(Id, i64)> {
        let id = self.undo_send?;
        let m = self.mailbox.message(id)?;
        let at = m.send_at.filter(|at| *at > self.now())?;
        waits_in_outbox(self, m).then_some((id, at))
    }

    /// Whether the Send that put the message in the wait is the last thing done (nothing else was a
    /// step of the history since): then Undo (Ctrl+Z) takes the send back, else it undoes that step.
    pub fn undo_send_latest(&self) -> bool {
        self.undo_send_step.is_some() && self.undo_send_step == Some(self.undo_steps_moved())
    }

    /// Delayed mail in the Outboxes: the next time, how many wait, and how many are late. `None`
    /// when no message has a Send time.
    pub fn send_later_note(&self) -> Option<SendLaterNote> {
        let now = self.now();
        let mut note = SendLaterNote::default();
        // (What waits for Undo Send isn't delayed mail: the bar says so.)
        for m in self.mailbox.messages.values().filter(|m| m.send_at.is_some() && Some(m.id) != self.undo_send && waits_in_outbox(self, m)) {
            let Some(at) = m.send_at else { continue };
            if at > now {
                note.waiting = note.waiting.saturating_add(1);
                note.next = Some(note.next.map_or(at, |n| n.min(at)));
            } else if now.saturating_sub(at) >= LATE_AFTER {
                note.late = note.late.saturating_add(1);
                note.late_account.get_or_insert(m.account);
            }
        }
        (note != SendLaterNote::default()).then_some(note)
    }
}

#[cfg(test)]
#[path = "send_later_tests.rs"]
mod send_later_tests;
