// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading what a Zimbra server wrote in JSON, as hostile data: nothing is indexed, every list is
//! capped by its caller, every number checked, every text cleaned.
//!
//! Zimbra writes the same thing in more than one shape (an attribute as a string or a number, an
//! element's text as `{"_content": …}`, a list of one as an object): each reader here takes all of
//! them, and nothing else.

use serde_json::Value;

/// Longest text taken from an answer, in bytes: the unit the model's own check counts, so a
/// text of many-byte characters can't pass here and be refused there.
pub(crate) const MAX_TEXT: usize = 16 * 1024;

/// The text of a value in the shapes Zimbra writes it, as it was sent — before [`text`]'s
/// cleaning, so a check on an id, a token or a value compared with the server's word sees the
/// real one: a character [`text`] would take out must never make a value it wasn't pass for one
/// it was.
pub(crate) fn raw(value: Option<&Value>) -> Option<String> {
    fn raw_in(value: &Value, depth: u8) -> Option<String> {
        if depth > 4 {
            return None;
        }
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Object(map) => map.get("_content").and_then(|v| raw_in(v, depth + 1)),
            Value::Array(list) => list.first().and_then(|v| raw_in(v, depth + 1)),
            Value::Null => None,
        }
    }
    raw_in(value?, 0)
}

/// The text of a value as Zimbra writes it: a string, a number, `{"_content": …}`, or a list of
/// those (the first). Control characters but tab and line breaks are taken out, and it is held to
/// [`MAX_TEXT`] bytes, whole characters only.
pub(crate) fn text(value: Option<&Value>) -> Option<String> {
    let text = raw(value)?;
    let mut bytes = 0;
    Some(
        text.chars()
            .filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r'))
            .take_while(|c| {
                bytes += c.len_utf8();
                bytes <= MAX_TEXT
            })
            .collect(),
    )
}

/// A text for one line (a name, a subject): control characters made spaces, trimmed.
pub(crate) fn line(value: Option<&Value>) -> String {
    text(value).map(|t| t.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()).unwrap_or_default()
}

/// A whole number that is not negative.
pub(crate) fn number(value: Option<&Value>) -> Option<u64> {
    match value? {
        Value::Number(n) => n.as_u64(),
        Value::String(text) => text.trim().parse::<u64>().ok(),
        _ => None,
    }
}

/// Whether a flag is on, when the value is a flag at all: `None` for one that is there but in
/// no shape a flag is written — reading one off would end a read or a check before its end.
pub(crate) fn checked_on(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(1) => Some(true),
            Some(0) => Some(false),
            _ => None,
        },
        Some(other) => match raw(Some(other))?.trim() {
            "1" | "true" | "TRUE" => Some(true),
            "0" | "false" | "FALSE" => Some(false),
            _ => None,
        },
        None => Some(false),
    }
}

/// An item's or a folder's id in the account's own mailbox: digits, and not zero. An id of
/// another mailbox (`<account id>:257`, a folder shared with the owner) is not one. The check is
/// the value as it was sent: [`text`] takes control characters out, and a cleaned-up copy of an
/// id that isn't one could pass for a different id that is.
pub(crate) fn own_id(value: Option<&Value>) -> Option<String> {
    let raw = match value? {
        Value::String(text) => text.as_str(),
        Value::Number(n) => {
            return Some(n.to_string())
                .filter(|s| !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()) && s.bytes().any(|b| b != b'0'));
        }
        Value::Object(map) => return own_id(map.get("_content")),
        Value::Array(list) => return own_id(list.first()),
        _ => return None,
    };
    let text = raw.trim();
    (!text.is_empty() && text.len() <= 10 && text.bytes().all(|b| b.is_ascii_digit()) && text.bytes().any(|b| b != b'0')).then(|| text.to_string())
}

/// Whether `value` is an id of another mailbox (`<account>:<id>`): a folder or item shared with
/// the owner, where [`own_id`] is `None` for the same reason. The same shapes [`own_id`] accepts
/// are read, on the value as sent.
pub(crate) fn foreign_id(value: Option<&Value>) -> bool {
    let raw = match value {
        Some(Value::String(text)) => text.as_str(),
        Some(Value::Object(map)) => return foreign_id(map.get("_content")),
        Some(Value::Array(list)) => return foreign_id(list.first()),
        _ => return false,
    };
    let raw = raw.trim();
    match raw.split_once(':') {
        Some((a, b)) => !a.is_empty() && !b.is_empty() && raw.len() <= 128 && !raw.chars().any(char::is_control),
        None => false,
    }
}

/// A list Zimbra may write as one object when there is one.
pub(crate) fn list(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(items)) => items,
        Some(one @ Value::Object(_)) => std::slice::from_ref(one),
        _ => &[],
    }
}

/// A field the answer holds that is no list of anything — neither the array Zimbra writes for
/// many, the object it writes for one, nor null: there but unreadable, where [`list`] would pass
/// it for an empty list and a page or a listing would come out complete while it isn't.
pub(crate) fn bad_list(value: Option<&Value>) -> bool {
    matches!(value, Some(v) if !matches!(v, Value::Array(_) | Value::Object(_) | Value::Null))
}

/// The ids of a comma-separated list (`"257,258"`), own ids only, at most `most` of them. The
/// raw text is split, never [`text`]'s cut copy: a list longer than the cap is no list at all
/// (`None`), so that a token cut in two can never pass for an id the server didn't send.
pub(crate) fn ids(value: Option<&Value>, most: usize) -> Option<Vec<String>> {
    const MAX_LIST: usize = 64 * 1024;
    let raw = match value? {
        Value::String(text) => text.as_str(),
        Value::Object(map) => match map.get("_content") {
            Some(Value::String(text)) => text.as_str(),
            _ => return None,
        },
        // A shape that isn't a list of ids is no list at all, rather than an empty one: an empty
        // one would have its caller take the page's token for good.
        _ => return None,
    };
    // A control character in a list of ids is no odd id to pass over: left in, it would turn one
    // id into another the server didn't name — a deletion could take a message it wasn't for.
    if raw.len() > MAX_LIST || raw.chars().any(char::is_control) {
        return None;
    }
    let mut out = Vec::new();
    for id in raw.split(',') {
        let value = Value::String(id.to_string());
        // An id of another mailbox's item (`<zid>:<rid>`) is passed over; anything else is
        // no id at all — dropping it while the page's token is kept would lose a deletion
        // the server did send.
        match own_id(Some(&value)) {
            Some(id) => out.push(id),
            None if foreign_id(Some(&value)) => {}
            None => return None,
        }
        if out.len() >= most {
            break;
        }
    }
    Some(out)
}
