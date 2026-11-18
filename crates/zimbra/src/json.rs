// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Reading what a Zimbra server wrote in JSON, as hostile data: nothing is indexed, every list is
//! capped by its caller, every number checked, every text cleaned.
//!
//! Zimbra writes the same thing in more than one shape (an attribute as a string or a number, an
//! element's text as `{"_content": …}`, a list of one as an object): each reader here takes all of
//! them, and nothing else.

use serde_json::Value;

/// Longest text taken from an answer, in characters, before its caller cuts it further.
pub(crate) const MAX_TEXT: usize = 16 * 1024;

/// The text of a value as Zimbra writes it: a string, a number, `{"_content": …}`, or a list of
/// those (the first). Control characters but tab and line breaks are taken out, and it is held to
/// [`MAX_TEXT`] characters.
pub(crate) fn text(value: Option<&Value>) -> Option<String> {
    fn raw(value: &Value, depth: u8) -> Option<String> {
        if depth > 4 {
            return None;
        }
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Object(map) => map.get("_content").and_then(|v| raw(v, depth + 1)),
            Value::Array(list) => list.first().and_then(|v| raw(v, depth + 1)),
            Value::Null => None,
        }
    }
    let text = raw(value?, 0)?;
    Some(text.chars().filter(|c| !c.is_control() || matches!(c, '\t' | '\n' | '\r')).take(MAX_TEXT).collect())
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

/// Whether a flag is on: `true`, `1` or `"1"`, `"true"`.
pub(crate) fn on(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_u64() == Some(1),
        Some(other) => text(Some(other)).is_some_and(|t| matches!(t.trim(), "1" | "true" | "TRUE")),
        None => false,
    }
}

/// An item's or a folder's id in the account's own mailbox: digits, and not zero. An id of
/// another mailbox (`<account id>:257`, a folder shared with the owner) is not one.
pub(crate) fn own_id(value: Option<&Value>) -> Option<String> {
    let text = text(value)?;
    let text = text.trim();
    (!text.is_empty() && text.len() <= 10 && text.bytes().all(|b| b.is_ascii_digit()) && text.bytes().any(|b| b != b'0')).then(|| text.to_string())
}

/// A list Zimbra may write as one object when there is one.
pub(crate) fn list(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(items)) => items,
        Some(one @ Value::Object(_)) => std::slice::from_ref(one),
        _ => &[],
    }
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
        _ => return Some(Vec::new()),
    };
    if raw.len() > MAX_LIST {
        return None;
    }
    Some(raw.split(',').filter_map(|id| own_id(Some(&Value::String(id.to_string())))).take(most).collect())
}
