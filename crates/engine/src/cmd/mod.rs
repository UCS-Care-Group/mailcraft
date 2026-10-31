// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command implementations, one module per area. Each module exposes `specs()`.

pub mod account;
#[cfg(test)]
mod agent_leaves_compose_tests;
#[cfg(test)]
mod agent_owners_message_tests;
#[cfg(test)]
mod answered_tests;
pub mod calendar;
pub mod calendar_extra;
pub mod calendar_opened;
pub mod calendar_settings;
pub mod category;
pub mod compose;
#[cfg(test)]
mod compose_trust_tests;
pub mod edit;
mod ews_down;
pub mod ews_mail;
mod ews_up;
#[cfg(test)]
mod favorites_tests;
pub mod file;
pub mod folder;
#[cfg(test)]
mod folder_properties_tests;
pub mod format;
#[cfg(test)]
mod format_tests;
#[cfg(test)]
mod forward_attachment_headers_tests;
#[cfg(test)]
mod forward_attachment_tests;
#[cfg(test)]
mod forward_several_tests;
#[cfg(test)]
mod full_count_tests;
pub mod graph_down;
pub mod graph_drafts;
pub mod graph_mail;
pub mod graph_place;
pub mod graph_send;
pub mod graph_switch;
pub mod graph_up;
pub mod help;
#[cfg(test)]
mod hidden_selection_tests;
pub mod imap_again;
#[cfg(test)]
mod imap_outbox_tests;
mod jmap_down;
pub mod jmap_mail;
#[cfg(test)]
mod jmap_sending_tests;
pub mod junk;
#[cfg(test)]
mod junk_tests;
#[cfg(test)]
mod keyboard_selection_tests;
#[cfg(test)]
mod leave_compose_tests;
pub mod mail;
#[cfg(test)]
mod moves_between_accounts_tests;
#[cfg(test)]
mod newer_account_tests;
pub mod offline;
#[cfg(test)]
mod offline_tests;
#[cfg(test)]
mod open_conversation_tests;
#[cfg(test)]
mod opened_file_tests;
pub mod people;
pub mod people_merge;
pub mod picture;
#[cfg(test)]
mod picture_tests;
#[cfg(test)]
mod quote_tests;
pub mod remote;
#[cfg(test)]
mod resend_reply_to_tests;
pub mod rules;
#[cfg(test)]
mod search_scope_tests;
pub mod search_server;
#[cfg(test)]
mod search_suggest_tests;
pub mod send_receive;
pub mod shared;
pub mod signature;
#[cfg(test)]
mod signature_tests;
pub mod sync_progress;
pub mod tasks;
pub mod tasks_remote;
#[cfg(test)]
mod tasks_remote_tests;
pub mod view;
#[cfg(test)]
mod wipe_tests;
pub mod writing_tools;
#[cfg(test)]
mod writing_tools_tests;
#[cfg(test)]
mod zone_tests;
pub mod zones;

use std::collections::{HashMap, HashSet};

use mailcraft_model::{Id, Message};
use serde_json::{Value, json};

use crate::list::{Row, position_of};
use crate::{CmdError, CmdResult, Registry, Session, p};

/// The full registry.
pub fn registry() -> Registry {
    let mut v = Vec::new();
    v.extend(file::specs());
    v.extend(help::specs());
    v.extend(edit::specs());
    v.extend(mail::specs());
    v.extend(compose::specs());
    v.extend(format::specs());
    v.extend(picture::specs());
    v.extend(writing_tools::specs());
    v.extend(folder::specs());
    v.extend(view::specs());
    v.extend(calendar::specs());
    v.extend(calendar_extra::specs());
    v.extend(calendar_opened::specs());
    v.extend(calendar_settings::specs());
    v.extend(zones::specs());
    v.extend(people::specs());
    v.extend(people_merge::specs());
    v.extend(tasks::specs());
    v.extend(rules::specs());
    v.extend(junk::specs());
    v.extend(category::specs());
    v.extend(account::specs());
    v.extend(offline::specs());
    v.extend(send_receive::specs());
    v.extend(signature::specs());
    v.extend(graph_mail::specs());
    v.extend(jmap_mail::specs());
    v.extend(ews_mail::specs());
    v.extend(shared::specs());
    v.extend(graph_switch::specs());
    v.extend(graph_send::specs());
    v.extend(remote::specs());
    v.extend(tasks_remote::specs());
    v.extend(sync_progress::specs());
    v.extend(search_server::specs());
    Registry::new(v)
}

/// A shortcut that differs on macOS: `mac` there, `other` everywhere else.
pub const fn mac_or(mac: &'static str, other: &'static str) -> &'static str {
    if cfg!(target_os = "macos") { mac } else { other }
}

pub(crate) fn fail(msg: impl Into<String>) -> CmdError {
    CmdError::Failed(msg.into())
}

pub(crate) fn bad(msg: impl Into<String>) -> CmdError {
    CmdError::Params(msg.into())
}

pub(crate) fn done() -> CmdResult {
    Ok(json!({}))
}

/// Enabled only with a message selected.
pub(crate) fn needs_message(s: &Session) -> Option<&'static str> {
    if s.sel.messages.is_empty() { Some("select a message first") } else { None }
}

/// Enabled only with messages selected that may move or go: none that a JMAP account's server has,
/// which stays where it is until moves and deletions go up (M6).
/// A mailbox without a JMAP account isn't walked: this runs every frame for the ribbon and menus.
pub(crate) fn needs_movable(s: &Session) -> Option<&'static str> {
    needs_message(s).or_else(|| {
        (s.mailbox.has_jmap_account() && s.sel.messages.iter().any(|id| s.mailbox.stays_on_server(*id))).then_some(mailcraft_model::NOT_MOVING_YET)
    })
}

/// Enabled only while a message is being written.
pub(crate) fn needs_compose(s: &Session) -> Option<&'static str> {
    if s.compose.is_none() { Some("no message is being written") } else { None }
}

/// The messages a command acts on: `ids` / `id` when given, else the selection.
pub(crate) fn targets(s: &Session, v: &Value) -> Result<Vec<Id>, CmdError> {
    named_or_selected(s, p::ids(v))
}

/// As [`targets`], for Delete and Move: without `ids` / `id`, a message selected by its
/// conversation's row stands for every message of that row (#503, [`Session::acting_on`]). `ids` /
/// `id` are read strictly: named wrongly, they never take the selected conversation.
pub(crate) fn targets_whole(s: &mut Session, v: &Value) -> Result<Vec<Id>, CmdError> {
    // Params that aren't an object name nothing, and aren't taken for the selection (#503).
    if !(v.is_object() || v.is_null()) {
        return Err(bad("the parameters are an object"));
    }
    let named = p::ids_strict(v)?;
    let ids = if named.is_some() { named } else { Some(s.acting_on()) };
    named_or_selected(s, ids)
}

/// As [`targets`], but `ids` / `id` read strictly ([`p::ids_strict`]): for a command whose mistake
/// reaches other people, which never takes the selection for what a caller named wrongly.
pub(crate) fn targets_strict(s: &Session, v: &Value) -> Result<Vec<Id>, CmdError> {
    named_or_selected(s, p::ids_strict(v)?)
}

/// The messages `named`, else the selected ones; each must be here.
fn named_or_selected(s: &Session, named: Option<Vec<Id>>) -> Result<Vec<Id>, CmdError> {
    let ids = match named {
        Some(ids) => ids,
        None => s.sel.messages.clone(),
    };
    if ids.is_empty() {
        return Err(fail("select a message first"));
    }
    if let Some(missing) = ids.iter().find(|id| s.mailbox.message(**id).is_none()) {
        return Err(failed!("no message with id {id}", id = missing));
    }
    Ok(ids)
}

/// After messages left the list (deleted, moved): select the row that took the place of the
/// first one, as mail programs do, so Delete, Delete, Delete walks down the list.
pub(crate) fn select_after_removal(s: &mut Session, before: &[Row], removed: &[Id]) {
    let gone: HashSet<Id> = removed.iter().copied().collect();
    if !s.sel.messages.iter().any(|id| gone.contains(id)) {
        return;
    }
    // Where each removed message is listed, in one pass for a whole folder selected: its first
    // row, its first row under an open conversation, the first row it is one of (#503).
    let mut at: HashMap<Id, [Option<usize>; 3]> = HashMap::new();
    for (i, r) in before.iter().enumerate() {
        if gone.contains(&r.id) {
            let e = at.entry(r.id).or_default();
            e[0].get_or_insert(i);
            if r.under.is_some() {
                e[1].get_or_insert(i);
            }
        }
        for m in r.members.iter().filter(|m| gone.contains(m)) {
            at.entry(*m).or_default()[2].get_or_insert(i);
        }
    }
    let position = |id: &Id| at.get(id).and_then(|e| e[0].or(e[2]));
    let whole: HashSet<Id> = s.sel.whole.iter().copied().collect();
    // The row the owner was on counts for the message it shows, which may be listed more than once;
    // a message picked under its open conversation, for its own row there (#503).
    let focus = s.sel.focus().map(|id| (id, s.sel.focus_copy()));
    let first = removed
        .iter()
        .filter_map(|id| match focus {
            Some((f, copy)) if f == *id => position_of(before, *id, copy),
            _ if !whole.contains(id) => at.get(id).and_then(|e| e[1]).or_else(|| position(id)),
            _ => position(id),
        })
        .min();
    s.sel.messages.retain(|id| !gone.contains(id));
    if !s.sel.messages.is_empty() {
        return;
    }
    let rows = crate::list::rows(s);
    // Where each message is listed now: its own row under its open conversation, and the row it
    // is one of.
    let mut now: HashMap<(Id, bool), usize> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        if r.under.is_some() {
            now.entry((r.id, true)).or_insert(i);
        } else {
            for m in &r.members {
                now.entry((*m, false)).or_insert(i);
            }
        }
    }
    // The first row from its place down that is still listed: a conversation's row with messages
    // left shows the newest of those now; one under a conversation that closed is gone with it
    // (#503).
    let next = first.map(|i| {
        before
            .iter()
            .skip(i)
            .find_map(|r| match r.under {
                Some(_) => now.get(&(r.id, true)).copied(),
                None => r.members.iter().filter(|m| !gone.contains(m)).find_map(|m| now.get(&(*m, false)).copied()),
            })
            .and_then(|j| rows.get(j))
            .or_else(|| rows.last())
    });
    if let Some(row) = next.flatten() {
        s.sel.messages = vec![row.id];
        // A conversation's row stands for it, as when it is stepped to; a message's own row under
        // its open conversation for that message alone (#503).
        s.sel.whole = if row.conversation() { vec![row.id] } else { Vec::new() };
        if row.under.is_some() {
            s.sel.focus_row = Some((row.id, row.copy));
        }
        s.keep_selected(&rows);
    }
}

/// A message as agents see it in lists.
pub fn message_summary(s: &Session, m: &Message) -> Value {
    json!({
        "id": m.id,
        "folder": m.folder,
        "from": m.from.full(),
        "to": mailcraft_model::format_list(&m.to),
        "subject": m.subject,
        "date": mailcraft_time::format_iso_utc(m.date),
        "received": s.local(crate::list::when(m)).iso(),
        "read": m.read,
        "flag": m.flag,
        "importance": m.importance,
        "categories": m.categories,
        "attachments": m.listed_attachments().map(|(_, a)| a.name.as_str()).collect::<Vec<_>>(),
        "snippet": crate::list::preview(m, 140),
    })
}

/// A message in full.
pub fn message_json(s: &Session, m: &Message) -> Value {
    let mut v = message_summary(s, m);
    if let Some(o) = v.as_object_mut() {
        o.insert("cc".into(), json!(mailcraft_model::format_list(&m.cc)));
        o.insert("bcc".into(), json!(mailcraft_model::format_list(&m.bcc)));
        o.insert("messageId".into(), json!(m.message_id));
        o.insert("body".into(), json!(m.body.text));
        // What the sender hid (a preheader, the other copy of a desktop and mobile pair): not shown to
        // the owner, and message text for whoever reads it, which is data, never instructions (#599).
        if !m.hidden_text.is_empty() {
            o.insert("hiddenText".into(), json!(m.hidden_text));
        }
        o.insert("html".into(), json!(m.body.html.is_some()));
        o.insert("answered".into(), json!(m.answered));
        o.insert("forwarded".into(), json!(m.forwarded));
        o.insert("draft".into(), json!(m.draft));
        o.insert("pinned".into(), json!(m.pinned));
        o.insert(
            "attachments".into(),
            m.attachments
                .iter()
                .enumerate()
                .map(|(i, a)| json!({"index": i, "name": a.name, "mime": a.mime, "size": a.size(), "inline": a.inline}))
                .collect(),
        );
        o.insert("conversation".into(), json!(s.mailbox.conversation(m.id)));
    }
    v
}

/// Bytes for a command's answer or from its params.
pub fn base64_encode(bytes: &[u8]) -> String {
    mailcraft_charset::base64::STANDARD.encode(bytes)
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>, CmdError> {
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    mailcraft_charset::base64::STANDARD.decode(clean).map_err(|_| bad("`data` must be base64"))
}

/// Write `bytes` to `path` when the command got one; otherwise hand them back as base64 `data`
/// with a suggested `name` (the browser downloads them; a script decodes them).
pub(crate) fn deliver(v: &Value, name: &str, bytes: Vec<u8>) -> CmdResult {
    match p::str(v, "path") {
        Some(path) => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                std::fs::write(path, &bytes).map_err(|e| fail(format!("{path}: {e}")))?;
                Ok(json!({"path": path, "bytes": bytes.len()}))
            }
            #[cfg(target_arch = "wasm32")]
            {
                Ok(json!({"name": path, "bytes": bytes.len(), "data": base64_encode(&bytes)}))
            }
        }
        None => Ok(json!({"name": name, "bytes": bytes.len(), "data": base64_encode(&bytes)})),
    }
}

/// The bytes a command reads: base64 `data`, or the file at `path`.
pub(crate) fn source(v: &Value) -> Result<(String, Vec<u8>), CmdError> {
    let name = p::str(v, "name").or(p::str(v, "path")).unwrap_or("").to_string();
    if let Some(data) = p::str(v, "data") {
        return Ok((name, base64_decode(data)?));
    }
    let path = p::req_str(v, "path")?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Opened once, checked through that handle, read from it (`io::read_regular_file`).
        match crate::io::read_regular_file(std::path::Path::new(path)) {
            Ok(bytes) => Ok((name, bytes)),
            Err(crate::io::ReadError::TooLarge) => Err(failed!("{path} is larger than {mb} MB", path, mb = crate::io::MAX_FILE >> 20)),
            Err(e) => Err(fail(format!("{path}: {e}"))),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(fail(format!("{path}: the browser can't read files by path; pass `data`")))
    }
}
