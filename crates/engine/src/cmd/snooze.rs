// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Snooze (#949): put a message out of the way until a Return time. It waits in the account's
//! Snoozed folder, then comes back by itself to the folder it came from, unread and marked.
//!
//! The moves are the mailbox's own ([`mailcraft_model::Mailbox::move_each`]), and the folder is
//! made as any folder the owner makes, so what each kind of account tells its server is what it
//! tells for any move and any new folder. Nothing here does I/O. The Return time is kept in the
//! mailbox file only: it never leaves this computer.
//!
//! - `mail.snooze` and `mail.unsnooze` (Return now) are each one step of the history.
//! - `mail.returnDue` is the return step the window runs every minute and at start-up: it isn't a
//!   step of the history (nobody did it), and it also calls off the snooze of mail that was moved
//!   out of the Snoozed folder by other means.
//! - Taking in a run's news ([`mark`], [`after`]) brings a snoozed message back at once when mail
//!   of its conversation arrived, and calls off the snooze of mail the server shows elsewhere.

use mailcraft_model::{FolderKind, Id, MAX_SNOOZE_AHEAD, Mailbox, ThreadKey};
use serde_json::{Value, json};
use std::collections::HashSet;

use super::{bad, fail, needs_message, needs_movable, select_after_removal, targets, targets_whole};
use crate::{CmdResult, CommandSpec, Session};

/// The most messages one `mail.returnDue` brings back; the rest come at the next one, a minute on.
pub const MOST_RETURNED_AT_ONCE: usize = 1_000;
/// The most new messages of one run looked at for the conversations they are in.
const MOST_ARRIVALS_LOOKED_AT: usize = 20_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.snooze", "Snooze", "Home › Tags", snooze)
            .params(
                r#"{"until": number (seconds since 1970, UTC) | "YYYY-MM-DDTHH:MM" (this computer's time, or with an offset), "ids"?: [number]} (default: the selected messages; for a conversation's row selected (closed), the messages it shows). The messages move to the account's Snoozed folder (made when missing) and come back at `until`, unread, to the folder they came from. `until` must be in the future and is brought back to five years ahead. Refused for a message being written, one in an Outbox, and one a JMAP account's server has (it can't be moved yet)"#,
            )
            .selection(needs_movable),
        CommandSpec::new("mail.unsnooze", "Return Now", "Home › Tags", unsnooze)
            .params(r#"{"ids"?: [number]} (default: the selected messages) — snoozed messages come back now, as at their Return time. Refused when none is snoozed"#)
            .selection(needs_message),
        CommandSpec::new("mail.returnDue", "Return Snoozed Mail", "Send / Receive", return_due).pure().params(
            r#"{} — brings back every snoozed message whose Return time has come (at most 1,000 at a time), also those that came due while MailCraft was closed, and calls off the snooze of mail moved out of the Snoozed folder. Answers `returned` (the messages, to alert about) and `calledOff`"#,
        ),
    ]
}

/// The Return time `until` names: a number of seconds since 1970 (UTC) or an ISO date and time.
fn until(s: &Session, v: &Value) -> Result<i64, crate::CmdError> {
    let now = s.now();
    let wrong = || bad("`until` is a time: seconds since 1970 (UTC), or a date and time like 2026-10-11T09:00");
    let at = match v.get("until") {
        Some(Value::Number(n)) => n.as_i64().ok_or_else(wrong)?,
        Some(Value::String(text)) => {
            let (wall, offset) = mailcraft_time::parse_iso(text).ok_or_else(wrong)?;
            wall.to_timestamp(offset.unwrap_or_else(|| s.offset_at(now)))
        }
        _ => return Err(wrong()),
    };
    if at <= now {
        return Err(bad("`until` must be in the future"));
    }
    Ok(at.min(now.saturating_add(MAX_SNOOZE_AHEAD)))
}

fn snooze(s: &mut Session, v: &Value) -> CmdResult {
    let at = until(s, v)?;
    let ids = targets_whole(s, v)?;
    let before = s.rows();
    let name = s.interface_language.t("Snoozed").to_string();
    // Taken whole: a refusal leaves the mailbox as it was.
    let count = s.mailbox.snooze_messages(&ids, at, &name)?;
    select_after_removal(s, &before, &ids);
    let folders: Vec<Id> = ids.iter().filter_map(|id| s.mailbox.message(*id).map(|m| m.folder)).collect::<HashSet<_>>().into_iter().collect();
    Ok(json!({"snoozed": count, "until": at, "folders": folders}))
}

fn unsnooze(s: &mut Session, v: &Value) -> CmdResult {
    let ids = targets(s, v)?;
    let before = s.rows();
    let now = s.now();
    let back = s.mailbox.return_snoozed(&ids, now);
    if back.is_empty() {
        return Err(fail("none of these messages is snoozed"));
    }
    select_after_removal(s, &before, &back);
    Ok(json!({"returned": back}))
}

fn return_due(s: &mut Session, _: &Value) -> CmdResult {
    let called_off = s.mailbox.call_off_strays();
    let now = s.now();
    let mut due = s.mailbox.due_snoozes(now);
    due.truncate(MOST_RETURNED_AT_ONCE);
    let back = s.mailbox.return_snoozed(&due, now);
    Ok(json!({"returned": back, "calledOff": called_off}))
}

/// What a run's news is compared with ([`after`]): the newest message id before it was taken in.
/// Ids are handed out in increasing order and never again, so a message with a higher one arrived
/// since.
pub(crate) struct Mark(Id);

/// Mark the mailbox before a run's news is taken in.
pub(crate) fn mark(mailbox: &Mailbox) -> Mark {
    Mark(mailbox.messages.keys().next_back().copied().unwrap_or(0))
}

/// After a run's news was taken in: a snooze is called off for mail the server shows outside the
/// Snoozed folder (moved by hand elsewhere), and a message that waits comes back at once when mail
/// of its conversation arrived (a reply). Mail that only says what the owner did does not count:
/// what was sent, drafts, and what lies in Deleted Items, Junk E-mail or the Snoozed folder.
pub(crate) fn after(s: &mut Session, since: &Mark) {
    if !s.mailbox.has_snoozed() {
        return;
    }
    s.mailbox.call_off_strays();
    let Some(from) = since.0.checked_add(1) else { return };
    let mb = &s.mailbox;
    let arrived: Vec<Id> = mb
        .messages
        .range(from..)
        .map(|(id, _)| *id)
        .filter(|id| {
            let Some(m) = mb.message(*id) else { return false };
            let Some(folder) = mb.folder(m.folder) else { return false };
            let own = matches!(folder.kind, FolderKind::Sent | FolderKind::Drafts | FolderKind::Outbox | FolderKind::Deleted | FolderKind::Junk);
            !m.draft && !own && mb.snoozed_folder(m.account) != Some(m.folder)
        })
        .take(MOST_ARRIVALS_LOOKED_AT)
        .collect();
    if arrived.is_empty() {
        return;
    }
    let threads = s.threads();
    let replied: HashSet<(Id, ThreadKey)> = arrived.iter().filter_map(|id| s.mailbox.message(*id).map(|m| (m.account, threads.key(*id)))).collect();
    let waiting: Vec<Id> = s
        .mailbox
        .messages
        .values()
        .filter(|m| m.snooze.is_some_and(|z| z.waiting()) && replied.contains(&(m.account, threads.key(m.id))))
        .map(|m| m.id)
        .collect();
    if !waiting.is_empty() {
        let now = s.now();
        s.mailbox.return_snoozed(&waiting, now);
    }
}
