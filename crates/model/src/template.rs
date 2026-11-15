// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Local reusable message templates with plain-text field expansion (#952).
//!
//! Templates are stored locally on this computer in the mailbox file and are never sent to a
//! mail server. Fields (`{recipient.name}`, `{recipient.company}`, `{date}`) are expanded in
//! one non-recursive pass as plain text only; unknown fields stay as typed, and nothing is ever
//! evaluated.

use serde::{Deserialize, Serialize};

use crate::Id;
use crate::time::Timestamp;

/// Maximum number of templates kept in a mailbox.
pub const MAX_TEMPLATES: usize = 500;
/// Maximum combined byte size of all templates kept in a mailbox (4 MiB across name, subject, and
/// body). Bounds both memory and undo-history snapshots when many large templates exist.
pub const MAX_TEMPLATES_TOTAL_BYTES: usize = 4 * 1024 * 1024;
/// Maximum length of a template's name, in characters.
pub const MAX_TEMPLATE_NAME: usize = 120;
/// Maximum length of a template's subject, in characters.
pub const MAX_TEMPLATE_SUBJECT: usize = 500;
/// Maximum length of a template's body, in characters.
pub const MAX_TEMPLATE_BODY: usize = 50_000;
/// Maximum length of a template's name, in UTF-8 bytes.
pub const MAX_TEMPLATE_NAME_BYTES: usize = 480;
/// Maximum length of a template's subject, in UTF-8 bytes.
pub const MAX_TEMPLATE_SUBJECT_BYTES: usize = 2_000;
/// Maximum length of a template's body, in UTF-8 bytes.
pub const MAX_TEMPLATE_BODY_BYTES: usize = 200_000;
/// Maximum byte length of a `{field}` name considered during expansion.
const MAX_FIELD_NAME_BYTES: usize = 64;
/// Maximum output length of an expanded template body, in characters.
const MAX_EXPANDED_CHARS: usize = crate::rich::MAX_CHARS;
/// Maximum output length of an expanded template body, in UTF-8 bytes.
const MAX_EXPANDED_BYTES: usize = 1 << 20;

/// Values used to fill fields when inserting a template.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateFields {
    pub recipient_name: String,
    pub recipient_company: String,
    pub date: String,
}

/// A reusable local message template.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Template {
    /// Unique identifier of this template.
    pub id: Id,
    /// Display name for the template.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Optional subject line filled in when the message's subject is empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub subject: String,
    /// Plain-text body of the template, which may contain `{recipient.name}`, `{recipient.company}`, or `{date}`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// When this template was created.
    #[serde(skip_serializing_if = "is_zero_timestamp")]
    pub created: Timestamp,
}

fn is_zero_timestamp(t: &Timestamp) -> bool {
    *t == 0
}

/// Truncate `s` on a UTF-8 character boundary so it stays within `max_chars` and `max_bytes`.
fn truncate_at_boundary(s: &str, max_chars: usize, max_bytes: usize) -> &str {
    let mut end = s.len().min(max_bytes);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let byte_capped = s.get(..end).unwrap_or("");
    match byte_capped.char_indices().nth(max_chars) {
        Some((idx, _)) => byte_capped.get(..idx).unwrap_or(byte_capped),
        None => byte_capped,
    }
}

/// Clean single-line text: strip control and invisible format characters, normalize whitespace to
/// single spaces, trim, and cap at `max_chars` and `max_bytes` on a char boundary.
fn clean_single_line(s: &str, max_chars: usize, max_bytes: usize) -> String {
    let cleaned = mailcraft_names::incoming(s, max_chars).unwrap_or_default();
    truncate_at_boundary(&cleaned, max_chars, max_bytes).trim().to_string()
}

/// Clean multiline template body text: normalize CRLF/CR to LF, strip control characters except
/// `\n` and `\t` (and strip invisible format characters), and cap at `max_chars` and `max_bytes` on
/// a char boundary.
fn clean_body(s: &str, max_chars: usize, max_bytes: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_bytes));
    let mut chars_count = 0usize;
    let mut prev_cr = false;
    for c in s.chars() {
        if chars_count >= max_chars {
            break;
        }
        let ch = match c {
            '\r' => {
                prev_cr = true;
                '\n'
            }
            '\n' if prev_cr => {
                prev_cr = false;
                continue;
            }
            '\n' | '\t' => {
                prev_cr = false;
                c
            }
            c if c.is_control() || mailcraft_names::is_format(c) => {
                prev_cr = false;
                continue;
            }
            c => {
                prev_cr = false;
                c
            }
        };
        if out.len().saturating_add(ch.len_utf8()) > max_bytes {
            break;
        }
        out.push(ch);
        chars_count += 1;
    }
    out
}

impl Template {
    /// Create a new template, sanitizing its name, subject, and body to length caps and stripping
    /// control characters. If `name` is empty, falls back to a snippet of `subject` or `body`.
    pub fn new(id: Id, name: &str, subject: &str, body: &str, created: Timestamp) -> Self {
        let mut t = Template { id, name: name.to_string(), subject: subject.to_string(), body: body.to_string(), created };
        t.sanitize();
        t
    }

    /// Re-apply sanitization and length bounds (e.g. after reading a mailbox file).
    pub fn sanitize(&mut self) {
        self.name = clean_single_line(&self.name, MAX_TEMPLATE_NAME, MAX_TEMPLATE_NAME_BYTES);
        self.subject = clean_single_line(&self.subject, MAX_TEMPLATE_SUBJECT, MAX_TEMPLATE_SUBJECT_BYTES);
        self.body = clean_body(&self.body, MAX_TEMPLATE_BODY, MAX_TEMPLATE_BODY_BYTES);
        if self.name.is_empty() {
            if !self.subject.is_empty() {
                self.name = clean_single_line(&self.subject, 40, MAX_TEMPLATE_NAME_BYTES);
            } else {
                let first_line = self.body.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                self.name = clean_single_line(first_line, 40, MAX_TEMPLATE_NAME_BYTES);
            }
        }
    }

    /// Expand the template's subject and body in one non-evaluating pass using `fields`.
    pub fn expand(&self, fields: &TemplateFields) -> (String, String) {
        let subject_raw = expand_text_capped(&self.subject, fields, MAX_TEMPLATE_SUBJECT, MAX_TEMPLATE_SUBJECT_BYTES);
        let subject = clean_single_line(&subject_raw, MAX_TEMPLATE_SUBJECT, MAX_TEMPLATE_SUBJECT_BYTES);
        let body = expand_text_capped(&self.body, fields, MAX_EXPANDED_CHARS, MAX_EXPANDED_BYTES);
        (subject, body)
    }

    /// Total byte size of this template's text fields (`name`, `subject`, and `body`).
    pub fn byte_size(&self) -> usize {
        self.name.len().saturating_add(self.subject.len()).saturating_add(self.body.len())
    }
}

/// Expand `{recipient.name}`, `{recipient.company}`, and `{date}` in `template` as plain text in
/// one pass. Unknown fields are left as typed, and substituted text is never re-scanned.
pub fn expand_text(template: &str, fields: &TemplateFields) -> String {
    expand_text_capped(template, fields, MAX_EXPANDED_CHARS, MAX_EXPANDED_BYTES)
}

fn expand_text_capped(template: &str, fields: &TemplateFields, max_chars: usize, max_bytes: usize) -> String {
    let recipient_name = clean_single_line(&fields.recipient_name, MAX_TEMPLATE_SUBJECT, MAX_TEMPLATE_SUBJECT_BYTES);
    let recipient_company = clean_single_line(&fields.recipient_company, MAX_TEMPLATE_SUBJECT, MAX_TEMPLATE_SUBJECT_BYTES);
    let date = clean_single_line(&fields.date, MAX_TEMPLATE_NAME, MAX_TEMPLATE_NAME_BYTES);

    let mut out = String::with_capacity(template.len().min(max_bytes));
    let mut out_chars = 0usize;
    let mut push_bounded = |s: &str| -> bool {
        for c in s.chars() {
            if out_chars >= max_chars || out.len().saturating_add(c.len_utf8()) > max_bytes {
                return false;
            }
            out.push(c);
            out_chars += 1;
        }
        true
    };

    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let prefix = rest.get(..open).unwrap_or("");
        if !push_bounded(prefix) {
            return out;
        }
        let after_open = rest.get(open + 1..).unwrap_or("");
        // Look ahead for a closing '}' before another '{' or newline, within MAX_FIELD_NAME_BYTES.
        let search_limit = after_open.len().min(MAX_FIELD_NAME_BYTES + 1);
        let mut bound = search_limit;
        while bound > 0 && !after_open.is_char_boundary(bound) {
            bound -= 1;
        }
        let window = after_open.get(..bound).unwrap_or("");
        let mut close_at = None;
        for (idx, c) in window.char_indices() {
            match c {
                '}' => {
                    close_at = Some(idx);
                    break;
                }
                '{' | '\n' | '\r' => break,
                _ => {}
            }
        }
        if let Some(close) = close_at {
            let key = window.get(..close).unwrap_or("");
            let value = match key {
                "recipient.name" => Some(recipient_name.as_str()),
                "recipient.company" => Some(recipient_company.as_str()),
                "date" => Some(date.as_str()),
                _ => None,
            };
            match value {
                Some(v) => {
                    if !push_bounded(v) {
                        return out;
                    }
                }
                None => {
                    if !push_bounded("{") || !push_bounded(key) || !push_bounded("}") {
                        return out;
                    }
                }
            }
            rest = after_open.get(close + 1..).unwrap_or("");
        } else {
            if !push_bounded("{") {
                return out;
            }
            rest = after_open;
        }
    }
    let _ = push_bounded(rest);
    out
}
