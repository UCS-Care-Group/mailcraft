// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Moving mail between Gmail's categories (#911): `mail.setGmailCategory`.
//!
//! The move is made here at once, like a read mark, and is an undo step. It goes to Gmail at the
//! next Send/Receive, offline too: a message whose category here isn't the one Gmail last said
//! ([`Message::gmail_move_waiting`]) is put in the account's request (`SyncRequest::gmail_moves`),
//! and Gmail's answer that it took the move makes the two agree. What was changed here wins until
//! Gmail has it; a change made only in Gmail is taken in at the next run. Only for a Gmail account
//! that signs in with Google, and only while moves are switched on ([`crate::gmail::moves_offered`]).

use std::collections::{BTreeSet, HashSet};

use mailcraft_model::{GmailCategory, Id, Message};
use serde_json::{Value, json};

use super::{bad, fail, select_after_removal, targets_whole};
use crate::gmail::{moves_offered, why_not_movable};
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.setGmailCategory", "Move to Category", "Home › Move", set_category)
            .params(
                r#"{"category": "primary"|"social"|"promotions"|"updates"|"forums", "ids"?: [number]} (messages of a Gmail account's Inbox, of an account that signs in with Google; named ids move those messages and every copy here of the same Gmail message (Gmail keeps one category for a message), never other messages of their conversations; without ids, the selection, and in conversation view every message of its conversations in that Inbox)"#,
            )
            .when(moves_offered),
    ]
}

fn set_category(s: &mut Session, v: &Value) -> CmdResult {
    // (The registry checks it too; a caller that names messages is held to it all the same.)
    if let Some(why) = moves_offered(s) {
        return Err(fail(why));
    }
    let code = p::req_str(v, "category")?;
    let category = GmailCategory::parse(code).ok_or_else(|| bad("`category` is primary, social, promotions, updates or forums"))?;
    let named = p::ids_strict(v)?.is_some();
    let mut ids = targets_whole(s, v)?;
    // What is named moves, all of it or none; of a selection (a conversation's row may hold mail
    // of other folders), what can move does.
    let mut refused = None;
    ids.retain(|id| match s.mailbox.message(*id).map(|m| why_not_movable(s, m)) {
        Some(None) => true,
        Some(Some(why)) => {
            refused.get_or_insert(why);
            false
        }
        None => {
            refused.get_or_insert("no such message");
            false
        }
    });
    if let Some(why) = refused.filter(|_| named || ids.is_empty()) {
        return Err(fail(why));
    }
    // Named messages move without the rest of their conversations, as `mail.move` moves them; a
    // selection in conversation view takes its conversations along. Either way every copy here
    // of the same Gmail message goes too: Gmail keeps one category for a message.
    let all = if named { ids } else { with_conversations(s, &ids) };
    let all = with_copies(s, all);
    let before = s.rows();
    let mut moved = Vec::new();
    for id in &all {
        let m = s.mailbox.message_mut(*id)?;
        // What Gmail said before the first move here is what the move is measured against.
        if m.gmail_synced.is_none() {
            m.gmail_synced = m.gmail_category;
        }
        // A new move of the owner's: Gmail's refusals of the one before don't count against it.
        m.gmail_refusals = 0;
        if m.gmail_category != Some(category) {
            m.gmail_category = Some(category);
            moved.push(*id);
        }
    }
    // Out of the category in view: the next one is selected, as for a move to a folder.
    if crate::gmail::in_view(s).is_some_and(|shown| shown != category) {
        select_after_removal(s, &before, &moved);
    }
    Ok(json!({"moved": moved.len(), "category": category.code()}))
}

/// `ids`, and every other copy here of the same Gmail messages (one `X-GM-MSGID` in the same
/// Inbox): Gmail has the message in one category, so its copies move together and the latest
/// move is the one Gmail is told.
fn with_copies(s: &Session, mut ids: Vec<Id>) -> Vec<Id> {
    let mb = &s.mailbox;
    let of: HashSet<(Id, Id, u64)> =
        ids.iter().filter_map(|id| mb.message(*id)).filter(|m| m.gmail_id != 0).map(|m| (m.account, m.folder, m.gmail_id)).collect();
    if of.is_empty() {
        return ids;
    }
    let mut seen: HashSet<Id> = ids.iter().copied().collect();
    for m in mb.messages.values() {
        if m.gmail_id != 0 && of.contains(&(m.account, m.folder, m.gmail_id)) && seen.insert(m.id) && why_not_movable(s, m).is_none() {
            ids.push(m.id);
        }
    }
    ids
}

/// `ids`, and in conversation view every other message of their conversations that lies in the
/// same Inbox and can be moved too (spec: with conversations, the whole conversation in that
/// Inbox). In the order given, then the rest in the mailbox's order.
fn with_conversations(s: &Session, ids: &[Id]) -> Vec<Id> {
    let mut seen: HashSet<Id> = HashSet::new();
    let mut out: Vec<Id> = ids.iter().copied().filter(|id| seen.insert(*id)).collect();
    if !s.view.conversations {
        return out;
    }
    let index = s.threads();
    let mb = &s.mailbox;
    let named: Vec<&Message> = ids.iter().filter_map(|id| mb.message(*id)).collect();
    let folders: BTreeSet<Id> = named.iter().map(|m| m.folder).collect();
    let keys: HashSet<_> = named.iter().map(|m| (m.account, index.key(m.id))).collect();
    for m in mb.messages.values() {
        if folders.contains(&m.folder) && !seen.contains(&m.id) && keys.contains(&(m.account, index.key(m.id))) && why_not_movable(s, m).is_none() {
            seen.insert(m.id);
            out.push(m.id);
        }
    }
    out
}
