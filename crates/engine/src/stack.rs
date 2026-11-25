// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A conversation stacked in the reading pane (`view.conversationSettings`, #1778): which of its
//! messages it lists, and which a selection reads, so that the pane and the read marks agree.
//!
//! The work is bounded, whatever the length of a conversation: the pane lists at most [`LISTED`]
//! messages (the selected one and the newest; it says how many older ones it leaves out) and
//! shows at most [`WHOLE`] of them whole besides the selected one; the others are rows that open
//! their message when clicked. A selection reads only what is shown whole of what wasn't read
//! yet: the selected message and the newest [`WHOLE`] others not read yet. A message not read
//! that is left a row stays not read, and bold, until the owner opens it.

use mailcraft_model::Id;

/// Most messages of a conversation the pane lists.
pub const LISTED: usize = 200;
/// Most messages the pane shows whole at once besides the selected one.
pub const WHOLE: usize = 48;

/// The messages of `thread` (oldest first) the pane lists: the newest [`LISTED`], or, when the
/// selected one (`focus`) is older than those, it and the newest but one; oldest first.
pub fn listed(thread: &[Id], focus: Id) -> Vec<Id> {
    let newest = |n: usize| thread.get(thread.len().saturating_sub(n)..).unwrap_or(thread);
    if thread.len() <= LISTED || newest(LISTED).contains(&focus) {
        return newest(LISTED).to_vec();
    }
    let mut ids = vec![focus];
    ids.extend_from_slice(newest(LISTED - 1));
    ids
}

/// What selecting `focus` reads of the messages `listed` when they are stacked: of those that
/// `unread` says aren't read yet, the selected one and the newest [`WHOLE`] others, the ones the
/// pane shows whole for it.
pub fn read_by_selecting(listed: &[Id], focus: Id, unread: impl Fn(Id) -> bool) -> Vec<Id> {
    let mut read: Vec<Id> = listed.iter().rev().copied().filter(|id| *id != focus && unread(*id)).take(WHOLE).collect();
    if unread(focus) {
        read.push(focus);
    }
    read
}
