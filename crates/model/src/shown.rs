// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Which of a message's parts its HTML shows in place, decided in one way for every use of it
//! (AU3-15): the attachment list and its filters ([`crate::Message::listed_attachments`]), the
//! pictures a reply carries with its quote, the parts brought in with a body for a reply, and a
//! `.pst` file's inline pictures.
//!
//! A part is shown in place only when an `<img>` of the HTML asks for it by its `Content-ID`
//! (`src="cid:…"`, [`shown_ids`]) and it is a picture the reading pane draws, a PNG, JPEG, GIF or
//! WebP by its own bytes ([`is_picture`]). A `cid:` written anywhere else (in text, a comment, a
//! link) shows nothing, and a file of any other kind is listed whatever the HTML says of it.
//!
//! The HTML is the sender's: it is read once, to its end, with caps on the tags looked at and
//! the ids kept.

use std::collections::{HashMap, HashSet};

use crate::Attachment;

/// Most content ids kept from one body.
pub const MAX_SHOWN_IDS: usize = 4_096;
/// Most tags looked at in one body.
const MAX_TAGS: usize = 500_000;
/// Longest content id read.
const MAX_ID_BYTES: usize = 998;

/// The content ids the `<img>` elements of an HTML body ask for, each as written (without `cid:`
/// and any angle brackets) and with `%xx` decoded, in lower case.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShownIds(HashSet<String>);

impl ShownIds {
    /// Whether `content_id` is one of them, without regard to letter case (as [`part_for`] finds
    /// the part a `cid:` address names).
    pub fn shows(&self, content_id: &str) -> bool {
        !content_id.is_empty() && self.0.contains(&content_id.to_ascii_lowercase())
    }

    /// Whether any `<img>` asks for a part by `cid:`.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Whether `data` is a picture the reading pane draws, by its own bytes: a PNG, JPEG, GIF or WebP.
pub fn is_picture(data: &[u8]) -> bool {
    picture_kind(data).is_some()
}

/// The type of the picture `data` is, by its own bytes ([`is_picture`]).
pub fn picture_kind(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.get(..4) == Some(b"RIFF".as_slice()) && data.get(8..12) == Some(b"WEBP".as_slice()) {
        Some("image/webp")
    } else {
        None
    }
}

/// Whether `a` is a picture: by its bytes when they are in memory, else by its declared type
/// (one kept outside memory was read whole when it came, and is told by what it was said to be).
pub fn is_picture_part(a: &Attachment) -> bool {
    if a.is_loaded() {
        return is_picture(&a.data);
    }
    let mime = a.mime.trim().to_ascii_lowercase();
    matches!(mime.as_str(), "image/png" | "image/jpeg" | "image/jpg" | "image/pjpeg" | "image/gif" | "image/webp")
}

/// Whether the HTML whose ids are `ids` shows `a` in place: an `<img>` asks for its `Content-ID`
/// and it is a picture ([`is_picture_part`]).
pub fn shown_in_place(ids: &ShownIds, a: &Attachment) -> bool {
    !a.content_id.is_empty() && is_picture_part(a) && ids.shows(&a.content_id)
}

/// The part a picture's address `src` names (`cid:…`, any letter case, angle brackets and
/// `%xx` escapes allowed), with its index: the first whose `Content-ID` is the id as written,
/// then as decoded, then either without regard to letter case. The one rule the reading pane,
/// the phone and the attachment list find a part by; [`PartIndex`] for many addresses.
pub fn part_for<'a>(attachments: &'a [Attachment], src: &str) -> Option<(usize, &'a Attachment)> {
    PartIndex::new(attachments).find(src)
}

/// The parts of a message by `Content-ID`, for finding many pictures' parts ([`part_for`]) in
/// one pass over the parts and one look-up per picture, however many of each a message has.
pub struct PartIndex<'a> {
    attachments: &'a [Attachment],
    /// The first part of each `Content-ID`, as written.
    exact: HashMap<&'a str, usize>,
    /// The first part of each `Content-ID` in lower case.
    folded: HashMap<String, usize>,
}

impl<'a> PartIndex<'a> {
    pub fn new(attachments: &'a [Attachment]) -> Self {
        let mut exact = HashMap::new();
        let mut folded = HashMap::new();
        for (i, a) in attachments.iter().enumerate().filter(|(_, a)| !a.content_id.is_empty()) {
            exact.entry(a.content_id.as_str()).or_insert(i);
            folded.entry(a.content_id.to_ascii_lowercase()).or_insert(i);
        }
        PartIndex { attachments, exact, folded }
    }

    /// The part `src` names, with its index ([`part_for`]).
    pub fn find(&self, src: &str) -> Option<(usize, &'a Attachment)> {
        if self.exact.is_empty() {
            return None;
        }
        let id = content_id(src.as_bytes())?;
        let decoded = percent_decode(&id);
        let at = self.exact.get(id.as_str()).or_else(|| self.exact.get(decoded.as_str())).copied().or_else(|| {
            let either = [self.folded.get(&id.to_ascii_lowercase()), self.folded.get(&decoded.to_ascii_lowercase())];
            either.into_iter().flatten().min().copied()
        })?;
        self.attachments.get(at).map(|a| (at, a))
    }
}

/// The content ids the `<img>` elements of `html` ask for (`src="cid:…"`), at most
/// [`MAX_SHOWN_IDS`]. Comments are passed over; only the first `src` of an element counts.
pub fn shown_ids(html: &str) -> ShownIds {
    let mut out = HashSet::new();
    let bytes = html.as_bytes();
    let mut at = 0usize;
    let mut tags = 0usize;
    while let Some(found) = bytes.get(at..).and_then(|rest| rest.iter().position(|b| *b == b'<')) {
        let start = at.saturating_add(found);
        tags = tags.saturating_add(1);
        if tags > MAX_TAGS || out.len() >= MAX_SHOWN_IDS {
            break;
        }
        let rest = bytes.get(start..).unwrap_or_default();
        if rest.starts_with(b"<!--") {
            match rest.get(4..).and_then(|r| r.windows(3).position(|w| w == b"-->")) {
                Some(end) => at = start.saturating_add(4).saturating_add(end).saturating_add(3),
                None => break,
            }
            continue;
        }
        let name_len = rest.get(1..).unwrap_or_default().iter().take_while(|b| b.is_ascii_alphanumeric()).count();
        let name = rest.get(1..1usize.saturating_add(name_len)).unwrap_or_default();
        if !name.eq_ignore_ascii_case(b"img") {
            at = start.saturating_add(1);
            continue;
        }
        let (src, end) = first_src(rest.get(1usize.saturating_add(name_len)..).unwrap_or_default());
        at = start.saturating_add(1).saturating_add(name_len).saturating_add(end);
        let Some(id) = src.and_then(content_id) else { continue };
        let decoded = percent_decode(&id);
        if decoded != id && out.len() < MAX_SHOWN_IDS {
            out.insert(decoded.to_ascii_lowercase());
        }
        if out.len() < MAX_SHOWN_IDS {
            out.insert(id.to_ascii_lowercase());
        }
    }
    ShownIds(out)
}

/// The value of the first `src` attribute in the attributes `rest` of a tag, and how far the tag
/// went (past its `>`, or to the end).
fn first_src(rest: &[u8]) -> (Option<&[u8]>, usize) {
    let mut i = 0usize;
    let mut src = None;
    loop {
        while rest.get(i).is_some_and(|b| b.is_ascii_whitespace() || *b == b'/') {
            i += 1;
        }
        match rest.get(i) {
            None => return (src, i),
            Some(b'>') => return (src, i + 1),
            Some(_) => {}
        }
        let name_start = i;
        while rest.get(i).is_some_and(|b| !b.is_ascii_whitespace() && !matches!(b, b'=' | b'>' | b'/')) {
            i += 1;
        }
        let name = rest.get(name_start..i).unwrap_or_default();
        while rest.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let mut value: &[u8] = &[];
        if rest.get(i) == Some(&b'=') {
            i += 1;
            while rest.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            match rest.get(i) {
                Some(&quote @ (b'"' | b'\'')) => {
                    let begin = i + 1;
                    let len = rest.get(begin..).unwrap_or_default().iter().position(|b| *b == quote);
                    let end = len.map_or(rest.len(), |len| begin + len);
                    value = rest.get(begin..end).unwrap_or_default();
                    i = end.saturating_add(1).min(rest.len());
                }
                _ => {
                    let begin = i;
                    while rest.get(i).is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>') {
                        i += 1;
                    }
                    value = rest.get(begin..i).unwrap_or_default();
                }
            }
        } else if name.is_empty() {
            // Nothing that makes a name: step over it.
            i += 1;
        }
        if src.is_none() && name.eq_ignore_ascii_case(b"src") {
            src = Some(value);
        }
    }
}

/// The content id a `src` value names, when it is a `cid:` address.
fn content_id(src: &[u8]) -> Option<String> {
    let src = src.trim_ascii();
    let scheme = src.get(..4)?;
    if !scheme.eq_ignore_ascii_case(b"cid:") {
        return None;
    }
    let id = src.get(4..)?.trim_ascii();
    // The angle brackets as written, or as an attribute escapes them (a quote carries `&lt;…&gt;`).
    let id = id.strip_prefix(b"<").or_else(|| strip_prefix_ignore_case(id, b"&lt;")).unwrap_or(id);
    let id = id.strip_suffix(b">").or_else(|| strip_suffix_ignore_case(id, b"&gt;")).unwrap_or(id);
    if id.is_empty() || id.len() > MAX_ID_BYTES {
        return None;
    }
    std::str::from_utf8(id).ok().map(str::to_string)
}

fn strip_prefix_ignore_case<'a>(bytes: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    let (head, rest) = bytes.split_at_checked(prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then_some(rest)
}

fn strip_suffix_ignore_case<'a>(bytes: &'a [u8], suffix: &[u8]) -> Option<&'a [u8]> {
    let (rest, tail) = bytes.split_at_checked(bytes.len().checked_sub(suffix.len())?)?;
    tail.eq_ignore_ascii_case(suffix).then_some(rest)
}

/// `%40` and the like decoded; anything that isn't a valid escape, or doesn't give UTF-8, is left.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let escape = (b == b'%').then(|| text.get(i + 1..i + 3)).flatten().and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escape {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}
