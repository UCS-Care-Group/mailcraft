// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What became of what went up from an Exchange server account (#252, E6), held to sizes before it
//! is taken in: a host may hand over anything, so an answer whose ids can't be ids, or whose texts
//! are longer than any server's, is dropped rather than taken in. A change that has no answer here
//! is worked out again for the next run, as one that never got one is.

use mailcraft_model::sync::{ChangeOutcome, DraftDone, EwsMailResult, GraphFolder, MessageChange, MessageChangeDone, MessageOutcome, SendDone};

use super::ews_down::is_id;

/// Longest change key kept.
const MAX_KEY: usize = 1024;

fn id_or_empty(id: &str) -> bool {
    id.is_empty() || is_id(id)
}

fn folder_ok(f: &GraphFolder) -> bool {
    id_or_empty(&f.id) && id_or_empty(&f.parent) && f.name.len() <= 4096
}

fn message_ok(done: &MessageChangeDone) -> bool {
    let change = match &done.change {
        MessageChange::Update { id, .. } | MessageChange::Delete { id } | MessageChange::MovedOut { id, .. } => is_id(id),
        MessageChange::Move { id, to, .. } | MessageChange::Copy { id, to, .. } => is_id(id) && id_or_empty(to),
    };
    let outcome = match &done.outcome {
        MessageOutcome::Done { folder, id, categories, .. } => {
            id_or_empty(folder) && id_or_empty(id) && categories.as_ref().is_none_or(|c| c.len() <= 200 && c.iter().all(|c| c.len() <= 4096))
        }
        _ => true,
    };
    change && outcome && done.there.is_none()
}

fn draft_ok(done: &DraftDone) -> bool {
    id_or_empty(&done.id)
        && id_or_empty(&done.folder)
        && done.change_key.len() <= MAX_KEY
        && !done.change.marker.chars().any(char::is_control)
        && done.attachments.as_ref().is_none_or(|a| a.len() <= 500 && a.iter().all(|f| id_or_empty(&f.id)))
}

fn sent_ok(done: &SendDone) -> bool {
    done.draft.as_ref().is_none_or(draft_ok) && id_or_empty(&done.draft_left)
}

/// `result`'s answers to what went up, held to sizes: what can't be taken in is left out. Mail
/// placed from another mailbox is no part of an Exchange server account's, and is always left out.
pub(crate) fn bounded_up(result: &mut EwsMailResult) {
    result.placed.clear();
    result.changes.retain(|done| match &done.outcome {
        ChangeOutcome::Done { folder } => folder_ok(folder),
        ChangeOutcome::Refused { why } => why.len() <= 4096,
        ChangeOutcome::Gone => true,
    });
    result.messages.retain(message_ok);
    result.drafts.retain(draft_ok);
    result.sent.retain(sent_ok);
}
