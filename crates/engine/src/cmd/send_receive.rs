// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send / Receive › Cancel All and Work Offline.
//!
//! - **Cancel All** (`sync.cancel`): the accounts still waiting their turn in the run under way
//!   are stopped; the host calls off the one at work (the window does, through its own Stop).
//!   What a run brought stays.
//! - **Work Offline** (`sync.offline`): a preference, kept between runs, never an undo step.
//!   While it is on, nothing that goes to a server is asked for: [`Session::run`] refuses every
//!   command in [`ASKS_A_SERVER`], and the window starts no Send/Receive, no fetch, no picture
//!   and no look-up. Mail that is sent waits in the Outbox until it is turned off.

use mailcraft_model::{FolderKind, Id};
use serde_json::{Value, json};

use super::sync_progress::RunState;
use crate::{CmdResult, CommandSpec, Session, p};

/// What a refusal says while Work Offline is on.
pub const OFFLINE: &str = "You're working offline. Turn off Work Offline (Send / Receive) to connect.";

/// The commands that say what a host should ask a server for: refused while working offline.
pub const ASKS_A_SERVER: &[&str] = &[
    "sync.request",
    "sync.graphRequest",
    "sync.graphFetchRequest",
    "sync.graphSearchRequest",
    "sync.jmapRequest",
    "sync.jmapFetchRequest",
    "sync.ewsRequest",
    "sync.ewsFetchRequest",
    "sync.eventChanges",
    "sync.taskChanges",
];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("sync.cancel", "Cancel All", "Send / Receive › Download", cancel).pure().params(
            r#"{} — calls off the Send/Receive under way: the accounts waiting their turn are stopped and the one at work stops at its next safe place; what came in stays. Answers {"cancelling": bool (whether a run was under way), "stopped": [account ids]}"#,
        ),
        CommandSpec::new("sync.offline", "Work Offline", "Send / Receive › Preferences", offline).pure().params(
            r#"{"value"?: bool} (default: the other way) — while on, MailCraft asks no server for anything: no Send/Receive, no message fetched, no pictures, no sender images, no look-ups; mail that is sent waits in the Outbox, and goes with the first Send/Receive after it is turned off. Kept between runs; not an undo step. Answers {"offline": bool, "outbox": number (messages waiting in the Outboxes)}"#,
        ),
    ]
}

fn cancel(s: &mut Session, _: &Value) -> CmdResult {
    let running = s.sync_progress.running();
    let waiting: Vec<Id> = s.sync_progress.accounts.iter().filter(|r| r.state == RunState::Waiting).map(|r| r.account).collect();
    for account in &waiting {
        s.sync_progress.end(*account, true);
    }
    Ok(json!({"cancelling": running, "stopped": waiting}))
}

fn offline(s: &mut Session, v: &Value) -> CmdResult {
    s.prefs.offline = p::bool(v, "value").unwrap_or(!s.prefs.offline);
    Ok(json!({"offline": s.prefs.offline, "outbox": outbox(s)}))
}

/// Messages waiting in the Outboxes of every account.
pub fn outbox(s: &Session) -> usize {
    let mb = &s.mailbox;
    mb.messages.values().filter(|m| mb.folder(m.folder).is_some_and(|f| f.kind == FolderKind::Outbox)).count()
}

#[cfg(test)]
#[path = "send_receive_tests.rs"]
mod send_receive_tests;
