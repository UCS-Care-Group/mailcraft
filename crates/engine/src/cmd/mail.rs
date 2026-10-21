// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Mail: selecting, reading, filing, flagging and deleting messages.

use mailcraft_model::{FlagStatus, FolderKind, Id, Rgb};
use serde_json::{Value, json};

use super::{bad, deliver, done, fail, message_json, message_summary, needs_message, select_after_removal, targets};
use crate::list::position;
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("mail.select", "Select Message", "Mail", select)
            .pure()
            .params(r#"{"id": number} | {"ids": [number]}, "extend"?: bool (add to the selection), "range"?: bool (select up to it)"#),
        CommandSpec::new("mail.selectAll", "Select All", "Folder › Actions", select_all).pure().key("Mod+A"),
        CommandSpec::new("mail.next", "Next Message", "Mail", next).pure().key("ArrowDown"),
        CommandSpec::new("mail.previous", "Previous Message", "Mail", previous).pure().key("ArrowUp"),
        CommandSpec::new("mail.get", "Get Message", "Mail", get).pure().params(r#"{"id"?: number} (default: the selected message)"#),
        CommandSpec::new("mail.list", "List Messages", "Mail", list).pure().params(r#"{"limit"?: number} — the message list in view"#),
        CommandSpec::new("mail.delete", "Delete", "Home › Delete", delete).key("Delete / Mod+D").params(IDS).selection(needs_message),
        CommandSpec::new("mail.deletePermanently", "Permanently Delete", "Home › Delete", delete_permanently)
            .key("Shift+Delete")
            .params(IDS)
            .selection(needs_message),
        CommandSpec::new("mail.archive", "Archive", "Home › Delete", archive).key("Backspace").params(IDS).selection(needs_message),
        CommandSpec::new("mail.move", "Move to Folder", "Home › Move", move_to)
            .key("Mod+Shift+V")
            .params(r#"{"folder": number, "ids"?: [number]}"#)
            .selection(needs_message),
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
        CommandSpec::new("mail.junk", "Block Sender", "Home › Delete", junk).params(IDS).selection(needs_message),
        CommandSpec::new("mail.notJunk", "Not Junk", "Home › Delete", not_junk).key("Mod+Alt+J").params(IDS).selection(needs_message),
        CommandSpec::new("mail.findRelated", "Find Related Messages", "Home › Find", find_related).pure().selection(needs_message),
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

fn select(s: &mut Session, v: &Value) -> CmdResult {
    let ids = p::ids(v).ok_or_else(|| bad("`id` or `ids` is required"))?;
    if let Some(missing) = ids.iter().find(|id| s.mailbox.message(**id).is_none()) {
        return Err(failed!("no message with id {id}", id = missing));
    }
    let Some(last) = ids.last().copied() else {
        s.sel.messages.clear();
        return done();
    };
    if p::bool(v, "range") == Some(true) {
        // From the anchor (the first selected message) to this one, in list order.
        let rows = s.rows();
        let anchor = s.sel.messages.first().copied().unwrap_or(last);
        if let (Some(a), Some(b)) = (position(&rows, anchor), position(&rows, last)) {
            let (lo, hi) = (a.min(b), a.max(b));
            let mut picked: Vec<Id> = rows.iter().skip(lo).take(hi - lo + 1).map(|r| r.id).filter(|id| *id != anchor && *id != last).collect();
            picked.insert(0, anchor);
            if last != anchor {
                picked.push(last);
            }
            s.sel.messages = picked;
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
    // Follow the message to its folder (a search result, a conversation member elsewhere).
    if let Some(m) = s.sel.focus().and_then(|id| s.mailbox.message(id))
        && s.view.search.is_empty()
        && m.folder != s.view.folder
    {
        s.view.folder = m.folder;
    }
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
    let mut ids: Vec<Id> = rows.iter().map(|r| r.id).filter(|id| Some(*id) != focus).collect();
    // The message in the reading pane stays the focus.
    ids.extend(focus.filter(|f| position(&rows, *f).is_some()));
    s.sel.messages = ids;
    Ok(json!({"selected": s.sel.messages.len()}))
}

fn step(s: &mut Session, by: i64) -> CmdResult {
    let rows = s.rows();
    if rows.is_empty() {
        return done();
    }
    let at = s.sel.focus().and_then(|id| position(&rows, id));
    let i = match at {
        Some(i) => (i as i64).saturating_add(by).clamp(0, rows.len() as i64 - 1) as usize,
        None if by >= 0 => 0,
        None => rows.len() - 1,
    };
    if let Some(row) = rows.get(i) {
        s.sel.messages = vec![row.id];
    }
    mark_opened(s);
    Ok(json!({"selected": s.sel.messages}))
}

fn next(s: &mut Session, _: &Value) -> CmdResult {
    step(s, 1)
}

fn previous(s: &mut Session, _: &Value) -> CmdResult {
    step(s, -1)
}

fn get(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::id(v, "id").or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    if s.mailbox.message(id).is_none() {
        return Err(failed!("no message with id {id}", id));
    }
    // Its text, from where the mailbox keeps it when it isn't in memory (a text that can't be
    // read says so in its place).
    let _ = s.load_where(id, |_| false);
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    Ok(message_json(s, m))
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
            }
            Some(m)
        })
        .collect();
    Ok(json!({"folder": s.view.folder, "total": rows.len(), "messages": items}))
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
    s.view.search = format!("subject:\"{}\"", topic.replace('"', ""));
    s.view.scope = crate::Scope::Everywhere;
    Ok(json!({"search": s.view.search, "found": s.rows().len()}))
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
    deliver(v, &name, bytes.as_ref().clone())
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
