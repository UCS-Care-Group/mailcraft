// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail: selecting, reading, filing, flagging and deleting messages.

use mailcraft_model::{FlagStatus, FolderKind, Id, Rgb};
use serde_json::{Value, json};

use super::{bad, deliver, done, fail, message_json, message_summary, needs_message, needs_movable, select_after_removal, targets};
use crate::list::{Row, listed, position, position_of};
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.select", "Select Message", "Mail", select)
            .pure()
            .params(r#"{"id": number} | {"ids": [number]}, "extend"?: bool (add to the selection), "range"?: bool (select up to it), "copy"?: number (which of its rows, arranged by categories), "elsewhere"?: bool (opened from outside the list, as from a task: shown in its folder, in the All tab, with no search)"#),
        CommandSpec::new("mail.selectAll", "Select All", "Folder › Actions", select_all).pure().key("Mod+A"),
        CommandSpec::new("mail.next", "Next Message", "Mail", next).pure().key("ArrowDown / Shift+ArrowDown").params(EXTEND),
        CommandSpec::new("mail.previous", "Previous Message", "Mail", previous).pure().key("ArrowUp / Shift+ArrowUp").params(EXTEND),
        CommandSpec::new("mail.first", "First Message", "Mail", first).pure().key("Home / Shift+Home").params(EXTEND),
        CommandSpec::new("mail.last", "Last Message", "Mail", last).pure().key("End / Shift+End").params(EXTEND),
        CommandSpec::new("mail.pageDown", "Page Down", "Mail", page_down).pure().key("PageDown / Shift+PageDown").params(PAGE),
        CommandSpec::new("mail.pageUp", "Page Up", "Mail", page_up).pure().key("PageUp / Shift+PageUp").params(PAGE),
        CommandSpec::new("mail.get", "Get Message", "Mail", get).pure().params(r#"{"id"?: number} (default: the selected message)"#),
        CommandSpec::new("mail.list", "List Messages", "Mail", list).pure().params(r#"{"limit"?: number} — the message list in view"#),
        CommandSpec::new("mail.delete", "Delete", "Home › Delete", delete).key("Delete / Mod+D").params(IDS).selection(needs_movable),
        CommandSpec::new("mail.deletePermanently", "Permanently Delete", "Home › Delete", delete_permanently)
            .key("Shift+Delete")
            .params(IDS)
            .selection(needs_movable),
        CommandSpec::new("mail.archive", "Archive", "Home › Delete", archive).key("Backspace").params(IDS).selection(needs_movable),
        CommandSpec::new("mail.move", "Move to Folder", "Home › Move", move_to)
            .key("Mod+Shift+V")
            .params(r#"{"folder": number, "ids"?: [number]}"#)
            .selection(needs_movable),
        CommandSpec::new("mail.copy", "Copy to Folder", "Home › Move", copy_to)
            .key("Mod+Shift+Y")
            .params(r#"{"folder": number, "ids"?: [number]}"#)
            .selection(needs_message),
        CommandSpec::new("mail.markRead", "Mark as Read", "Home › Tags", mark_read).key("Mod+Q").params(IDS).selection(needs_message),
        CommandSpec::new("mail.markUnread", "Mark as Unread", "Home › Tags", mark_unread).key("Mod+U").params(IDS).selection(needs_message),
        CommandSpec::new("mail.toggleRead", "Unread/ Read", "Home › Tags", toggle_read).params(IDS).selection(needs_message),
        CommandSpec::new("mail.markAllRead", "Mark All as Read", "Folder › Clean Up", mark_all_read)
            .params(r#"{"folder"?: number} (default: the folder in view)"#),
        CommandSpec::new("mail.flag", "Follow Up", "Home › Tags", flag)
            .key("Insert")
            .params(r#"{"due"?: "YYYY-MM-DD", "ids"?: [number]}"#)
            .selection(needs_message),
        CommandSpec::new("mail.clearFlag", "Clear Flag", "Home › Tags", clear_flag).params(IDS).selection(needs_message),
        CommandSpec::new("mail.complete", "Mark Complete", "Home › Tags", complete).params(IDS).selection(needs_message),
        CommandSpec::new("mail.toggleFlag", "Flag Message", "Home › Tags", toggle_flag).params(IDS).selection(needs_message),
        CommandSpec::new("mail.categorize", "Categorize", "Home › Tags", categorize)
            .params(r#"{"category": string, "value"?: bool (default: toggle), "ids"?: [number]}"#)
            .selection(needs_message),
        CommandSpec::new("mail.clearCategories", "Clear All Categories", "Home › Tags", clear_categories).params(IDS).selection(needs_message),
        CommandSpec::new("mail.pin", "Pin", "Home › Tags", pin)
            .params(r#"{"value"?: bool (default: toggle), "ids"?: [number]}"#)
            .selection(needs_message),
        CommandSpec::new("mail.junk", "Move to Junk", "Home › Delete", junk).params(IDS).selection(needs_movable),
        CommandSpec::new("mail.notJunk", "Not Junk", "Home › Delete", not_junk).key("Mod+Alt+J").params(IDS).selection(needs_movable),
        CommandSpec::new("mail.findRelated", "Find Related Messages", "Home › Find", find_related).pure().selection(needs_message),
        CommandSpec::new("mail.findFromSender", "Messages from Sender", "Home › Find", find_from_sender)
            .pure()
            .params(r#"{"id"?: number} (a message; default: the selected one)"#)
            .selection(needs_message),
        CommandSpec::new("mail.openAttachment", "Open Attachment", "Attachments › Actions", open_attachment)
            .pure()
            .params(
                r#"{"index": number, "id"?: number} — opens the file with the computer's own program for it, in the MailCraft window only (never for an agent). Programs, scripts, shortcuts, installers and disk images are refused: save them instead. Anywhere else answers why it opened nothing"#,
            )
            .selection(needs_message),
        CommandSpec::new("mail.saveAttachment", "Save Attachment", "Attachments › Actions", save_attachment)
            .pure()
            .params(r#"{"index": number, "id"?: number, "path"?: string} — without a path, answers the bytes as base64 `data`"#)
            .selection(needs_message),
        CommandSpec::new("mail.removeAttachment", "Remove Attachment", "Attachments › Actions", remove_attachment)
            .params(r#"{"index": number, "id"?: number}"#)
            .selection(needs_message),
    ]
}

const IDS: &str = r#"{"ids"?: [number]} (default: the selected messages)"#;
const EXTEND: &str = r#"{"extend"?: bool} — with `extend` (Shift), the selection runs from its anchor to the message moved to, as Shift-click does"#;
const PAGE: &str = r#"{"rows"?: number (the rows a page of the list holds; default 10, 1 to 10000), "extend"?: bool} — with `extend` (Shift), the selection runs from its anchor to the message moved to"#;

/// The rows a page is when the caller doesn't say (the window says how many fit).
const PAGE_ROWS: u64 = 10;
/// The most a page may be, whatever a caller says.
const MAX_PAGE_ROWS: u64 = 10_000;

fn select(s: &mut Session, v: &Value) -> CmdResult {
    let ids = p::ids(v).ok_or_else(|| bad("`id` or `ids` is required"))?;
    if let Some(missing) = ids.iter().find(|id| s.mailbox.message(**id).is_none()) {
        return Err(failed!("no message with id {id}", id = missing));
    }
    let Some(last) = ids.last().copied() else {
        s.sel.clear_messages();
        return done();
    };
    // Which of the message's rows was clicked, for one listed under several categories.
    let copy = p::u64(v, "copy").and_then(|c| u16::try_from(c).ok()).unwrap_or(0);
    // A Ctrl+click or a Shift+click adds to what the list shows selected, never to a message it
    // doesn't show (#485).
    if p::bool(v, "range") == Some(true) || p::bool(v, "extend") == Some(true) {
        let before = s.sel.messages.clone();
        let shown = s.in_list(&before);
        s.sel.messages.retain(|id| shown.contains(id));
    }
    if p::bool(v, "range") == Some(true) {
        // From the anchor (the first selected message) to this one, in list order.
        let rows = s.rows();
        let (anchor, anchor_copy) = range_anchor(s).unwrap_or((last, 0));
        if let (Some(a), Some(b)) = (position_of(&rows, anchor, anchor_copy), position_of(&rows, last, copy)) {
            s.sel.messages = span(&rows, anchor, a, last, b);
        }
    } else if p::bool(v, "extend") == Some(true) {
        for id in ids {
            match s.sel.messages.iter().position(|x| *x == id) {
                // Ctrl-click on a selected message takes it out again.
                Some(i) => {
                    s.sel.messages.remove(i);
                }
                None => s.sel.messages.push(id),
            }
        }
    } else {
        s.sel.messages = ids;
    }
    s.sel.focus_row = s.sel.focus().map(|id| (id, copy));
    // A range keeps its anchor; any other click is the next one's.
    if p::bool(v, "range") != Some(true) || s.sel.anchor_row.is_none_or(|(id, _)| !s.sel.messages.contains(&id)) {
        s.sel.anchor_row = s.sel.messages.first().map(|id| (*id, if Some(*id) == s.sel.focus() { copy } else { 0 }));
    }
    // Follow the message to its folder (a search result, a conversation member elsewhere).
    if let Some(m) = s.sel.focus().and_then(|id| s.mailbox.message(id))
        && s.view.search.is_empty()
        && m.folder != s.view.folder
    {
        s.view.folder = m.folder;
    }
    // Opened from a task, a contact or an alert (`elsewhere`): in its folder, in the All tab and
    // with no search, always (#485).
    if let Some(folder) = s.focused_message().map(|m| m.folder)
        && p::bool(v, "elsewhere") == Some(true)
    {
        s.view.folder = folder;
        s.show_whole_folder();
    }
    // What is selected is in the list (#485), and stays in it while it is selected (#428): opened
    // from outside it (a conversation in the reading pane, an agent), in its folder, in the All
    // tab and with no search, when the list doesn't show it there.
    s.show_selected();
    mark_opened(s);
    Ok(json!({"selected": s.sel.messages}))
}

/// Reading a message marks it read (File › Options). Not an undo step: it follows the selection.
fn mark_opened(s: &mut Session) {
    if !s.prefs.mark_read_on_open || s.sel.messages.len() != 1 {
        return;
    }
    if let Some(id) = s.sel.focus()
        && s.mailbox.message(id).is_some_and(|m| !m.read && !m.draft)
        && let Ok(m) = s.mailbox.message_mut(id)
    {
        m.read = true;
        s.dirty = true;
    }
}

fn select_all(s: &mut Session, _: &Value) -> CmdResult {
    let rows = s.rows();
    let focus = s.sel.focus();
    let mut ids: Vec<Id> = rows.iter().filter(|r| r.copy == 0).map(|r| r.id).filter(|id| Some(*id) != focus).collect();
    // The message in the reading pane stays the focus.
    ids.extend(focus.filter(|f| position(&rows, *f).is_some()));
    s.sel.messages = ids;
    s.keep_selected(&rows);
    Ok(json!({"selected": s.sel.messages.len()}))
}

/// Where a range runs from: the anchor's row while its message is still selected, else the first
/// selected message.
fn range_anchor(s: &Session) -> Option<(Id, u16)> {
    s.sel.anchor_row.filter(|(id, _)| s.sel.messages.contains(id)).or_else(|| s.sel.messages.first().map(|id| (*id, s.sel.anchor_copy(*id))))
}

/// The messages of rows `a` (the anchor's) to `b` (the focus's), in list order, each once (a
/// message listed under several categories has several rows, #327): the anchor first and the
/// focus last, so that the reading pane shows the focus.
fn span(rows: &[Row], anchor: Id, a: usize, focus: Id, b: usize) -> Vec<Id> {
    let (lo, hi) = (a.min(b), a.max(b));
    let mut seen = std::collections::HashSet::new();
    let mut picked: Vec<Id> =
        rows.iter().skip(lo).take((hi - lo).saturating_add(1)).map(|r| r.id).filter(|id| *id != anchor && *id != focus && seen.insert(*id)).collect();
    if focus != anchor {
        picked.insert(0, anchor);
    }
    picked.push(focus);
    picked
}

/// Moves the focus to row `i` of the list. With `extend` (Shift), the selection runs from the
/// anchor, which stays where it is (so going back makes the range smaller), to it; otherwise it
/// is that one message, which the next range runs from.
fn land(s: &mut Session, rows: &[Row], i: usize, extend: bool) -> CmdResult {
    let Some(row) = rows.get(i) else { return done() };
    let anchor = if extend { range_anchor(s) } else { None };
    match anchor.and_then(|(id, copy)| Some((id, copy, position_of(rows, id, copy)?))) {
        Some((id, copy, at)) => {
            s.sel.messages = span(rows, id, at, row.id, i);
            s.sel.anchor_row = Some((id, copy));
        }
        None => {
            s.sel.messages = vec![row.id];
            s.sel.anchor_row = Some((row.id, row.copy));
        }
    }
    s.sel.focus_row = Some((row.id, row.copy));
    s.keep_selected(rows);
    mark_opened(s);
    Ok(json!({"selected": s.sel.messages}))
}

fn extend(v: &Value) -> bool {
    p::bool(v, "extend") == Some(true)
}

/// Moves the focus `by` rows, clamped to the list; from no focus, down starts at the first row
/// and up at the last.
fn step(s: &mut Session, by: i64, v: &Value) -> CmdResult {
    let rows = s.rows();
    let Some(end) = rows.len().checked_sub(1) else { return done() };
    let at = s.sel.focus().and_then(|id| position_of(&rows, id, s.sel.focus_copy()));
    let i = match at {
        Some(i) => {
            let to = i64::try_from(i).unwrap_or(i64::MAX).saturating_add(by).max(0);
            usize::try_from(to).unwrap_or(end).min(end)
        }
        None if by >= 0 => 0,
        None => end,
    };
    land(s, &rows, i, extend(v))
}

fn next(s: &mut Session, v: &Value) -> CmdResult {
    step(s, 1, v)
}

fn previous(s: &mut Session, v: &Value) -> CmdResult {
    step(s, -1, v)
}

fn first(s: &mut Session, v: &Value) -> CmdResult {
    let rows = s.rows();
    land(s, &rows, 0, extend(v))
}

fn last(s: &mut Session, v: &Value) -> CmdResult {
    let rows = s.rows();
    let Some(end) = rows.len().checked_sub(1) else { return done() };
    land(s, &rows, end, extend(v))
}

/// A page: the rows the caller says fit, clamped to 1 to 10,000.
fn page(v: &Value) -> i64 {
    i64::try_from(p::u64(v, "rows").unwrap_or(PAGE_ROWS).clamp(1, MAX_PAGE_ROWS)).unwrap_or(1)
}

fn page_down(s: &mut Session, v: &Value) -> CmdResult {
    step(s, page(v), v)
}

fn page_up(s: &mut Session, v: &Value) -> CmdResult {
    step(s, -page(v), v)
}

fn get(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    if s.mailbox.message(id).is_none() {
        return Err(failed!("no message with id {id}", id));
    }
    // Its text, from where the mailbox keeps it when it isn't in memory (a text that can't be
    // read says so in its place).
    let _ = s.load_where(id, |_| false);
    let m = s.mailbox.messages.get(&id).cloned().ok_or_else(|| failed!("no message with id {id}", id))?;
    // What may not be what it seems about it (sender warnings): worked out by MailCraft from the
    // account's server's verdict, the sender and the links, never from what the message asks.
    let warnings = s.sender_warnings(&m, true);
    let mut v = message_json(s, &m);
    if let Some(o) = v.as_object_mut() {
        o.insert("warnings".into(), Value::Array(warnings.iter().map(mailcraft_model::sender_warnings::Warning::json).collect()));
    }
    Ok(v)
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let limit = p::u64(v, "limit").unwrap_or(200).min(10_000) as usize;
    let rows = s.rows();
    let items: Vec<Value> = rows
        .iter()
        .take(limit)
        .filter_map(|r| {
            let mut m = message_summary(s, s.mailbox.message(r.id)?);
            if let Some(o) = m.as_object_mut() {
                if r.members.len() > 1 {
                    o.insert("conversation".into(), json!(r.members));
                }
                if let Some(g) = &r.group {
                    o.insert("group".into(), json!(g));
                }
                if r.copy > 0 {
                    o.insert("copy".into(), json!(r.copy));
                }
            }
            Some(m)
        })
        .collect();
    Ok(json!({"folder": s.view.folder, "total": listed(&rows), "rows": rows.len(), "messages": items}))
}

fn delete(s: &mut Session, v: &Value) -> CmdResult {
    let ids = targets(s, v)?;
    let before = s.rows();
    let mut gone = 0;
    for id in &ids {
        gone += usize::from(s.mailbox.delete_message(*id)?);
    }
    select_after_removal(s, &before, &ids);
    Ok(json!({"deleted": ids.len(), "permanently": gone}))
}

fn delete_permanently(s: &mut Session, v: &Value) -> CmdResult {
    let ids = targets(s, v)?;
    let before = s.rows();
    for id in &ids {
        s.mailbox.remove_message(*id)?;
    }
    select_after_removal(s, &before, &ids);
    Ok(json!({"deleted": ids.len(), "permanently": ids.len()}))
}

/// Move `ids` to each one's account folder of `kind`.
fn move_to_kind(s: &mut Session, ids: &[Id], kind: FolderKind) -> CmdResult {
    let before = s.rows();
    let mut moved = Vec::new();
    for id in ids {
        let account = s.mailbox.message(*id).map(|m| m.account).ok_or_else(|| failed!("no message with id {id}", id))?;
        let folder = s
            .mailbox
            .folder_of_kind(account, kind)
            .ok_or_else(|| failed!("this account has no {folder} folder", folder = mailcraft_i18n::Arg::folder(kind.default_name())))?;
        if s.mailbox.message(*id).is_some_and(|m| m.folder != folder) {
            s.mailbox.move_message(*id, folder)?;
            moved.push(*id);
        }
    }
    select_after_removal(s, &before, &moved);
    Ok(json!({"moved": moved.len()}))
}

fn archive(s: &mut Session, v: &Value) -> CmdResult {
    let ids = targets(s, v)?;
    move_to_kind(s, &ids, FolderKind::Archive)
}

fn junk(s: &mut Session, v: &Value) -> CmdResult {
    let ids = targets(s, v)?;
    move_to_kind(s, &ids, FolderKind::Junk)
}

fn not_junk(s: &mut Session, v: &Value) -> CmdResult {
    let ids = targets(s, v)?;
    move_to_kind(s, &ids, FolderKind::Inbox)
}

fn move_to(s: &mut Session, v: &Value) -> CmdResult {
    let folder = p::req_id(v, "folder")?;
    let ids = targets(s, v)?;
    let before = s.rows();
    let mut moved = Vec::new();
    for id in &ids {
        if s.mailbox.message(*id).is_some_and(|m| m.folder != folder) {
            s.mailbox.move_message(*id, folder)?;
            moved.push(*id);
        }
    }
    select_after_removal(s, &before, &moved);
    Ok(json!({"moved": moved.len(), "folder": folder}))
}

fn copy_to(s: &mut Session, v: &Value) -> CmdResult {
    let folder = p::req_id(v, "folder")?;
    let ids = targets(s, v)?;
    let mut copies = Vec::new();
    for id in &ids {
        copies.push(s.mailbox.copy_message_marked(*id, folder, u64::try_from(s.now()).unwrap_or_default())?);
    }
    Ok(json!({"copies": copies, "folder": folder}))
}

/// Apply `change` to each target; answers how many messages it changed.
fn each(s: &mut Session, v: &Value, change: impl Fn(&mut mailcraft_model::Message)) -> CmdResult {
    let ids = targets(s, v)?;
    let mut changed = 0;
    for id in &ids {
        let Some(old) = s.mailbox.message(*id) else { continue };
        let mut new = old.clone();
        change(&mut new);
        // Untouched messages stay shared with the undo snapshot.
        if new != *old {
            *s.mailbox.message_mut(*id)? = new;
            changed += 1;
        }
    }
    Ok(json!({"changed": changed}))
}

fn mark_read(s: &mut Session, v: &Value) -> CmdResult {
    each(s, v, |m| m.read = true)
}

fn mark_unread(s: &mut Session, v: &Value) -> CmdResult {
    each(s, v, |m| m.read = false)
}

fn toggle_read(s: &mut Session, v: &Value) -> CmdResult {
    // Like the ribbon button: if anything is unread, mark all read; else mark all unread.
    let ids = targets(s, v)?;
    let any_unread = ids.iter().filter_map(|id| s.mailbox.message(*id)).any(|m| !m.read);
    each(s, v, |m| m.read = any_unread)
}

fn mark_all_read(s: &mut Session, v: &Value) -> CmdResult {
    let folder = p::id(v, "folder").unwrap_or(s.view.folder);
    if s.mailbox.folder(folder).is_none() {
        return Err(failed!("no folder with id {id}", id = folder));
    }
    let ids: Vec<Id> = s.mailbox.messages_in(folder).filter(|m| !m.read).map(|m| m.id).collect();
    for id in &ids {
        s.mailbox.message_mut(*id)?.read = true;
    }
    Ok(json!({"changed": ids.len()}))
}

fn flag(s: &mut Session, v: &Value) -> CmdResult {
    let due = p::date(v, "due")?;
    let offset = s.offset_at(s.now());
    // Due at the end of the working day.
    let due = due.and_then(|d| mailcraft_time::DateTime::new(d, s.prefs.work_end.min(23), 0, 0)).map(|t| t.to_timestamp(offset));
    each(s, v, |m| {
        m.flag = FlagStatus::Flagged;
        m.flag_due = due.or(m.flag_due);
    })
}

fn clear_flag(s: &mut Session, v: &Value) -> CmdResult {
    each(s, v, |m| {
        m.flag = FlagStatus::None;
        m.flag_due = None;
    })
}

fn complete(s: &mut Session, v: &Value) -> CmdResult {
    each(s, v, |m| m.flag = FlagStatus::Complete)
}

fn toggle_flag(s: &mut Session, v: &Value) -> CmdResult {
    let ids = targets(s, v)?;
    let all_flagged = ids.iter().filter_map(|id| s.mailbox.message(*id)).all(|m| m.flag == FlagStatus::Flagged);
    each(s, v, |m| {
        m.flag = if all_flagged { FlagStatus::None } else { FlagStatus::Flagged };
        if all_flagged {
            m.flag_due = None;
        }
    })
}

fn categorize(s: &mut Session, v: &Value) -> CmdResult {
    let name = mailcraft_model::clean_name(p::req_str(v, "category")?).ok_or_else(|| bad("`category` is empty"))?;
    // A category nobody defined yet joins the list, in grey, so it can be given a colour.
    let name = match s.mailbox.category(&name) {
        Some(c) => c.name.clone(),
        None => {
            s.mailbox.set_category(&name, Rgb(0x8A, 0x8A, 0x8A))?;
            name
        }
    };
    let ids = targets(s, v)?;
    let all_have = ids.iter().filter_map(|id| s.mailbox.message(*id)).all(|m| m.has_category(&name));
    let on = p::bool(v, "value").unwrap_or(!all_have);
    each(s, v, |m| {
        m.categories.retain(|c| !c.eq_ignore_ascii_case(&name));
        if on {
            m.categories.push(name.clone());
        }
    })
}

fn clear_categories(s: &mut Session, v: &Value) -> CmdResult {
    each(s, v, |m| m.categories.clear())
}

fn pin(s: &mut Session, v: &Value) -> CmdResult {
    let ids = targets(s, v)?;
    let all_pinned = ids.iter().filter_map(|id| s.mailbox.message(*id)).all(|m| m.pinned);
    let on = p::bool(v, "value").unwrap_or(!all_pinned);
    each(s, v, |m| m.pinned = on)
}

fn find_related(s: &mut Session, _: &Value) -> CmdResult {
    let m = s.focused_message().ok_or_else(|| fail("select a message first"))?;
    let topic = m.topic();
    if topic.is_empty() {
        return Err(fail("this message has no subject to look for"));
    }
    // Written so that the search reads it back as it is: a backslash at its end keeps its
    // closing quote from being read as part of it (#301).
    s.view.search = format!("subject:{}", mailcraft_model::search::keyword_value(&topic));
    s.view.scope = crate::Scope::Everywhere;
    Ok(json!({"search": s.view.search, "found": listed(&s.rows())}))
}

/// Mail from the sender of a message (the one in focus, or `id`), in every mailbox: a search for
/// its address, which finds that address and no other (#301), left in the search box to be
/// narrowed. A quoted name (`"ada smith"@example.org`) is written so that the search reads it
/// back whole.
fn find_from_sender(s: &mut Session, v: &Value) -> CmdResult {
    use mailcraft_model::search::{Query, is_address, keyword_value};
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    let address = m.from.email.trim();
    if address.is_empty() {
        return Err(fail("this message has no sender address to look for"));
    }
    let search = format!("from:{}", keyword_value(address));
    // What is written is read back as that one address, or not written at all: an address the
    // search doesn't take as a whole one (`ada@[192.0.2.1]`) is said to be one.
    let query = Query::parse(&search);
    if !is_address(address) || query.len() != 1 || !query.is_address_term(0) || !query.matches(m, 0) {
        return Err(fail("the search can't look for this sender's address as a whole address"));
    }
    s.view.search = search;
    s.view.scope = crate::Scope::Everywhere;
    Ok(json!({"search": s.view.search, "found": listed(&s.rows())}))
}

fn attachment_target(s: &Session, v: &Value) -> Result<(Id, usize), crate::CmdError> {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let index = p::u64(v, "index").ok_or_else(|| bad("`index` (number) is required"))? as usize;
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    if m.attachments.get(index).is_none() {
        return Err(failed!("the message has no attachment {index}", index));
    }
    Ok((id, index))
}

fn save_attachment(s: &mut Session, v: &Value) -> CmdResult {
    let (id, index) = attachment_target(s, v)?;
    let a = s.mailbox.message(id).and_then(|m| m.attachments.get(index)).ok_or_else(|| fail("no such attachment"))?;
    // A message can name its attachment anything: the name is made safe as for opening it.
    let name = crate::opening::save_name(&a.name);
    // The file's bytes, from where the mailbox keeps them when they aren't in memory.
    let bytes = s.content(id, crate::Part::Attachment(index))?;
    let out = deliver(v, &name, bytes.as_ref().clone())?;
    // A saved attachment came from mail: mark it as a browser marks a download.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = p::str(v, "path") {
        crate::mark::mark_from_internet(std::path::Path::new(path));
    }
    Ok(out)
}

/// The window opens attachments (through its host); everywhere else this says why nothing was
/// opened: refused for a program or a script, or not the place for it.
fn open_attachment(s: &mut Session, v: &Value) -> CmdResult {
    let (id, index) = attachment_target(s, v)?;
    let name = s.mailbox.message(id).and_then(|m| m.attachments.get(index)).map(|a| a.name.clone()).ok_or_else(|| fail("no such attachment"))?;
    Err(match crate::opening::verdict(&name) {
        crate::opening::Verdict::Blocked => fail(crate::opening::blocked_message(&name)),
        _ => crate::CmdError::Disabled("attachments are opened in the MailCraft window; `mail.saveAttachment` gives the file".into()),
    })
}

fn remove_attachment(s: &mut Session, v: &Value) -> CmdResult {
    let (id, index) = attachment_target(s, v)?;
    let m = s.mailbox.message_mut(id)?;
    let removed = m.attachments.remove(index);
    Ok(json!({"removed": removed.name}))
}

#[cfg(test)]
#[path = "from_sender_tests.rs"]
mod from_sender_tests;
