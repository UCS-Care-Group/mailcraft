// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Internet messages (RFC 5322 + MIME) to [`Message`].

use std::sync::Arc;

use crate::{AUTH_AS, DIRECTIONALITY, MAX_AUTH_AS};
use crate::{Attachment, FlagStatus, Importance, Message};

use crate::header::{self, Headers, parse_params};
use crate::words::{decode_words, has_words};
use crate::{address, charset, media, transfer};

/// Deepest nesting of multiparts that is opened; what lies deeper is skipped.
const MAX_DEPTH: usize = 20;

/// Most MIME parts read from one message.
const MAX_PARTS: usize = 1000;

/// How many S/MIME opaque signed parts of one message are opened (#30): one. Signed data inside
/// signed data is a file, so that nesting can't make a message many times its size (each level
/// would be kept as a file and opened again).
const MAX_OPAQUE: usize = 1;
/// The most bytes opened from opaque signed parts in one message: past it they stay files.
const MAX_OPAQUE_BYTES: usize = 64 * 1024 * 1024;

/// Most unmodelled header fields kept in [`Message::headers`], and their total size in bytes.
const MAX_KEPT_HEADERS: usize = 200;
const MAX_KEPT_BYTES: usize = 64 * 1024;

/// Most recipients (`To`, `Cc` and `Bcc` together) and most `Reply-To` addresses kept from one message.
pub const MAX_RECIPIENTS: usize = 2000;

/// Most categories (`Keywords`) kept from one message.
pub const MAX_CATEGORIES: usize = 500;

/// Most message ids read from `References`.
const MAX_IDS: usize = 1000;

/// Parse an Internet message (an `.eml` file, or what a server hands over).
///
/// Never fails: parts that can't be read are skipped, and garbage gives a mostly empty message
/// with the bytes as its text.
///
/// Sets `message_id` (without angle brackets), `in_reply_to`, `references`, `from`, `reply_to`,
/// `to`, `cc`, `bcc`, `subject` (encoded-words decoded), `date` (from `Date`; 0 when it is
/// missing or unreadable), `body`, `attachments`, `importance`, `categories` (from
/// `Keywords`), `read` (`Status` has `R`), `flag` (`X-Status` has `F`), `answered` (`X-Status` has
/// `A`) and `headers` (every other field that isn't MIME structure, in order: as written, but for
/// the unstructured ones, whose encoded-words are decoded).
/// Leaves `id`, `account`, `folder` and `uid` at 0 and `draft`, `pinned`, `forwarded` false.
///
/// The body: the first `text/plain` part that isn't an attachment is `body.text`, the first
/// `text/html` part is `body.html`. Further text parts outside a `multipart/alternative` (a
/// mailing list's footer) are added to the end. When there is HTML and no plain text, `body.text`
/// is made from the HTML, so it is always the readable text. Line ends are `\n`.
///
/// Attachments: every part with `Content-Disposition: attachment`, every part with a file name,
/// and every part that is neither text nor multipart. A `message/rfc822` part is one attachment;
/// it is not opened. `inline` is set for `Content-Disposition: inline`, and for a part with a
/// `Content-ID` inside `multipart/related`. An S/MIME opaque signed part (`application/pkcs7-mime`
/// signed data, #30) is an attachment, and what it holds is read as if it stood in its place:
/// its text joins the body and its files the attachments.
pub fn parse(bytes: &[u8]) -> Message {
    parse_opening(bytes, MAX_OPAQUE_BYTES)
}

/// [`parse`], opening at most `opaque_limit` bytes of opaque signed parts (for the tests, which
/// have no 64 MB to spare).
pub(crate) fn parse_opening(bytes: &[u8], opaque_limit: usize) -> Message {
    let (headers, body) = header::split(bytes, true);
    let mut message = Message::default();
    read_headers(&headers, &mut message);
    let mut walk = Walk { opaque_limit, ..Walk::default() };
    walk.part(&headers, body, 0, Parent::None);
    // The HTML is read once: its text (when there is no plain part to use) and the words it hides.
    let (html_text, hidden) = walk.html.as_deref().map(mailcraft_html::text_and_hidden).unzip();
    message.hidden_text = hidden.unwrap_or_default();
    message.body.text = match (walk.text, html_text) {
        (Some(text), Some(html)) if text.trim().is_empty() => html,
        (Some(text), _) => text,
        (None, Some(html)) => html,
        (None, None) => String::new(),
    };
    message.body.html = walk.html;
    message.attachments = walk.attachments;
    message
}

fn read_headers(headers: &Headers, m: &mut Message) {
    let mut kept_bytes = 0usize;
    let mut kept_auth_as = 0usize;
    let mut kept_direction = 0usize;
    let mut complete = false;
    for field in &headers.0 {
        let name = field.name.to_ascii_lowercase();
        let value = field.value.as_str();
        match name.as_str() {
            "to" => {
                let room = MAX_RECIPIENTS.saturating_sub(m.to.len() + m.cc.len() + m.bcc.len());
                m.to.extend(address::parse_list(value).into_iter().take(room));
            }
            "cc" => {
                let room = MAX_RECIPIENTS.saturating_sub(m.to.len() + m.cc.len() + m.bcc.len());
                m.cc.extend(address::parse_list(value).into_iter().take(room));
            }
            "bcc" => {
                let room = MAX_RECIPIENTS.saturating_sub(m.to.len() + m.cc.len() + m.bcc.len());
                m.bcc.extend(address::parse_list(value).into_iter().take(room));
            }
            "reply-to" => {
                let room = MAX_RECIPIENTS.saturating_sub(m.reply_to.len());
                m.reply_to.extend(address::parse_list(value).into_iter().take(room));
            }
            "references" => {
                let room = MAX_IDS.saturating_sub(m.references.len());
                m.references.extend(message_ids(value).into_iter().take(room));
            }
            "keywords" => {
                let room = MAX_CATEGORIES.saturating_sub(m.categories.len());
                m.categories.extend(value.split(',').map(|k| decode_words(k.trim())).filter(|k| !k.is_empty()).take(room));
            }
            "status" => m.read |= value.contains('R'),
            "x-status" => {
                if value.contains('F') {
                    m.flag = FlagStatus::Flagged;
                }
                m.answered |= value.contains('A');
            }
            "x-mailcraft-flag" => complete |= value.trim().eq_ignore_ascii_case("complete"),
            // MailCraft's own note of the file a message was opened from: never a sender's.
            "x-mailcraft-opened-file-sha256" => {}
            // Nor of where in a .pst file a message was (#815).
            "x-mailcraft-pst-item" => {}
            // Read once, below.
            "date" | "from" | "subject" | "message-id" | "in-reply-to" | "importance" | "x-priority" | "mime-version" => {}
            structural if structural.starts_with("content-") => {}
            // Kept apart from the cap below, so that a sender's padding can't push Exchange's own out.
            auth_as if auth_as.eq_ignore_ascii_case(AUTH_AS) => {
                if kept_auth_as < MAX_AUTH_AS {
                    kept_auth_as += 1;
                    // Cut short by `header::split` already.
                    m.headers.push((field.name.clone(), value.to_string()));
                    m.fresh_stamps = true;
                }
            }
            // So is Exchange's word on which way the message went.
            direction if direction.eq_ignore_ascii_case(DIRECTIONALITY) => {
                if kept_direction < MAX_AUTH_AS {
                    kept_direction += 1;
                    m.headers.push((field.name.clone(), value.to_string()));
                }
            }
            _ => {
                let size = field.name.len() + value.len();
                if m.headers.len() < MAX_KEPT_HEADERS && kept_bytes + size <= MAX_KEPT_BYTES {
                    kept_bytes += size;
                    let value = if has_words(&name) { decode_words(value) } else { value.to_string() };
                    m.headers.push((field.name.clone(), value));
                }
            }
        }
    }
    if complete {
        m.flag = FlagStatus::Complete;
    }
    if let Some((timestamp, _)) = headers.get("date").and_then(mailcraft_time::parse_rfc5322) {
        m.date = timestamp;
    }
    if let Some(from) = headers.get("from").and_then(|v| address::parse_list(v).into_iter().next()) {
        m.from = from;
    }
    m.subject = headers.get("subject").map(decode_words).unwrap_or_default();
    let first_id = |name: &str| headers.get(name).and_then(|v| message_ids(v).into_iter().next()).unwrap_or_default();
    m.message_id = first_id("message-id");
    m.in_reply_to = first_id("in-reply-to");
    m.importance = headers.get("importance").and_then(Importance::parse).or_else(|| headers.get("x-priority").and_then(priority)).unwrap_or_default();
}

/// `X-Priority`: 1 and 2 are high, 4 and 5 low (`1 (Highest)` is common).
fn priority(value: &str) -> Option<Importance> {
    match value.trim().chars().next()? {
        '1' | '2' => Some(Importance::High),
        '3' => Some(Importance::Normal),
        '4' | '5' => Some(Importance::Low),
        _ => None,
    }
}

/// The message ids of a `Message-ID`, `In-Reply-To` or `References` value, without angle brackets.
fn message_ids(value: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        let after = rest.get(open + 1..).unwrap_or("");
        let Some(close) = after.find('>') else { break };
        let id = after.get(..close).unwrap_or("").trim();
        if !id.is_empty() && ids.len() < MAX_IDS {
            ids.push(id.to_string());
        }
        rest = after.get(close + 1..).unwrap_or("");
    }
    if ids.is_empty() && !value.contains('<') {
        // Some senders leave the brackets off; words without an `@` are a comment, not an id.
        ids.extend(value.split(|c: char| c.is_whitespace() || c == ',').filter(|t| t.contains('@')).take(MAX_IDS).map(str::to_string));
    }
    ids
}

/// What kind of multipart a part sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Parent {
    None,
    Alternative,
    Related,
    Other,
}

/// What a walk over the MIME tree has found.
#[derive(Default)]
struct Walk {
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<Attachment>,
    parts: usize,
    /// Opaque signed parts opened so far, and the bytes of what they held ([`MAX_OPAQUE`],
    /// [`MAX_OPAQUE_BYTES`]).
    opaque: usize,
    opaque_bytes: usize,
    /// The most bytes opened from opaque signed parts ([`MAX_OPAQUE_BYTES`] but in tests).
    opaque_limit: usize,
}

impl Walk {
    fn part(&mut self, headers: &Headers, body: &[u8], depth: usize, parent: Parent) {
        if self.parts >= MAX_PARTS {
            return;
        }
        self.parts += 1;
        let content_type = headers.get("content-type").map(parse_params).unwrap_or_default();
        let declared = Some(content_type.value.as_str()).filter(|t| is_media_type(t));
        // RFC 2045: a part that doesn't say what it is, is plain text.
        let media_type = declared.unwrap_or("text/plain");

        if let Some(subtype) = media_type.strip_prefix("multipart/") {
            if depth >= MAX_DEPTH {
                return;
            }
            let Some(boundary) = content_type.get("boundary").filter(|b| !b.is_empty()) else {
                // Nothing to split on: show what is there rather than nothing.
                self.add_text(charset::unix_lines(&charset::guess(body)), parent);
                return;
            };
            let kind = match subtype {
                "alternative" => Parent::Alternative,
                "related" => Parent::Related,
                _ => Parent::Other,
            };
            for piece in split_multipart(body, boundary, MAX_PARTS.saturating_sub(self.parts)) {
                let (part_headers, part_body) = header::split(piece, false);
                self.part(&part_headers, part_body, depth + 1, kind);
            }
            return;
        }

        let disposition = headers.get("content-disposition").map(parse_params).unwrap_or_default();
        let file_name = disposition.text("filename").or_else(|| content_type.text("name")).filter(|n| !n.is_empty());
        let attached = disposition.value == "attachment" || file_name.is_some();
        let encoding = headers.get("content-transfer-encoding").map(|e| e.trim().to_ascii_lowercase()).unwrap_or_default();

        if !attached && (media_type == "text/plain" || media_type == "text/html") {
            // The line end that closes a message is not part of its text.
            let raw = if depth == 0 { body.strip_suffix(b"\r\n").or_else(|| body.strip_suffix(b"\n")).unwrap_or(body) } else { body };
            let text = charset::decode(&decode(raw, &encoding), content_type.get("charset").unwrap_or(""));
            let text = charset::unix_lines(text.trim_start_matches('\u{feff}'));
            if media_type == "text/html" {
                self.add_html(text, parent)
            } else {
                self.add_text(text, parent)
            }
            return;
        }
        // Other text alternatives (AMP, enriched, watch) are more renderings of the body, not files.
        if !attached && parent == Parent::Alternative && media_type.starts_with("text/") && media_type != "text/calendar" {
            return;
        }

        let content_id =
            headers.get("content-id").map(|v| v.trim().trim_start_matches('<').trim_end_matches('>').trim().to_string()).unwrap_or_default();
        let mime = match declared {
            Some(t) => t.to_string(),
            None => media::media_type_for(file_name.as_deref().unwrap_or("")).to_string(),
        };
        let name = file_name.unwrap_or_else(|| media::default_name(&mime));
        let inline = disposition.value == "inline" || (!content_id.is_empty() && parent == Parent::Related);
        let data = Arc::new(decode(body, &encoding));
        // S/MIME's opaque signed data (#30): the message is what it holds, read in its place after
        // the file itself. Not one sent along as a file inside another message, and nothing is
        // checked here. The file is kept beside what it holds (about twice the room): it is the
        // Digital signature's own file, which Details opens and saves and an agent is pointed to
        // (`signaturePart`), and a message read without the bytes it came as (past the cap of
        // those) has nothing else to check its signature from; the window and `mail.get` leave it
        // out of the files.
        let pkcs7 = matches!(mime.as_str(), "application/pkcs7-mime" | "application/x-pkcs7-mime");
        let openable = pkcs7
            && (depth == 0 || disposition.value != "attachment")
            && depth < MAX_DEPTH
            && self.opaque < MAX_OPAQUE
            && data.len() <= self.opaque_limit.saturating_sub(self.opaque_bytes);
        let signed = openable.then(|| mailcraft_ber::signed_content(&data)).flatten();
        if let Some(content) = &signed {
            self.opaque = self.opaque.saturating_add(1);
            self.opaque_bytes = self.opaque_bytes.saturating_add(content.len());
        }
        self.attachments.push(Attachment { name, mime, content_id, inline, data });
        if let Some(content) = signed {
            let (inner_headers, inner_body) = header::split(&content, false);
            self.part(&inner_headers, inner_body, depth.saturating_add(1), parent);
        }
    }

    fn add_text(&mut self, text: String, parent: Parent) {
        match &mut self.text {
            None => self.text = Some(text),
            // Another rendering of the same body.
            Some(_) if parent == Parent::Alternative => {}
            Some(_) if text.trim().is_empty() => {}
            Some(body) => {
                if let Some(html) = &mut self.html {
                    html.push_str(&mailcraft_html::from_text(&text));
                }
                if !body.is_empty() {
                    body.push_str(if body.ends_with('\n') { "\n" } else { "\n\n" });
                }
                body.push_str(&text);
            }
        }
    }

    fn add_html(&mut self, html: String, parent: Parent) {
        match &mut self.html {
            None => self.html = Some(html),
            Some(_) if parent == Parent::Alternative => {}
            Some(body) => {
                body.push('\n');
                body.push_str(&html);
            }
        }
    }
}

/// `type/subtype`, both there.
fn is_media_type(t: &str) -> bool {
    t.split_once('/').is_some_and(|(kind, sub)| !kind.is_empty() && !sub.is_empty() && !sub.contains('/'))
}

/// Undo a content transfer encoding; `7bit`, `8bit`, `binary` and anything unknown are the bytes themselves.
fn decode(body: &[u8], encoding: &str) -> Vec<u8> {
    match encoding {
        "base64" => transfer::base64_decode(body),
        "quoted-printable" => transfer::qp_decode(body),
        _ => body.to_vec(),
    }
}

/// The parts of a multipart body: what lies between `--boundary` lines, without the line break
/// that belongs to the delimiter. The text before the first delimiter and after the closing
/// `--boundary--` is ignored. A body that is cut short still gives its last part.
pub(crate) fn split_multipart<'a>(body: &'a [u8], boundary: &str, max: usize) -> Vec<&'a [u8]> {
    let delimiter = [b"--", boundary.as_bytes()].concat();
    let mut parts = Vec::new();
    let mut start: Option<usize> = None;
    let mut pos = 0;
    while pos < body.len() {
        let (line, next) = header::line_at(body, pos);
        if let Some(rest) = line.strip_prefix(delimiter.as_slice()) {
            let (closing, tail) = match rest.strip_prefix(b"--") {
                Some(tail) => (true, tail),
                None => (false, rest),
            };
            if tail.iter().all(|b| matches!(b, b' ' | b'\t')) {
                if let Some(from) = start.take()
                    && parts.len() < max
                {
                    let piece = body.get(from..pos).unwrap_or_default();
                    parts.push(piece.strip_suffix(b"\r\n").or_else(|| piece.strip_suffix(b"\n")).unwrap_or(piece));
                }
                if closing || parts.len() >= max {
                    return parts;
                }
                start = Some(next);
            }
        }
        pos = next;
    }
    if let Some(from) = start
        && parts.len() < max
    {
        parts.push(body.get(from..).unwrap_or_default());
    }
    parts
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;
