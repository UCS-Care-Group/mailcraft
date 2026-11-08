// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Digitally signed and Encrypted messages, told by their own MIME (#30): `multipart/signed`
//! (RFC 1847, used by S/MIME, RFC 8551, and PGP/MIME, RFC 3156), `multipart/encrypted` (PGP/MIME),
//! `application/pkcs7-mime` (S/MIME's signed or enveloped data, also as `x-pkcs7-mime`), and a
//! plain text body that holds OpenPGP armour (RFC 9580 §6.2, "inline PGP"). Such a message keeps
//! the bytes it came as ([`mailcraft_model::Message::original`]); nothing here checks or opens
//! anything.

use mailcraft_model::{Message, Original};

use crate::header::{self, Headers, parse_params};
use crate::{PLACE_HEADER, parse, transfer};

/// How deep into the first part of nested multiparts the body is looked for.
const MAX_DEPTH: usize = 4;

/// How a message is protected, by its MIME.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// `multipart/signed`: S/MIME's or PGP/MIME's detached Digital signature.
    Signed,
    /// `multipart/encrypted`: a PGP/MIME Encrypted message.
    Encrypted,
    /// `application/pkcs7-mime`: S/MIME's signed or enveloped data in one part.
    Pkcs7,
    /// A plain text body with OpenPGP armour at the start of a line.
    InlinePgp,
}

/// How the message `bytes` is protected, if it is: by its top-level type, or, for a message
/// whose top level is another multipart (a mailing list wrapping it, say), by the first part
/// of it, as deep as [`MAX_DEPTH`]. A part of such a type that is an attachment (a file
/// `smime.p7m` sent along) doesn't make the message one. A type with no body is not one either:
/// only the header of such a message is here. Never fails; the work is linear in the bytes.
pub fn protection(bytes: &[u8]) -> Option<Protection> {
    let (headers, body) = header::split(bytes, true);
    of_part(&headers, body, 0)
}

/// Whether the message `bytes` is Digitally signed or Encrypted ([`protection`]).
pub fn is_protected(bytes: &[u8]) -> bool {
    protection(bytes).is_some()
}

fn of_part(headers: &Headers, body: &[u8], depth: usize) -> Option<Protection> {
    let content_type = headers.get("content-type").map(parse_params).unwrap_or_default();
    let disposition = headers.get("content-disposition").map(parse_params).unwrap_or_default();
    if disposition.value == "attachment" {
        return None;
    }
    let encoding = headers.get("content-transfer-encoding").map(|e| e.trim().to_ascii_lowercase()).unwrap_or_default();
    let media_type = content_type.value.as_str();
    let whole = body.iter().any(|b| !b.is_ascii_whitespace());
    match media_type {
        "multipart/signed" => well_formed_signed(&content_type, body).then_some(Protection::Signed),
        "multipart/encrypted" => well_formed_encrypted(&content_type, body).then_some(Protection::Encrypted),
        // S/MIME says what it holds (`smime-type`) and carries it in base64 (RFC 8551 §3.2).
        "application/pkcs7-mime" | "application/x-pkcs7-mime"
            if whole && (content_type.get("smime-type").is_some_and(|t| !t.trim().is_empty()) || encoding == "base64") =>
        {
            Some(Protection::Pkcs7)
        }
        // RFC 2045: a part that doesn't say what it is, is plain text.
        "text/plain" | "" => {
            let text = match encoding.as_str() {
                "base64" => transfer::base64_decode(body),
                "quoted-printable" => transfer::qp_decode(body),
                _ => return mailcraft_model::protected::has_armour_block(body).then_some(Protection::InlinePgp),
            };
            mailcraft_model::protected::has_armour_block(&text).then_some(Protection::InlinePgp)
        }
        multipart if multipart.starts_with("multipart/") && depth < MAX_DEPTH => {
            let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
            let first = crate::parse::split_multipart(body, boundary, 1);
            let (part_headers, part_body) = header::split(first.first()?, false);
            of_part(&part_headers, part_body, depth.saturating_add(1))
        }
        _ => None,
    }
}

/// The media types of the parts of the multipart `body` with `content_type`'s boundary, when it
/// has exactly `n` parts, each with something in it; else `None`.
fn part_types(content_type: &header::Params, body: &[u8], n: usize) -> Option<Vec<String>> {
    let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
    let parts = crate::parse::split_multipart(body, boundary, n.saturating_add(1));
    if parts.len() != n {
        return None;
    }
    parts
        .iter()
        .map(|part| {
            let (headers, part_body) = header::split(part, false);
            let whole = part_body.iter().any(|b| !b.is_ascii_whitespace());
            whole.then(|| headers.get("content-type").map(parse_params).unwrap_or_default().value)
        })
        .collect()
}

/// RFC 1847 §2.1: two parts, the second the signature, of the type `protocol` names, which is
/// S/MIME's (RFC 8551 §3.5) or PGP/MIME's (RFC 3156 §5).
fn well_formed_signed(content_type: &header::Params, body: &[u8]) -> bool {
    let protocol = content_type.get("protocol").map(|p| p.trim().to_ascii_lowercase()).unwrap_or_default();
    let known = matches!(protocol.as_str(), "application/pkcs7-signature" | "application/x-pkcs7-signature" | "application/pgp-signature");
    known && part_types(content_type, body, 2).is_some_and(|types| types.get(1) == Some(&protocol))
}

/// RFC 3156 §4: protocol `application/pgp-encrypted`, and two parts, the control part of that
/// type and the encrypted data as `application/octet-stream`.
fn well_formed_encrypted(content_type: &header::Params, body: &[u8]) -> bool {
    let protocol = content_type.get("protocol").map(|p| p.trim().to_ascii_lowercase()).unwrap_or_default();
    protocol == "application/pgp-encrypted"
        && part_types(content_type, body, 2).is_some_and(|types| types == ["application/pgp-encrypted", "application/octet-stream"])
}

/// [`parse()`] for a message as its server or a file gave it: a Digitally signed or Encrypted
/// one ([`protection`]) keeps the bytes it came as ([`Message::original`]), up to
/// [`mailcraft_model::MAX_ORIGINAL`]; any other is read exactly as [`parse()`] reads it. The line
/// of MailCraft's own that a copy put on a server carries first ([`PLACE_HEADER`]) is not kept.
pub fn parse_fetched(bytes: &[u8]) -> Message {
    let mut message = parse(bytes);
    // (Measured in CRLF lines before anything is made of it: no more than the cap is ever held.)
    let bare = bytes.windows(2).filter(|w| matches!(w, [before, b'\n'] if *before != b'\r')).count();
    let bare = bare.saturating_add(usize::from(bytes.first() == Some(&b'\n')));
    if bytes.len().saturating_add(bare) <= mailcraft_model::MAX_ORIGINAL && is_protected(bytes) {
        let canonical = crlf(without_place(bytes));
        message.original = Original::new(canonical);
    }
    message
}

/// `bytes` with every line ending in CRLF: a bare LF becomes CRLF, what is already CRLF stays.
/// The canonical form a Digital signature is made over (RFC 1847, RFC 8551 §3.1.1, RFC 3156 §5),
/// and what a mail server takes; a message read from an mbox or a file of LF lines is kept so.
pub fn crlf(bytes: &[u8]) -> Vec<u8> {
    let extra = bytes.iter().filter(|b| **b == b'\n').count();
    let mut out = Vec::with_capacity(bytes.len().saturating_add(extra));
    let mut previous = 0u8;
    for &b in bytes {
        if b == b'\n' && previous != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        previous = b;
    }
    out
}

/// The fields of the top-level header block of `original` that say what its body is
/// (`MIME-Version`, `Content-Type`, `Content-Transfer-Encoding`, `Content-Disposition`), each as
/// the original has it, folds and all, every line ending in CRLF (a stray CR or NUL in one is
/// dropped); and its body, byte for byte, in CRLF lines ([`crlf`]). What [`crate::write()`] puts
/// after the header fields it writes from the message, for a message that keeps its original.
pub(crate) fn content_of(original: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let (_, body) = header::split(original, true);
    let block = original.get(..original.len().saturating_sub(body.len())).unwrap_or_default();
    let mut fields = Vec::new();
    let mut taking = false;
    let mut pos = 0;
    while pos < block.len() {
        let (line, next) = header::line_at(block, pos);
        if next <= pos {
            break;
        }
        pos = next;
        let line: Vec<u8> = line.iter().copied().filter(|b| *b != b'\r' && *b != 0).collect();
        if line.is_empty() {
            break;
        }
        if matches!(line.first(), Some(b' ' | b'\t')) {
            // (A fold of nothing but white space is left out: some readers take it for the end of
            // the header.)
            if taking && !line.iter().all(u8::is_ascii_whitespace) {
                fields.extend_from_slice(&line);
                fields.extend_from_slice(b"\r\n");
            }
            continue;
        }
        let name = line.iter().position(|b| *b == b':').and_then(|at| line.get(..at)).map(|n| n.trim_ascii().to_ascii_lowercase());
        taking = matches!(name.as_deref(), Some(b"mime-version" | b"content-type" | b"content-transfer-encoding" | b"content-disposition"));
        if taking {
            fields.extend_from_slice(&line);
            fields.extend_from_slice(b"\r\n");
        }
    }
    (fields, crlf(body))
}

/// `bytes` without the [`PLACE_HEADER`] lines MailCraft put first in a copy it made.
fn without_place(mut bytes: &[u8]) -> &[u8] {
    loop {
        let (line, next) = header::line_at(bytes, 0);
        let named = line.get(..PLACE_HEADER.len()).is_some_and(|name| name.eq_ignore_ascii_case(PLACE_HEADER.as_bytes()));
        if !named || line.get(PLACE_HEADER.len()) != Some(&b':') || next == 0 {
            return bytes;
        }
        bytes = bytes.get(next..).unwrap_or_default();
    }
}
