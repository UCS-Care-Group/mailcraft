// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The look-up of the conversation ids of messages listed before they were asked for
//! (#795): a Microsoft account's, an Exchange server account's or a JMAP account's mail kept by an
//! earlier version has its headers and a preview, and nothing that says which thread it is of. Until
//! it has, such messages are one conversation per topic among themselves, as every conversation
//! was before ([`mailcraft_model::thread_index`]).
//!
//! Each Send/Receive asks the account's own server, after the rest of its run, for the conversation
//! ids of at most [`MAX_THREAD_ASKS`] of them, newest first (`threads` of the run's request); the
//! answer comes back with the run (`threads` of its result) and is kept with each message. A message
//! the server gave none for (or no longer has) is marked asked and never asked again; one it refused
//! to say for is asked again in at most [`MAX_THREAD_TRIES`] runs in all; one it didn't answer for
//! (busy, the connection gone) is asked in a later run. So the look-up goes on from where it stopped, a thousand
//! messages a run, and once every message has an id or was asked, nothing more is asked. Nothing is
//! sent anywhere but to the account's own server, and nothing but the messages' server ids.

use std::collections::HashMap;

use mailcraft_model::sync::{MAX_THREAD_ASKS, MAX_THREAD_TRIES, ThreadOf, thread_id};
use mailcraft_model::{Id, Mailbox};

/// The server ids of the messages of `account` to ask the conversation of: those its server has
/// and has given no conversation id for, not asked yet, newest first, at most [`MAX_THREAD_ASKS`].
pub(crate) fn to_ask(mb: &Mailbox, account: Id) -> Vec<String> {
    let mut waiting: Vec<(i64, Id, &str)> = mb
        .messages
        .values()
        .filter(|m| m.account == account)
        .filter_map(|m| {
            let server = m.server.as_ref()?;
            (server.thread.is_empty() && !server.thread_asked && !server.gone && !server.id.is_empty()).then_some((
                m.received,
                m.id,
                server.id.as_str(),
            ))
        })
        .collect();
    // The newest first: a partial sort is enough, and linear.
    if waiting.len() > MAX_THREAD_ASKS {
        waiting.select_nth_unstable_by(MAX_THREAD_ASKS, |a, b| (b.0, b.1).cmp(&(a.0, a.1)));
        waiting.truncate(MAX_THREAD_ASKS);
    }
    waiting.sort_unstable_by_key(|&(when, id, _)| std::cmp::Reverse((when, id)));
    waiting.into_iter().map(|(_, _, id)| id.to_string()).collect()
}

/// What a host brought back held to its sizes: at most [`MAX_THREAD_ASKS`], each with an id
/// `is_id` takes and a conversation id as [`thread_id`] keeps it (anything else is none).
pub(crate) fn bounded(said: &mut Vec<ThreadOf>, is_id: fn(&str) -> bool) {
    said.truncate(MAX_THREAD_ASKS);
    said.retain(|t| is_id(&t.id));
    for t in said.iter_mut() {
        t.thread = thread_id(&t.thread);
    }
}

/// Take in what the server said of the conversations of the messages of `account` it was asked
/// about: each keeps its conversation id, or is marked asked when the server gave none. Whether
/// any message changed.
pub(crate) fn take(mb: &mut Mailbox, account: Id, said: &[ThreadOf]) -> bool {
    if said.is_empty() {
        return false;
    }
    let by_id: HashMap<&str, &ThreadOf> = said.iter().take(MAX_THREAD_ASKS).map(|t| (t.id.as_str(), t)).collect();
    let mine: Vec<(Id, String, bool)> = mb
        .messages
        .values()
        .filter(|m| m.account == account)
        .filter_map(|m| {
            let server = m.server.as_ref().filter(|s| s.thread.is_empty())?;
            by_id.get(server.id.as_str()).map(|t| (m.id, thread_id(&t.thread), t.refused))
        })
        .collect();
    let mut changed = false;
    for (id, thread, refused) in mine {
        if let Ok(m) = mb.message_mut(id)
            && let Some(server) = m.server.as_mut()
        {
            if thread.is_empty() && refused {
                // Refused: asked again in a later run, but not for ever.
                server.thread_tries = server.thread_tries.saturating_add(1);
                server.thread_asked = server.thread_tries >= MAX_THREAD_TRIES;
            } else if thread.is_empty() {
                server.thread_asked = true;
            } else {
                server.thread = thread;
            }
            changed = true;
        }
    }
    changed
}
