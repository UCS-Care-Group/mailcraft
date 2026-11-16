// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Device limits** (MB3.16): what a device that can't hold a whole mailbox (a phone) keeps of
//! it ([`mailcraft_model::DeviceLimits`]). A computer has none and nothing here changes it.
//!
//! - `account.deviceLimits` reads them, sets them (a device that had none starts with a phone's
//!   defaults), or takes them away.
//! - `account.trimDevice` keeps the mailbox within them: mail older than its account's window
//!   leaves this device; past the cap of messages the oldest leave; past the cap of storage the
//!   oldest go back to their headers first, and leave only when that isn't enough. Only mail the
//!   server has as it is here may leave or be thinned ([`OfflineWindow::server_copies`]): never
//!   the Outbox, Drafts, a held message, one not uploaded yet or with a change the server hasn't
//!   heard of, nor the message shown or one fetched this session. Nothing is asked of the server
//!   and nothing goes from it: what left is listed again only if it comes within the window. Not
//!   an undo step.

use std::collections::{HashMap, HashSet};

use mailcraft_model::device_limits::{MAX_MESSAGES, MAX_STORAGE, MIN_MESSAGES, MIN_STORAGE};
use mailcraft_model::sync::MessageChange;
use mailcraft_model::{AccountKind, Id, Message, OfflineWindow, Queued};
use serde_json::{Value, json};

use super::bad;
use super::offline::{content_bytes, thin_these};
use crate::{CmdResult, CommandSpec, Session, p};

/// Most messages one `account.trimDevice` thins or takes off this device.
const MOST: usize = 100_000;
/// What a message is counted as on disk beyond its content: its headers, preview and flags.
pub const HEADER_BYTES: u64 = 1 << 10;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.deviceLimits", "Mail Kept on This Device", "File › Info", device_limits)
            .pure()
            .params(r#"{"storage"?: number (bytes of mail kept on this device, 50 MB to 64 GB), "messages"?: number (messages kept on this device, 2000 to 500000), "defaults"?: number (the version of the phone's defaults the mailbox was moved to), "off"?: true (no limits: a computer's way)} — a device that can't hold a whole mailbox (a phone): mail older than an account's mail to keep offline isn't listed from the server at all, and past either cap the oldest mail goes back to its headers, then leaves this device (never the server, never mail not sent or uploaded yet). With nothing given, changes nothing. Answers {"on": bool, "storage": number, "messages": number, "defaults": number}"#),
        CommandSpec::new("account.trimDevice", "Keep Mail Within This Device's Limits", "File › Info", trim_device)
            .pure()
            .params(r#"{"most"?: number (default and at most 100000)} — on a device with limits (account.deviceLimits): mail older than its account's mail to keep offline leaves this device; past the cap of messages the oldest leave; past the cap of storage the oldest go back to their headers and preview first, then leave. Only mail the server has as it is here: never the Outbox, Drafts, held mail, mail not uploaded or with a change waiting to go up, the message shown or one fetched this session. Nothing on the server changes. Not an undo step. Answers {"thinned": number, "dropped": number, "bytes": number (estimated, kept after), "messages": number (kept after), "left": number (still to do, past "most")}"#),
    ]
}

/// What `account.deviceLimits` answers.
pub fn shown(s: &Session) -> Value {
    match s.mailbox.device_limits {
        Some(l) => json!({"on": true, "storage": l.storage, "messages": l.messages, "defaults": l.defaults}),
        None => json!({"on": false, "storage": 0, "messages": 0, "defaults": 0}),
    }
}

fn device_limits(s: &mut Session, v: &Value) -> CmdResult {
    if !v.is_object() && !v.is_null() {
        return Err(bad("the parameters are an object"));
    }
    if p::opt_bool(v, "off")? == Some(true) {
        if s.mailbox.device_limits.take().is_some() {
            // What the folders were listed with was the window's mail alone, and a saved delta link
            // keeps that: every folder, of the shared mailboxes too, is listed afresh, whole.
            let accounts: Vec<Id> = s.mailbox.accounts.iter().map(|a| a.id).collect();
            super::account::relist_accounts(&mut s.mailbox, &accounts);
            s.mailbox_changed();
        }
        return Ok(shown(s));
    }
    let number = |k: &str, least: u64, most: u64| -> Result<Option<u64>, crate::CmdError> {
        match v.get(k) {
            None => Ok(None),
            Some(_) => p::u64(v, k)
                .filter(|n| (least..=most).contains(n))
                .map(Some)
                .ok_or_else(|| crate::CmdError::Params(format!("`{k}` is a whole number from {least} to {most}"))),
        }
    };
    let storage = number("storage", MIN_STORAGE, MAX_STORAGE)?;
    let messages = number("messages", MIN_MESSAGES, MAX_MESSAGES)?;
    let defaults = number("defaults", 0, u64::from(u32::MAX))?.and_then(|d| u32::try_from(d).ok());
    if storage.is_none() && messages.is_none() && defaults.is_none() {
        return Ok(shown(s));
    }
    let before = s.mailbox.device_limits;
    // (A device with no limits yet starts from the phone's: a zero record would be moved to them
    // again at the next open, undoing what is set here.)
    let mut l = before.unwrap_or_else(|| mailcraft_model::DeviceLimits::phone(None));
    if let Some(n) = storage {
        l.storage = n;
    }
    if let Some(n) = messages {
        l.messages = n;
    }
    if let Some(n) = defaults {
        l.defaults = n;
    }
    let l = l.clamped();
    if before != Some(l) {
        s.mailbox.device_limits = Some(l);
        s.mailbox_changed();
    }
    Ok(shown(s))
}

/// What one step of keeping within the limits does to a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Thin,
    Drop,
}

/// The bytes message `m` is counted as on this device.
pub fn local_bytes(m: &Message) -> u64 {
    HEADER_BYTES.saturating_add(m.subject.len() as u64).saturating_add(content_bytes(m))
}

/// The messages of the Microsoft accounts that have a change the server hasn't heard of, or are
/// named by one waiting to go up.
fn waiting_to_go(s: &Session) -> HashSet<Id> {
    let mut ids = HashSet::new();
    let accounts: Vec<Id> = s.mailbox.accounts.iter().filter(|a| a.kind == AccountKind::Microsoft).map(|a| a.id).collect();
    for account in accounts {
        for change in super::graph_up::message_changes(s, account, usize::MAX).0 {
            match change {
                MessageChange::Update { message, .. } | MessageChange::Move { message, .. } | MessageChange::MovedOut { message, .. } => {
                    ids.insert(message);
                }
                MessageChange::Copy { copy, .. } => {
                    ids.insert(copy);
                }
                MessageChange::Delete { .. } => {}
            }
        }
    }
    for q in &s.mailbox.queued {
        if let Queued::CopyMessage { copy, .. } = q {
            ids.insert(*copy);
        }
    }
    ids
}

/// The steps worked out so far, and what the mailbox would be after them.
struct Tally {
    steps: Vec<(Id, Step)>,
    gone: HashSet<Id>,
    count: u64,
    bytes: u64,
}

impl Tally {
    /// `m` leaves this device; `already` of its bytes were counted off by thinning it.
    fn leave(&mut self, m: &Message, already: u64) {
        if self.gone.insert(m.id) {
            self.steps.push((m.id, Step::Drop));
            self.count = self.count.saturating_sub(1);
            self.bytes = self.bytes.saturating_sub(local_bytes(m).saturating_sub(already));
        }
    }
}

/// What keeping within the limits would do, oldest mail first.
fn plan(s: &Session) -> Vec<(Id, Step)> {
    let Some(limits) = s.mailbox.device_limits.map(|l| l.clamped()) else { return Vec::new() };
    let mb = &s.mailbox;
    let copies = OfflineWindow::server_copies(mb);
    let window = OfflineWindow::of(mb, s.now());
    let waiting = waiting_to_go(s);
    // (Only a Microsoft account's mail: the one kind a phone keeps in step.)
    let microsoft: HashSet<Id> = mb.accounts.iter().filter(|a| a.kind == AccountKind::Microsoft).map(|a| a.id).collect();
    let mut may: Vec<&Message> = mb
        .messages
        .values()
        .map(|m| &**m)
        .filter(|m| microsoft.contains(&m.account) && !copies.within(m) && m.returned == 0)
        .filter(|m| !waiting.contains(&m.id) && !s.kept_for_now.contains(&m.id) && !s.sel.messages.contains(&m.id))
        .collect();
    let age = |m: &Message| if m.received != 0 { m.received } else { m.date };
    may.sort_by_key(|m| (age(m), m.id));
    let mut t = Tally {
        steps: Vec::new(),
        gone: HashSet::new(),
        count: mb.messages.len() as u64,
        bytes: mb.messages.values().map(|m| local_bytes(m)).fold(0u64, u64::saturating_add),
    };
    // Older than its account's window: not listed on this device at all.
    for m in may.iter().filter(|m| !window.within(m)) {
        t.leave(m, 0);
    }
    // Past the cap of messages: the oldest leave.
    for m in &may {
        if t.count <= limits.messages {
            break;
        }
        t.leave(m, 0);
    }
    // Past the cap of storage: the oldest go back to their headers, then leave.
    // (What thinning releases is the content less the preview it keeps.)
    let mut thinned: HashMap<Id, u64> = HashMap::new();
    for m in &may {
        if t.bytes <= limits.storage {
            break;
        }
        let content = content_bytes(m);
        if content > 0 && !m.unfetched && !t.gone.contains(&m.id) {
            let released = content.saturating_sub(super::offline::preview_of(s, m.id).len() as u64);
            thinned.insert(m.id, released);
            t.steps.push((m.id, Step::Thin));
            t.bytes = t.bytes.saturating_sub(released);
        }
    }
    for m in &may {
        if t.bytes <= limits.storage {
            break;
        }
        let already = thinned.get(&m.id).copied().unwrap_or(0);
        t.leave(m, already);
    }
    let Tally { mut steps, gone, .. } = t;
    // (A message thinned and then left is only left.)
    steps.retain(|(id, step)| *step == Step::Drop || !gone.contains(id));
    steps
}

fn trim_device(s: &mut Session, v: &Value) -> CmdResult {
    let most = p::u64(v, "most").map_or(MOST, |n| usize::try_from(n).unwrap_or(MOST).clamp(1, MOST));
    let steps = plan(s);
    let left = steps.len().saturating_sub(most);
    let steps: Vec<(Id, Step)> = steps.into_iter().take(most).collect();
    let thin: Vec<Id> = steps.iter().filter(|(_, step)| *step == Step::Thin).map(|(id, _)| *id).collect();
    let drop: HashSet<Id> = steps.iter().filter(|(_, step)| *step == Step::Drop).map(|(id, _)| *id).collect();
    let thinned = if thin.is_empty() { 0 } else { thin_these(s, thin).messages };
    let dropped = leave_device(s, &drop);
    if thinned + dropped > 0 {
        log::info!("device limits: {thinned} message(s) went back to their headers, {dropped} left this device");
    }
    let bytes = s.mailbox.messages.values().map(|m| local_bytes(m)).fold(0u64, u64::saturating_add);
    Ok(json!({"thinned": thinned, "dropped": dropped, "bytes": bytes, "messages": s.mailbox.messages.len(), "left": left}))
}

/// The messages `ids` leave this device: out of the mailbox, their content files off the disk with
/// the next save. Nothing is asked of the server. Not an undo step.
fn leave_device(s: &mut Session, ids: &HashSet<Id>) -> usize {
    if ids.is_empty() {
        return 0;
    }
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut refs: Vec<mailcraft_model::ContentRef> = Vec::new();
    #[cfg(not(target_arch = "wasm32"))]
    if s.path.is_some() {
        for id in ids {
            if let Some(m) = s.mailbox.messages.get(id) {
                refs.extend(crate::store::content_names(*id, m));
            }
        }
    }
    s.let_go(ids);
    let before = s.mailbox.messages.len();
    s.mailbox.messages.retain(|id, _| !ids.contains(id));
    let dropped = before.saturating_sub(s.mailbox.messages.len());
    s.mailbox_changed();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = s.path.clone() {
        crate::store::forget_when_saved(&path, s.rev(), refs);
    }
    #[cfg(target_arch = "wasm32")]
    drop(refs);
    dropped
}
