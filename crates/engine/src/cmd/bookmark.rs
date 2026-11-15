// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local bookmarks in messages and conversations (#951).

use mailcraft_model::bookmark::clean_text;
use mailcraft_model::{Bookmark, MAX_BOOKMARK_NAME, MAX_BOOKMARK_OFFSET, MAX_BOOKMARK_QUOTE};
use serde_json::{Value, json};

use super::{done, fail, needs_message};
use crate::{CmdResult, CommandSpec, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bookmark.add", "Add Bookmark", "Home › Tags", add)
            .params(r#"{"id"?: number (message id, default focused), "name"?: string, "quote"?: string, "offset"?: number (character offset in body plain text)} — adds a local mark to a message or character offset"#)
            .selection(needs_message),
        CommandSpec::new("bookmark.remove", "Remove Bookmark", "Home › Tags", remove)
            .params(r#"{"id": number} — removes the bookmark by its unique id"#),
        CommandSpec::new("bookmark.list", "List Bookmarks", "Home › Tags", list)
            .reads()
            .params(r#"{"id"?: number (message id, default focused), "conversation"?: bool (if true, lists for whole conversation)} — answers {"bookmarks": [{"id", "message", "name", "quote", "offset", "created"}]}"#)
            .selection(needs_message),
        CommandSpec::new("bookmark.jump", "Jump to Bookmark", "Home › Tags", jump)
            .reads()
            .params(r#"{"id": number} — selects the message and jumps to the bookmark's position"#),
    ]
}

fn add(s: &mut Session, v: &Value) -> CmdResult {
    let message_id = p::message_strict(v)?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let message = s.mailbox.message(message_id).ok_or_else(|| fail(format!("no message with id {message_id}")))?;
    let raw_name = p::str(v, "name").unwrap_or_default();
    let quote = p::str(v, "quote").unwrap_or_default();
    let offset = p::i64(v, "offset").map_or(0, |n| n.clamp(0, MAX_BOOKMARK_OFFSET as i64) as usize);

    let cleaned_name = clean_text(raw_name, MAX_BOOKMARK_NAME);
    let cleaned_quote = clean_text(quote, MAX_BOOKMARK_QUOTE);
    let effective_name =
        if cleaned_name.is_empty() && cleaned_quote.is_empty() { clean_text(&message.subject, MAX_BOOKMARK_NAME) } else { cleaned_name };

    if let Some(existing) = s.mailbox.find_bookmark(message_id, &cleaned_quote, offset) {
        return Ok(json!({
            "id": existing.id,
            "message": existing.message,
            "name": existing.name,
            "quote": existing.quote,
            "offset": existing.offset
        }));
    }

    if s.mailbox.bookmarks().len() >= mailcraft_model::MAX_BOOKMARKS {
        return Err(fail("too many bookmarks"));
    }

    let now = s.now();
    let id = s.mailbox.alloc_id();

    let bookmark = Bookmark::new(id, message_id, &effective_name, quote, offset, now);
    if !s.mailbox.add_bookmark(bookmark.clone()) {
        return Err(fail("too many bookmarks"));
    }
    Ok(json!({
        "id": id,
        "message": bookmark.message,
        "name": bookmark.name,
        "quote": bookmark.quote,
        "offset": bookmark.offset
    }))
}

fn remove(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    if !s.mailbox.remove_bookmark(id) {
        return Err(fail(format!("no bookmark with id {id}")));
    }
    done()
}

fn list(s: &mut Session, v: &Value) -> CmdResult {
    let message_id = p::message_strict(v)?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let conversation = p::bool(v, "conversation").unwrap_or(false);

    let bookmarks = if conversation {
        let conversation_ids = s.conversation(message_id);
        if conversation_ids.is_empty() { s.mailbox.bookmarks_for_message(message_id) } else { s.mailbox.bookmarks_for_messages(&conversation_ids) }
    } else {
        s.mailbox.bookmarks_for_message(message_id)
    };

    let list: Vec<Value> = bookmarks
        .into_iter()
        .map(|b| {
            json!({
                "id": b.id,
                "message": b.message,
                "name": b.name,
                "quote": b.quote,
                "offset": b.offset,
                "created": b.created
            })
        })
        .collect();

    Ok(json!({ "bookmarks": list }))
}

fn jump(s: &mut Session, v: &Value) -> CmdResult {
    let id = p::req_id(v, "id")?;
    let (message_id, quote, offset) = {
        let bm = s.mailbox.bookmark(id).ok_or_else(|| fail(format!("no bookmark with id {id}")))?;
        (bm.message, bm.quote.clone(), bm.offset)
    };
    if s.mailbox.message(message_id).is_none() {
        return Err(fail(format!("message {message_id} no longer exists")));
    }
    // Select the message if not already uniquely selected, or if it sits outside the current view.
    let in_view = !s.in_list(&[message_id]).is_empty();
    if s.sel.focus() != Some(message_id) || s.sel.messages.len() != 1 || !in_view {
        let mut args = json!({ "id": message_id });
        if !in_view {
            args["elsewhere"] = json!(true);
        }
        s.run("mail.select", &args)?;
    }
    Ok(json!({
        "id": id,
        "message": message_id,
        "quote": quote,
        "offset": offset
    }))
}
