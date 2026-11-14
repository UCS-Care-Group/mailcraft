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
//! - Taking in a run's news ([`after`]) brings a snoozed message back at once when mail of its
//!   conversation arrived, and calls off the snooze of mail the server shows elsewhere. Mail is of
//!   its conversation only by a link that is known: it answers the message (`In-Reply-To`,
//!   `References`) or has the server's conversation id the message has.
//! - Snoozing a JMAP account's server mail isn't built yet, so a message it has is refused.

use mailcraft_model::{FolderKind, Id, MAX_SNOOZE_AHEAD, Mailbox, Message};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

use super::{bad, fail, needs_message, select_after_removal, targets, targets_whole};

/// The years a Return time given as a date and time may be in, as an automatic reply's are.
const FIRST_YEAR: i32 = 1990;
const LAST_YEAR: i32 = 2200;
use crate::{CmdResult, CommandSpec, Session};

/// Enabled only with messages selected that may be snoozed: none that a JMAP account's server has, for now.
fn needs_snoozable(s: &Session) -> Option<&'static str> {
    needs_message(s).or_else(|| s.mailbox.snooze_refusal(&s.sel.messages))
}

/// The most messages one `mail.returnDue` brings back; the rest come at the next one, a minute on.
pub const MOST_RETURNED_AT_ONCE: usize = 1_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.snooze", "Snooze", "Home › Tags", snooze)
            .params(
                r#"{"until": number (seconds since 1970, UTC) | "YYYY-MM-DDTHH:MM" (this computer's time, or with an offset), "ids"?: [number]} (default: the selected messages; for a conversation's row selected (closed), the messages it shows). The messages move to the account's Snoozed folder (made when missing) and come back at `until`, unread, to the folder they came from. `until` must be in the future and is brought back to five years ahead. Refused for a message being written, one in an Outbox, and one a JMAP account's server has (it can't be moved yet)"#,
            )
            .selection(needs_snoozable),
        CommandSpec::new("mail.unsnooze", "Return Now", "Home › Tags", unsnooze)
            .params(r#"{"ids"?: [number]} (default: the selected messages) — snoozed messages come back now, as at their Return time. Refused when none is snoozed"#)
            .selection(needs_message),
        CommandSpec::new("mail.returnDue", "Return Snoozed Mail", "Send / Receive", return_due).pure().params(
            r#"{} — brings back every snoozed message whose Return time has come (at most 1,000 at a time), also those that came due while MailCraft was closed, and calls off the snooze of mail moved out of the Snoozed folder. Answers `returned` (the messages, to alert about) and `calledOff`"#,
        ),
    ]
}

/// The Return time `until` names: a number of seconds since 1970 (UTC), or a date and time like
/// 2026-10-11T09:00: in the zone the calendar is shown in (so that a time across a change of the
/// clocks is the wall-clock time written), or with its own offset.
fn until(s: &Session, v: &Value) -> Result<i64, crate::CmdError> {
    let now = s.now();
    let wrong = || bad("`until` is a time: seconds since 1970 (UTC), or a date and time like 2026-10-11T09:00");
    let at = match v.get("until") {
        Some(Value::Number(n)) => n.as_i64().ok_or_else(wrong)?,
        Some(Value::String(text)) => {
            let (wall, offset) = (text.len() <= 40).then(|| mailcraft_time::parse_iso(text)).flatten().ok_or_else(wrong)?;
            if !(FIRST_YEAR..=LAST_YEAR).contains(&wall.date.year) {
                return Err(wrong());
            }
            match offset {
                Some(offset) => wall.to_timestamp(offset),
                None => s.instant_of(wall),
            }
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
    let settled = s.mailbox.settle_snoozes();
    let now = s.now();
    let mut due = s.mailbox.due_snoozes(now);
    due.truncate(MOST_RETURNED_AT_ONCE);
    let back = s.mailbox.return_snoozed(&due, now);
    Ok(json!({"returned": back, "calledOff": settled.called_off, "reset": settled.reset, "gaveUp": settled.gave_up}))
}

/// Add what [`after`] has to say to a run's problems (`errors` of its answer), and give it
/// alone as `said`, for a host that keeps the problems of a piece apart from its answer's.
pub(crate) fn tell(answer: &mut CmdResult, said: Vec<String>) {
    if said.is_empty() {
        return;
    }
    if let Ok(Value::Object(answer)) = answer
        && let Value::Array(errors) = answer.entry("errors").or_insert_with(|| json!([]))
    {
        errors.extend(said.iter().cloned().map(Value::String));
    }
    if let Ok(Value::Object(answer)) = answer {
        answer.insert("said".into(), json!(said));
    }
}

/// After a run's news was taken in: the snoozes are put right with where the mail is now (called
/// off for mail the server shows outside the Snoozed folder; waiting again for mail whose move
/// back the server refused, three times at most), and a message that waits comes back at once
/// when mail of its conversation arrived since it was snoozed. The mailbox is marked changed when
/// anything was. Answers what the run is to tell the owner: a snooze given up because the server
/// would not move the message back.
///
/// Mail only says what the owner did, and is no news, when it was sent, is a draft, lies in
/// Deleted Items, Junk E-mail, Drafts, an Outbox or the Snoozed folder, or is from the account's
/// own address (a copy to themselves, a list's echo). It is of the conversation only by a known
/// link: it answers the snoozed message by `In-Reply-To` or `References`, or the server gave both
/// the same conversation id. It arrived after the snooze when its id is higher than the newest
/// there was then ([`mailcraft_model::Snooze::after_id`]), whatever date its server gave it.
/// Nothing is remembered between runs: mail that got its conversation id in a later piece of a
/// run is found then.
pub(crate) fn after(s: &mut Session) -> Vec<String> {
    if !s.mailbox.has_snoozes() {
        return Vec::new();
    }
    let lang = s.interface_language;
    let settled = s.mailbox.settle_snoozes();
    let problems: Vec<String> = settled
        .gave_up
        .iter()
        .map(|id| {
            let subject = s.mailbox.message(*id).map(|m| m.subject.clone()).unwrap_or_default();
            let subject = if subject.trim().is_empty() { lang.t("(no subject)").to_string() } else { subject };
            mailcraft_i18n::fmt(
                lang.t("'{subject}' stays in the Snoozed folder: the server would not move it back, so it is no longer snoozed."),
                &[("subject", &subject)],
            )
        })
        .collect();
    let mut changed = settled.any();
    let mb = &s.mailbox;
    let waiting: Vec<&Message> = mb.messages.values().map(|m| m.as_ref()).filter(|m| m.snooze.is_some_and(|z| z.waiting())).collect();
    let back = match waiting.iter().filter_map(|m| m.snooze.map(|z| z.after_id)).min() {
        Some(oldest) => arrived_for(mb, &waiting, oldest),
        None => Vec::new(),
    };
    if !back.is_empty() {
        let now = s.now();
        changed |= !s.mailbox.return_snoozed(&back, now).is_empty();
    }
    if changed {
        s.mailbox_changed();
    }
    problems
}

/// The waiting messages that mail with an id above `oldest` is of the conversation of.
fn arrived_for(mb: &Mailbox, waiting: &[&Message], oldest: Id) -> Vec<Id> {
    let Some(from) = oldest.checked_add(1) else { return Vec::new() };
    let mut by_id: HashMap<(Id, &str), Vec<Id>> = HashMap::new();
    let mut by_thread: HashMap<(Id, &str), Vec<Id>> = HashMap::new();
    for m in waiting {
        if !m.message_id.is_empty() {
            by_id.entry((m.account, m.message_id.as_str())).or_default().push(m.id);
        }
        if let Some(thread) = m.server.as_ref().map(|x| x.thread.as_str()).filter(|t| !t.is_empty()) {
            by_thread.entry((m.account, thread)).or_default().push(m.id);
        }
    }
    // Looked up once: a run may bring thousands of messages.
    let kinds: HashMap<Id, FolderKind> = mb.folders.iter().map(|f| (f.id, f.kind)).collect();
    let snoozed: HashMap<Id, Option<Id>> = mb.accounts.iter().map(|a| (a.id, mb.snoozed_folder(a.id))).collect();
    let emails: HashMap<Id, &str> = mb.accounts.iter().map(|a| (a.id, a.email.trim())).collect();
    let after_id = |id: Id| mb.message(id).and_then(|m| m.snooze).map_or(Id::MAX, |z| z.after_id);
    let mut back: HashSet<Id> = HashSet::new();
    for m in mb.messages.range(from..).map(|(_, m)| m).filter(|m| !m.draft) {
        let Some(kind) = kinds.get(&m.folder) else { continue };
        let own = matches!(kind, FolderKind::Sent | FolderKind::Drafts | FolderKind::Outbox | FolderKind::Deleted | FolderKind::Junk);
        if own || snoozed.get(&m.account).copied().flatten() == Some(m.folder) {
            continue;
        }
        if emails.get(&m.account).is_some_and(|mine| !mine.is_empty() && mine.eq_ignore_ascii_case(m.from.email.trim())) {
            continue;
        }
        let linked = Some(m.in_reply_to.as_str()).filter(|id| !id.is_empty()).into_iter().chain(m.references.iter().map(String::as_str));
        let mut hits: Vec<Id> = linked.filter_map(|id| by_id.get(&(m.account, id))).flatten().copied().collect();
        if let Some(thread) = m.server.as_ref().map(|x| x.thread.as_str()).filter(|t| !t.is_empty()) {
            hits.extend(by_thread.get(&(m.account, thread)).into_iter().flatten().copied());
        }
        back.extend(hits.into_iter().filter(|id| *id != m.id && m.id > after_id(*id)));
    }
    back.into_iter().collect()
}
