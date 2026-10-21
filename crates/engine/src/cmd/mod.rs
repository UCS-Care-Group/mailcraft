// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Command implementations, one module per area. Each module exposes `specs()`.

pub mod account;
#[cfg(test)]
mod answered_tests;
pub mod calendar;
pub mod calendar_extra;
pub mod calendar_settings;
pub mod category;
pub mod compose;
pub mod edit;
#[cfg(test)]
mod favorites_tests;
pub mod file;
pub mod folder;
pub mod format;
#[cfg(test)]
mod format_tests;
pub mod graph_down;
pub mod graph_drafts;
pub mod graph_mail;
pub mod graph_place;
pub mod graph_send;
pub mod graph_switch;
pub mod graph_up;
#[cfg(test)]
mod jmap_sending_tests;
pub mod mail;
#[cfg(test)]
mod newer_account_tests;
pub mod offline;
#[cfg(test)]
mod offline_tests;
pub mod people;
#[cfg(test)]
mod quote_tests;
pub mod remote;
pub mod rules;
pub mod search_server;
pub mod shared;
pub mod signature;
#[cfg(test)]
mod signature_tests;
pub mod sync_progress;
pub mod tasks;
pub mod view;
#[cfg(test)]
mod zone_tests;
pub mod zones;

use mailcraft_model::{Id, Message};
use serde_json::{Value, json};

use crate::list::{Row, position};
use crate::{CmdError, CmdResult, Registry, Session, p};

/// The full registry.
pub fn registry() -> Registry {
    let mut v = Vec::new();
    v.extend(file::specs());
    v.extend(edit::specs());
    v.extend(mail::specs());
    v.extend(compose::specs());
    v.extend(format::specs());
    v.extend(folder::specs());
    v.extend(view::specs());
    v.extend(calendar::specs());
    v.extend(calendar_extra::specs());
    v.extend(calendar_settings::specs());
    v.extend(zones::specs());
    v.extend(people::specs());
    v.extend(tasks::specs());
    v.extend(rules::specs());
    v.extend(category::specs());
    v.extend(account::specs());
    v.extend(offline::specs());
    v.extend(signature::specs());
    v.extend(graph_mail::specs());
    v.extend(shared::specs());
    v.extend(graph_switch::specs());
    v.extend(graph_send::specs());
    v.extend(remote::specs());
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

/// Enabled only while a message is being written.
pub(crate) fn needs_compose(s: &Session) -> Option<&'static str> {
    if s.compose.is_none() { Some("no message is being written") } else { None }
}

/// The messages a command acts on: `ids` / `id` when given, else the selection.
pub(crate) fn targets(s: &Session, v: &Value) -> Result<Vec<Id>, CmdError> {
    let ids = match p::ids(v) {
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
    if !s.sel.messages.iter().any(|id| removed.contains(id)) {
        return;
    }
    let first = removed.iter().filter_map(|id| position(before, *id)).min();
    s.sel.messages.retain(|id| !removed.contains(id));
    if !s.sel.messages.is_empty() {
        return;
    }
    let rows = crate::list::rows(s);
    if let Some(row) = first.and_then(|i| rows.get(i).or_else(|| rows.last())) {
        s.sel.messages = vec![row.id];
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
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>, CmdError> {
    use base64::Engine as _;
    let clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(clean).map_err(|_| bad("`data` must be base64"))
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
        let meta = std::fs::metadata(path).map_err(|e| fail(format!("{path}: {e}")))?;
        if meta.len() > crate::io::MAX_FILE {
            return Err(failed!("{path} is larger than {mb} MB", path, mb = crate::io::MAX_FILE >> 20));
        }
        let bytes = std::fs::read(path).map_err(|e| fail(format!("{path}: {e}")))?;
        Ok((name, bytes))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(fail(format!("{path}: the browser can't read files by path; pass `data`")))
    }
}
