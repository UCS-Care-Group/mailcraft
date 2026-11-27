// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Internet messages (RFC 5322 + MIME) to [`Message`].

use std::collections::{HashMap, HashSet};
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

/// How many TNEF streams (`winmail.dat`) of one message are opened: one. One inside another
/// (an attached message) is never opened, and nor is a second beside the first.
const MAX_TNEF: usize = 1;
/// The biggest TNEF stream opened: past it, it stays a file.
const MAX_TNEF_BYTES: usize = 64 * 1024 * 1024;

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
/// its text joins the body and its files the attachments. A TNEF stream (`winmail.dat`,
/// `application/ms-tnef`, [MS-OXTNEF]) stays an attachment, and the files it holds are added
/// before it; its text is the body only when the message has none of its own, as the MIME
/// around it is to be believed before what is inside it (2.1.3.5.2). Its other contents (its
/// rich text, an attached message, an OLE object) are not opened: they stay in the stream.
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
    walk.finish_mixed_pictures();
    // The HTML is read once: its text (when there is no plain part to use) and the words it hides.
    let (html_text, hidden) = walk.body.html.as_deref().map(mailcraft_html::text_and_hidden).unzip();
    message.hidden_text = hidden.unwrap_or_default();
    // A winmail.dat's text, for a message with no text or HTML of its own.
    if walk.body.html.is_none()
        && walk.body.text.as_deref().is_none_or(|t| t.trim().is_empty())
        && let Some(text) = walk.tnef_text.take()
    {
        walk.body.text = Some(text);
    }
    message.body.text = match (walk.body.text, html_text) {
        (Some(text), Some(html)) if text.trim().is_empty() => html,
        (Some(text), _) => text,
        (None, Some(html)) => html,
        (None, None) => String::new(),
    };
    message.body.html = walk.body.html;
    message.attachments = walk.attachments;
    message
}

fn read_headers(headers: &Headers, m: &mut Message) {
    // Fields left out as the header was split, past its caps: what they said can't be told (#566).
    m.headers_cut |= headers.1;
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
                let list = address::parse_list(value);
                m.headers_cut |= list.len() > room;
                m.to.extend(list.into_iter().take(room));
            }
            "cc" => {
                let room = MAX_RECIPIENTS.saturating_sub(m.to.len() + m.cc.len() + m.bcc.len());
                let list = address::parse_list(value);
                m.headers_cut |= list.len() > room;
                m.cc.extend(list.into_iter().take(room));
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
                let mut words = value.split(',').map(|k| decode_words(k.trim())).filter(|k| !k.is_empty());
                m.categories.extend(words.by_ref().take(room));
                m.headers_cut |= words.next().is_some();
            }
            "status" => {
                m.read |= value.contains('R');
            }
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
            "from" | "subject" | "message-id" | "in-reply-to" | "date" | "importance" | "x-priority" | "mime-version" => {}
            structural if structural.starts_with("content-") => {}
            // Kept apart from the cap below, so that a sender's padding can't push Exchange's own out.
            auth_as if auth_as.eq_ignore_ascii_case(AUTH_AS) => {
                if kept_auth_as < MAX_AUTH_AS {
                    kept_auth_as += 1;
                    // Cut short by `header::split` already.
                    m.headers.push((field.name.clone(), value.to_string()));
                    m.fresh_stamps = true;
                } else {
                    m.headers_cut = true;
                }
            }
            // So is Exchange's word on which way the message went.
            direction if direction.eq_ignore_ascii_case(DIRECTIONALITY) => {
                if kept_direction < MAX_AUTH_AS {
                    kept_direction += 1;
                    m.headers.push((field.name.clone(), value.to_string()));
                } else {
                    m.headers_cut = true;
                }
            }
            _ => {
                let size = field.name.len() + value.len();
                if m.headers.len() < MAX_KEPT_HEADERS && kept_bytes + size <= MAX_KEPT_BYTES {
                    kept_bytes += size;
                    let value = if has_words(&name) { decode_words(value) } else { value.to_string() };
                    m.headers.push((field.name.clone(), value));
                } else {
                    // Left out: what it says can't be told from the message (#566).
                    m.headers_cut = true;
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
    Mixed,
    Other,
}

/// What a walk over the MIME tree has found.
#[derive(Default)]
struct Walk {
    body: BodyParts,
    attachments: Vec<Attachment>,
    parts: usize,
    /// Opaque signed parts opened so far, and the bytes of what they held ([`MAX_OPAQUE`],
    /// [`MAX_OPAQUE_BYTES`]).
    opaque: usize,
    opaque_bytes: usize,
    /// The most bytes opened from opaque signed parts ([`MAX_OPAQUE_BYTES`] but in tests).
    opaque_limit: usize,
    /// TNEF streams opened so far ([`MAX_TNEF`]), and the text of the first that had one.
    tnef: usize,
    tnef_text: Option<String>,
}

/// A single MIME rendering, collected independently before alternative selection.
#[derive(Default)]
struct BodyParts {
    text: Option<String>,
    html: Option<String>,
    source_html: bool,
    /// HTML insertion offset, absolute attachment index, and candidate id.
    mixed_pictures: Vec<(usize, usize, String)>,
}

impl Walk {
    fn finish_mixed_pictures(&mut self) {
        if self.body.mixed_pictures.is_empty() {
            return;
        }
        let Some(html) = self.body.html.take() else { return };
        // Inspect the finished HTML once, including references following the MIME
        // picture. Insert the remaining pictures in a single pass over the body.
        let blocks = mailcraft_html::parse(&html);
        let images = mailcraft_html::images(&blocks);
        if images.len() > 4096 {
            self.body.html = Some(html);
            return;
        }
        // Later explicit ids must also be reserved before any missing id is assigned.
        let mut reserved: HashSet<Vec<u8>> =
            self.attachments.iter().filter(|part| !part.content_id.is_empty()).map(|part| cid_raw_key(&part.content_id)).collect();
        for (_, index, generated_id) in &self.body.mixed_pictures {
            let Some(part) = self.attachments.get_mut(*index) else { continue };
            if part.content_id.is_empty() {
                let mut id = generated_id.clone();
                while !reserved.insert(cid_raw_key(&id)) {
                    id.insert(0, 'x');
                }
                part.content_id = id;
            }
        }
        // Match the model reader's PartIndex precedence: exact raw, exact decoded,
        // then the earliest case-insensitive raw-or-decoded match. One img shows one part.
        let mut exact = HashMap::new();
        let mut folded = HashMap::new();
        for (index, part) in self.attachments.iter().enumerate().filter(|(_, part)| !part.content_id.is_empty()) {
            exact.entry(part.content_id.as_str()).or_insert(index);
            folded.entry(cid_raw_key(&part.content_id)).or_insert(index);
        }
        let mut shown: HashSet<usize> = images
            .iter()
            .filter_map(|block| {
                let mailcraft_html::BlockKind::Image { src, .. } = &block.kind else { return None };
                cid_attachment_index(src, &exact, &folded)
            })
            .collect();
        let mut combined = String::new();
        let mut cursor = 0;
        for (at, index, _) in &self.body.mixed_pictures {
            let Some(before) = html.get(cursor..*at) else { continue };
            combined.push_str(before);
            cursor = *at;
            let Some(part) = self.attachments.get(*index) else { continue };
            let identity = exact.get(part.content_id.as_str()).copied().unwrap_or(*index);
            if shown.insert(identity) {
                let source = format!("cid:{}", part.content_id);
                combined.push_str(&format!("<img src=\"{}\" alt=\"{}\">", mailcraft_html::escape(&source), mailcraft_html::escape(&part.name)));
            }
        }
        combined.push_str(html.get(cursor..).unwrap_or_default());
        self.body.html = Some(combined);
    }

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
                "mixed" => Parent::Mixed,
                _ => Parent::Other,
            };
            if kind == Parent::Alternative {
                self.alternative_parts(body, boundary, depth, parent);
            } else {
                for piece in split_multipart(body, boundary, MAX_PARTS.saturating_sub(self.parts)) {
                    let (part_headers, part_body) = header::split(piece, false);
                    self.part(&part_headers, part_body, depth + 1, kind);
                }
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
                self.add_html(text)
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
        // RFC 2046's mixed parts are displayed in order. Apple Mail also uses this
        // form for inline pictures between text parts, without an HTML img element.
        // Only an explicitly inline, locally carried picture becomes body content.
        if parent == Parent::Mixed && disposition.value == "inline" && supported_picture(&data) {
            let body = self.body.html.get_or_insert_with(|| self.body.text.as_deref().map(mailcraft_html::from_text).unwrap_or_default());
            self.body.mixed_pictures.push((body.len(), self.attachments.len(), format!("mailcraft-mixed-{}@inline.invalid", self.parts)));
        }
        // A TNEF stream (`winmail.dat`): the files Outlook wrapped in it come first, then the
        // stream itself, which keeps what of it isn't read here.
        let tnef_type = matches!(mime.as_str(), "application/ms-tnef" | "application/vnd.ms-tnef") || name.eq_ignore_ascii_case("winmail.dat");
        // Only a stream that reads takes the one place: a broken one before it (a version that
        // can't be read) doesn't keep a good one shut.
        if tnef_type && self.tnef < MAX_TNEF && data.len() <= MAX_TNEF_BYTES && mailcraft_tnef::is_tnef(&data) && self.open_tnef(&data) {
            self.tnef = self.tnef.saturating_add(1);
        }
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

    /// The files of a TNEF stream, as attachments, and its text for later. False when it can't
    /// be read at all.
    ///
    /// Its files don't count against [`MAX_PARTS`], which bounds the MIME parts read: they are
    /// bounded by the reader's own cap (`Limits::max_files`, of the one stream opened), so a
    /// stream's files never keep the message's own later parts from being read.
    fn open_tnef(&mut self, data: &[u8]) -> bool {
        let Ok(found) = mailcraft_tnef::read(data, &mailcraft_tnef::Limits::default()) else { return false };
        for file in found.files {
            let name = file.name.unwrap_or_default();
            let mime = match file.mime_type.as_deref().map(media::safe_media_type) {
                Some(t) if t != "application/octet-stream" => t,
                _ => media::media_type_for(&name).to_string(),
            };
            let name = if name.is_empty() { media::default_name(&mime) } else { name };
            // No content id: only the stream's own rich text refers to one, and it isn't shown.
            // Carried, it could clash with a part of the message's own (a `cid:` picture goes to
            // the first attachment with its id, and the stream's files stand before what follows
            // it), so the stream could put its file in the place of the message's picture.
            self.attachments.push(Attachment { name, mime, content_id: String::new(), inline: false, data: Arc::new(file.data) });
        }
        if self.tnef_text.is_none() {
            self.tnef_text = found.body.map(|t| charset::unix_lines(&t));
        }
        true
    }

    /// Parse candidates with independent body state; only files and hostile-input caps are shared.
    fn alternative_parts(&mut self, body: &[u8], boundary: &str, depth: usize, parent: Parent) {
        let outer = std::mem::take(&mut self.body);
        let mut plain = None;
        let mut preferred = None;
        let mut fallback = None;
        for piece in split_multipart(body, boundary, MAX_PARTS.saturating_sub(self.parts)) {
            let (headers, body) = header::split(piece, false);
            self.part(&headers, body, depth + 1, Parent::Alternative);
            let mut candidate = std::mem::take(&mut self.body);
            if plain.is_none() {
                plain = candidate.text.take();
            }
            if candidate.source_html && preferred.is_none() {
                preferred = Some(candidate);
                fallback = None;
            } else if preferred.is_none() && candidate.html.is_some() && fallback.is_none() {
                fallback = Some(candidate);
            }
        }
        self.body = outer;
        if let Some(mut selected) = preferred.or(fallback) {
            let prefix = if self.body.html.is_none() { self.body.text.as_deref().map(mailcraft_html::from_text) } else { None };
            // Plain and HTML are distinct renderings. Merge the plain version without
            // adding it again to the HTML that already carries its selected rendering.
            let html = self.body.html.take();
            if let Some(text) = plain {
                self.add_text(text, parent);
            }
            self.body.html = html;
            if let Some(html) = selected.html.take() {
                // Prefix text belongs to an earlier mixed part, outside this alternative.
                if self.body.html.is_none() {
                    self.body.html = prefix;
                }
                let offset = self.body.html.as_ref().map_or(0, |body| body.len().saturating_add(1));
                let source_html = self.body.source_html || selected.source_html;
                self.add_html(html);
                self.body.source_html = source_html;
                self.body.mixed_pictures.extend(selected.mixed_pictures.into_iter().map(|(at, index, id)| (offset.saturating_add(at), index, id)));
            }
        } else if let Some(text) = plain {
            self.add_text(text, parent);
        }
    }

    fn add_text(&mut self, text: String, parent: Parent) {
        match &mut self.body.text {
            None => {
                if parent == Parent::Mixed
                    && let Some(html) = &mut self.body.html
                {
                    html.push_str(&mailcraft_html::from_text(&text));
                }
                self.body.text = Some(text);
            }
            // Another rendering of the same body.
            Some(_) if parent == Parent::Alternative => {}
            Some(_) if text.trim().is_empty() => {}
            Some(body) => {
                if let Some(html) = &mut self.body.html {
                    html.push_str(&mailcraft_html::from_text(&text));
                }
                if !body.is_empty() {
                    body.push_str(if body.ends_with('\n') { "\n" } else { "\n\n" });
                }
                body.push_str(&text);
            }
        }
    }

    fn add_html(&mut self, html: String) {
        self.body.source_html = true;
        match &mut self.body.html {
            None => self.body.html = Some(html),
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

fn supported_picture(data: &[u8]) -> bool {
    data.starts_with(b"\x89PNG\r\n\x1a\n")
        || data.starts_with(&[0xff, 0xd8, 0xff])
        || data.starts_with(b"GIF87a")
        || data.starts_with(b"GIF89a")
        || (data.get(..4) == Some(b"RIFF".as_slice()) && data.get(8..12) == Some(b"WEBP".as_slice()))
}

fn cid_attachment_index(src: &str, exact: &HashMap<&str, usize>, folded: &HashMap<Vec<u8>, usize>) -> Option<usize> {
    let src = src.trim();
    src.get(..4).filter(|scheme| scheme.eq_ignore_ascii_case("cid:"))?;
    let raw = src.get(4..)?.trim().trim_start_matches('<').trim_end_matches('>');
    if raw.is_empty() || raw.len() > 998 {
        return None;
    }
    let decoded = cid_decoded(raw);
    exact
        .get(raw)
        .or_else(|| exact.get(decoded.as_str()))
        .copied()
        .or_else(|| [folded.get(&cid_raw_key(raw)), folded.get(&cid_raw_key(&decoded))].into_iter().flatten().min().copied())
}

fn cid_raw_key(value: &str) -> Vec<u8> {
    value.trim().trim_matches(['<', '>']).bytes().map(|byte| byte.to_ascii_lowercase()).collect()
}

fn cid_decoded(value: &str) -> String {
    let value = value.trim().trim_matches(['<', '>']);
    let mut decoded = Vec::with_capacity(value.len());
    let mut bytes = value.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let mut pair = bytes.clone();
            if let (Some(hi), Some(lo)) = (pair.next(), pair.next())
                && let (Some(hi), Some(lo)) = (char::from(hi).to_digit(16), char::from(lo).to_digit(16))
            {
                decoded.push((hi * 16 + lo) as u8);
                bytes = pair;
                continue;
            }
        }
        decoded.push(byte);
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_string())
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
