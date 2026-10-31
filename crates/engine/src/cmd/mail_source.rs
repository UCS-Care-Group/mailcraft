// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! View Source: a message's header lines and the source of its body (its HTML, else its text), as
//! plain text to read and copy.
//!
//! A message is hostile, and so is its source: nothing of it is rendered or fetched here, and it
//! comes back as text alone. Its characters are already text (the message's character set was
//! read when it came in, `mailcraft-charset`); what can't be seen is made visible: a control
//! character as its picture (`␛`), a format character or another that shows nothing as its code
//! (`<U+202E>`), so that a right-to-left override or a zero-width space can't hide or turn what is
//! around it. The whole is at most [`MAX_SOURCE`] bytes.

use mailcraft_model::{Message, format_list};
use serde_json::{Value, json};

use crate::cmd::fail;
use crate::{CmdResult, Part, Session, p};

/// The most bytes of source shown; a longer one is cut at a character, and says so.
pub const MAX_SOURCE: usize = 1 << 20;

/// `mail.viewSource`.
pub(super) fn view_source(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id"])?;
    let id = p::id_strict(v, "id")?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    let mut raw = header_lines(m, s.offset_at(m.date));
    raw.push('\n');
    // The body as it is kept, read back from where the mailbox keeps it when it isn't in memory.
    let html = s.content(id, Part::Html)?;
    let body = if html.is_empty() { s.content(id, Part::Text)? } else { html };
    raw.push_str(&String::from_utf8_lossy(&body));
    let (source, truncated) = visible(&raw, MAX_SOURCE);
    Ok(json!({"id": id, "subject": m.subject, "source": source, "truncated": truncated}))
}

/// The message's header lines, as it was read: those MailCraft models, then the rest it kept.
fn header_lines(m: &Message, offset: i32) -> String {
    let mut out = String::new();
    let mut line = |name: &str, value: &str| {
        if !value.is_empty() {
            out.push_str(name);
            out.push_str(": ");
            out.push_str(value);
            out.push('\n');
        }
    };
    line("From", &m.from.full());
    line("Reply-To", &format_list(&m.reply_to));
    line("To", &format_list(&m.to));
    line("Cc", &format_list(&m.cc));
    line("Bcc", &format_list(&m.bcc));
    line("Subject", &m.subject);
    line("Date", &mailcraft_time::format_rfc5322(m.date, offset));
    line("Message-ID", &angled(&m.message_id));
    line("In-Reply-To", &angled(&m.in_reply_to));
    line("References", &m.references.iter().map(|r| angled(r)).collect::<Vec<_>>().join(" "));
    for (name, value) in &m.headers {
        line(name, value);
    }
    out
}

/// A message id in its angle brackets, as a header writes it.
fn angled(id: &str) -> String {
    let id = id.trim();
    if id.is_empty() || id.starts_with('<') { id.to_string() } else { format!("<{id}>") }
}

/// `text` with what can't be seen made visible, at most `max` bytes: line ends and tabs stay; a
/// carriage return before a line end goes; any other control character becomes its picture
/// (U+2400 on) and a format character, a line or paragraph separator, or another that shows
/// nothing becomes `<U+XXXX>`. Whether it was cut.
pub fn visible(text: &str, max: usize) -> (String, bool) {
    let mut out = String::with_capacity(text.len().min(max));
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let picture = match c {
            '\n' | '\t' => None,
            '\r' if chars.peek() == Some(&'\n') => continue,
            // The pictures of the C0 controls, and of Delete.
            '\u{0}'..='\u{1f}' => Some(char::from_u32(0x2400 + c as u32).unwrap_or('\u{fffd}').to_string()),
            '\u{7f}' => Some("\u{2421}".to_string()),
            _ if c.is_control() || shows_nothing(c) => Some(format!("<U+{:04X}>", c as u32)),
            _ => None,
        };
        let len = picture.as_ref().map_or(c.len_utf8(), String::len);
        if out.len().saturating_add(len) > max {
            return (out, true);
        }
        match picture {
            Some(p) => out.push_str(&p),
            None => out.push(c),
        }
    }
    (out, false)
}

/// Whether `c` shows nothing itself or changes how what is around it shows: a format character
/// (`mailcraft-names`), a line or paragraph separator, a variation selector, a filler.
fn shows_nothing(c: char) -> bool {
    mailcraft_names::is_format(c)
        || matches!(
            c,
            '\u{2028}' | '\u{2029}' | '\u{034f}' | '\u{115f}' | '\u{1160}' | '\u{3164}' | '\u{ffa0}' | '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}'
        )
}
