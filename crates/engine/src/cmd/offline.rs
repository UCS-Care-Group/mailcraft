// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail to keep offline (#143): mail of a Microsoft account older than its window goes back to
//! its headers and preview, and the content files it kept on disk go with it.
//!
//! - **What goes back** is mail outside its account's window ([`OfflineWindow`]) that is here
//!   whole: never mail the server lacks (a local folder's, a draft, the Outbox's, a held
//!   message), never one being shown, and not one the owner had fetched this session
//!   ([`Session::kept_for_now`]), which goes back the next time the mailbox is opened.
//! - **What stays** is everything but its content: where it is, read, flag, categories,
//!   importance, the server's state of it and the changes waiting to go up, its headers and a
//!   preview of its text. It is listed, sorted, threaded and searched by those as before.
//! - **Not an undo step:** content that went is fetched again when the message is opened, and
//!   an Undo of something else never brings it back.
//! - **On disk** its content files go once the mailbox is saved, at once rather than with the
//!   sweep hours later ([`crate::store::forget_when_saved`]), unless something else names them.

use std::collections::HashSet;

use mailcraft_model::{AccountKind, Id, KeepOffline, Message, OfflineWindow, size_label};
use serde_json::{Value, json};

use super::{bad, fail, graph_down};
use crate::{CmdResult, CommandSpec, Part, Session, p};

/// Most messages one `account.thinOffline` takes back.
const MOST: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("account.thinOffline", "Remove Older Mail from This Computer", "File › Info", thin_offline)
            .pure()
            .params(r#"{"account"?: number (and the shared mailboxes opened through it; default every account), "most"?: number (default and at most 100000)} — mail of a Microsoft account older than its mail to keep offline goes back to its headers and preview here, and its content files leave the disk once the mailbox is saved; it stays on the server and is fetched again when it is opened. Never mail the server lacks (local folders, Drafts, the Outbox, held messages), the message shown, or one fetched this session by opening it (that goes back the next time the mailbox is opened). Not an undo step. The window runs it at start, once a day, and when its owner shortens the window. Answers {"thinned": number, "bytes": number, "left": number (still to go, past "most")}"#)
            .when(has_graph_account),
        CommandSpec::new("account.offlineSize", "Space Older Mail Takes", "File › Info", offline_size)
            .pure()
            .params(r#"{"account": number, "keepOffline": "3d"|"1w"|"2w"|"1m"|"3m"|"6m"|"12m"|"2y"|"5y"|"10y"|"all"} — how much of the account's mail kept in full here (with the shared mailboxes opened through it) would go back to its headers and preview with that window; changes nothing. Answers {"messages": number, "bytes": number, "size": string ("1.2 GB"), "window": string ("3 months")}"#),
    ]
}

fn has_graph_account(s: &Session) -> Option<&'static str> {
    if s.mailbox.accounts.iter().any(|a| a.kind == AccountKind::Microsoft) {
        None
    } else {
        Some("no account is kept in step through Microsoft Graph")
    }
}

/// The Microsoft account a command names (not a shared mailbox: it follows its owner).
fn account_of(s: &Session, v: &Value, required: bool) -> Result<Option<Id>, crate::CmdError> {
    let id = match v.get("account") {
        None | Some(Value::Null) if !required => return Ok(None),
        _ => p::req_id(v, "account")?,
    };
    let a = s.mailbox.account(id).ok_or_else(|| failed!("no account with id {id}", id))?;
    if a.kind != AccountKind::Microsoft {
        return Err(failed!("{name} isn't kept in step through Microsoft Graph", name = a.name));
    }
    if a.is_shared() {
        return Err(fail("a shared mailbox keeps as much mail offline as the account it is opened through: name that account"));
    }
    Ok(Some(id))
}

fn thin_offline(s: &mut Session, v: &Value) -> CmdResult {
    let account = account_of(s, v, false)?;
    let most = p::u64(v, "most").map_or(MOST, |n| usize::try_from(n).unwrap_or(MOST).clamp(1, MOST));
    let done = thin(s, account, most);
    Ok(json!({"thinned": done.messages, "bytes": done.bytes, "left": done.left}))
}

/// What `account.offlineSize` answers: what a window would take back.
pub fn offline_size(s: &mut Session, v: &Value) -> CmdResult {
    let account = account_of(s, v, true)?.ok_or_else(|| bad("`account` is required"))?;
    let keep =
        KeepOffline::parse(p::req_str(v, "keepOffline")?).ok_or_else(|| bad("`keepOffline` is 3d, 1w, 2w, 1m, 3m, 6m, 12m, 2y, 5y, 10y or all"))?;
    let (messages, bytes) = would_free(s, account, keep);
    Ok(json!({"messages": messages, "bytes": bytes, "size": size_label(bytes), "window": keep.label()}))
}

/// How many messages of `account` (with the shared mailboxes opened through it) kept in full
/// here would go back to their headers and preview with the window `keep`, and the bytes of
/// content they have. Changes nothing.
pub fn would_free(s: &Session, account: Id, keep: KeepOffline) -> (usize, u64) {
    let window = OfflineWindow::proposed(&s.mailbox, s.now(), account, keep);
    let going = outside_of(s, Some(account), &window);
    let bytes = going.iter().filter_map(|id| s.mailbox.message(*id)).map(content_bytes).fold(0u64, u64::saturating_add);
    (going.len(), bytes)
}

/// How long a preview is: as long as Microsoft Graph's own (`bodyPreview`).
const PREVIEW: usize = 255;

/// What thinning did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Thinned {
    /// Messages that went back to their headers and preview, and the bytes of content they let go of.
    pub messages: usize,
    pub bytes: u64,
    /// Messages outside the window still whole (past `most`): for the next call.
    pub left: usize,
}

/// The messages of `account` (and of the shared mailboxes it opens; every account for `None`)
/// that are whole and outside their window at the session's clock, and may go back.
fn outside(s: &Session, account: Option<Id>) -> Vec<Id> {
    outside_of(s, account, &OfflineWindow::of(&s.mailbox, s.now()))
}

/// [`outside`], of `window`.
fn outside_of(s: &Session, account: Option<Id>, window: &OfflineWindow) -> Vec<Id> {
    let mb = &s.mailbox;
    if window.keeps_all() {
        return Vec::new();
    }
    let accounts: Option<HashSet<Id>> =
        account.map(|a| mb.accounts.iter().filter(|x| x.id == a || mb.owner_of(x.id).is_some_and(|o| o.id == a)).map(|x| x.id).collect());
    mb.messages
        .values()
        .filter(|m| !m.unfetched && accounts.as_ref().is_none_or(|set| set.contains(&m.account)))
        .filter(|m| !window.within(m) && !s.kept_for_now.contains(&m.id) && !s.sel.messages.contains(&m.id))
        .map(|m| m.id)
        .collect()
}

/// The bytes of content message `m` has, in memory and kept on disk.
pub(crate) fn content_bytes(m: &Message) -> u64 {
    let body = m.body.text.len() as u64 + m.body.html.as_ref().map_or(0, |h| h.len() as u64);
    let kept = m.kept.as_ref().map_or(0, |k| k.len());
    let files: u64 = m.attachments.iter().map(|a| a.size() as u64).sum();
    body.saturating_add(kept).saturating_add(files)
}

/// The preview message `id` keeps: the first words of its text (read from disk when only that
/// is kept there).
fn preview_of(s: &Session, id: Id) -> String {
    let Some(m) = s.mailbox.message(id) else { return String::new() };
    if !m.body.text.trim().is_empty() || m.body.html.is_some() {
        return crate::list::preview(m, PREVIEW);
    }
    let kept = m.kept.as_deref().filter(|k| !k.failed);
    if kept.is_some_and(|k| k.text.is_some())
        && let Ok(bytes) = s.content(id, Part::Text)
    {
        return mailcraft_html::text_preview(&String::from_utf8_lossy(&bytes), PREVIEW);
    }
    if kept.is_some_and(|k| k.html.is_some())
        && let Ok(bytes) = s.content(id, Part::Html)
    {
        return mailcraft_html::preview(&String::from_utf8_lossy(&bytes), PREVIEW);
    }
    String::new()
}

/// Mail of `account` (every account for `None`) outside its window goes back to its headers and
/// preview, `most` messages at most. Not an undo step.
pub fn thin(s: &mut Session, account: Option<Id>, most: usize) -> Thinned {
    let waiting = outside(s, account);
    let mut done = Thinned { left: waiting.len().saturating_sub(most), ..Default::default() };
    let mut thinned = HashSet::new();
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut refs: Vec<mailcraft_model::ContentRef> = Vec::new();
    for id in waiting.into_iter().take(most) {
        let preview = preview_of(s, id);
        let Some(m) = s.mailbox.messages.get(&id) else { continue };
        done.bytes = done.bytes.saturating_add(content_bytes(m));
        #[cfg(not(target_arch = "wasm32"))]
        if s.path.is_some() {
            refs.extend(crate::store::content_names(id, m));
        }
        if let Ok(m) = s.mailbox.message_mut(id) {
            graph_down::thin_message(m, preview);
            thinned.insert(id);
        }
    }
    done.messages = thinned.len();
    if done.messages == 0 {
        return done;
    }
    s.let_go(&thinned);
    s.mailbox_changed();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = s.path.clone() {
        crate::store::forget_when_saved(&path, s.rev(), refs);
    }
    #[cfg(target_arch = "wasm32")]
    drop(refs);
    log::info!("{} message(s) outside their mail to keep offline went back to their headers and preview ({} bytes)", done.messages, done.bytes);
    done
}
