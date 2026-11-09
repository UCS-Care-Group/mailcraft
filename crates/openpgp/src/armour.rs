// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! OpenPGP armour (RFC 9580 §6.2) and the Cleartext Signature Framework (RFC 9580 §7), read
//! here rather than in rPGP so that every size is ours to cap: the lines of a block are found,
//! its base64 is decoded up to a limit, and a cleartext-signed text is given back as the bytes
//! that were signed. Linear in the input; nothing here fails on any bytes.

use crate::mime;

/// The kinds of armour block this crate reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Signature,
    PublicKey,
}

impl Kind {
    fn label(self) -> &'static [u8] {
        match self {
            Kind::Signature => b"SIGNATURE",
            Kind::PublicKey => b"PUBLIC KEY BLOCK",
        }
    }
}

/// One line of `text`: its bytes without the line break, and without the white space it ends
/// in as well.
struct Line<'a> {
    raw: &'a [u8],
    text: &'a [u8],
}

fn lines(text: &[u8]) -> impl Iterator<Item = Line<'_>> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        if at >= text.len() {
            return None;
        }
        let rest = text.get(at..).unwrap_or_default();
        let len = rest.iter().position(|b| *b == b'\n');
        let raw = rest.get(..len.unwrap_or(rest.len())).unwrap_or_default();
        at = len.map_or(text.len(), |n| at.saturating_add(n).saturating_add(1));
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        Some(Line { raw, text: trim_end(raw) })
    })
}

/// `line` without the spaces, tabs and CR it ends in.
pub(crate) fn trim_end(line: &[u8]) -> &[u8] {
    let end = line.iter().rposition(|b| !matches!(b, b' ' | b'\t' | b'\r')).map_or(0, |i| i.saturating_add(1));
    line.get(..end).unwrap_or_default()
}

fn marker(edge: &[u8], label: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(label.len().saturating_add(20));
    m.extend_from_slice(b"-----");
    m.extend_from_slice(edge);
    m.extend_from_slice(b" PGP ");
    m.extend_from_slice(label);
    m.extend_from_slice(b"-----");
    m
}

/// The decoded contents of every armour block of `kind` in `text`, at most `max_blocks`, each
/// at most `limit` bytes (a larger one is skipped). A byte-order mark before a line is allowed.
pub(crate) fn blocks(text: &[u8], kind: Kind, max_blocks: usize, limit: usize) -> Vec<Vec<u8>> {
    let begin = marker(b"BEGIN", kind.label());
    let end = marker(b"END", kind.label());
    let mut out = Vec::new();
    if max_blocks == 0 {
        return out;
    }
    let mut inside: Option<Body> = None;
    for line in lines(text) {
        let line_text = line.text.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line.text);
        match inside.as_mut() {
            None if line_text == begin.as_slice() => inside = Some(Body::new(limit)),
            None => {}
            Some(body) if line_text == end.as_slice() => {
                if let Some(bytes) = body.finish() {
                    out.push(bytes);
                    if out.len() >= max_blocks {
                        return out;
                    }
                }
                inside = None;
            }
            Some(body) if line_text == begin.as_slice() => *body = Body::new(limit),
            Some(body) => body.line(line_text),
        }
    }
    out
}

/// The base64 of one block as its lines come: armour headers until the first empty line, then
/// data until the checksum line (`=` and four characters), which RFC 9580 lets a reader ignore.
struct Body {
    in_headers: bool,
    data: Vec<u8>,
    limit: usize,
    too_large: bool,
    done: bool,
}

impl Body {
    fn new(limit: usize) -> Body {
        Body { in_headers: true, data: Vec::new(), limit, too_large: false, done: false }
    }

    fn line(&mut self, line: &[u8]) {
        if self.done || self.too_large {
            return;
        }
        if self.in_headers {
            if line.is_empty() {
                self.in_headers = false;
                return;
            }
            // Armour without headers or the empty line after them: data from the first line.
            if line.contains(&b':') {
                return;
            }
            self.in_headers = false;
        }
        if line.first() == Some(&b'=') {
            self.done = true;
            return;
        }
        // Four characters of base64 are three bytes: stop collecting well before the limit.
        if self.data.len().saturating_add(line.len()) / 4 * 3 > self.limit {
            self.too_large = true;
            return;
        }
        self.data.extend_from_slice(line);
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        if self.too_large {
            return None;
        }
        let bytes = mime::base64(&self.data);
        (!bytes.is_empty() && bytes.len() <= self.limit).then_some(bytes)
    }
}

/// A cleartext-signed text (RFC 9580 §7): the bytes that were signed and the decoded
/// signature block after them.
#[derive(Debug)]
pub(crate) struct Cleartext {
    /// The text as it was signed: dash-escaping undone, white space at each line's end removed,
    /// lines joined by CR LF and no line break after the last one.
    pub(crate) signed: Vec<u8>,
    /// The signature packets, decoded from their armour.
    pub(crate) signature: Vec<u8>,
}

/// The first cleartext-signed text in `text`, if there is a whole one: `-----BEGIN PGP SIGNED
/// MESSAGE-----`, its `Hash:` headers and an empty line, the text, and a signature block, which
/// may be at most `limit` bytes.
pub(crate) fn cleartext(text: &[u8], limit: usize) -> Option<Cleartext> {
    let begin = marker(b"BEGIN", b"SIGNED MESSAGE");
    let sig_begin = marker(b"BEGIN", Kind::Signature.label());
    let sig_end = marker(b"END", Kind::Signature.label());
    #[derive(PartialEq)]
    enum At {
        Before,
        Headers,
        Text,
        Signature,
    }
    let mut at = At::Before;
    let mut signed: Vec<u8> = Vec::new();
    let mut first_line = true;
    let mut body = Body::new(limit);
    for line in lines(text) {
        let line_text = line.text.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line.text);
        match at {
            At::Before if line_text == begin.as_slice() => at = At::Headers,
            At::Before => {}
            // RFC 9580 §7.1: one or more `Hash:` lines (or none, in newer writers), then an
            // empty line.
            At::Headers if line_text.is_empty() => at = At::Text,
            At::Headers => {}
            At::Text if line_text == sig_begin.as_slice() => at = At::Signature,
            At::Text => {
                if !first_line {
                    signed.extend_from_slice(b"\r\n");
                }
                first_line = false;
                // Dash-escaping undone (§7.1), then the white space the line ends in removed
                // (§7.2).
                let raw = line.raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line.raw);
                signed.extend_from_slice(trim_end(raw.strip_prefix(b"- ").unwrap_or(raw)));
            }
            At::Signature if line_text == sig_end.as_slice() => {
                let signature = body.finish()?;
                return Some(Cleartext { signed, signature });
            }
            At::Signature => body.line(line_text),
        }
    }
    None
}
