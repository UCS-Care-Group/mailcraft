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
    // A part of such a type sent along as a file isn't the message's protection; the message
    // itself may say `attachment; filename="smime.p7m"`, as S/MIME programs write (RFC 8551
    // §3.2.1), and is protected all the same.
    let disposition = headers.get("content-disposition").map(parse_params).unwrap_or_default();
    if depth > 0 && disposition.value == "attachment" {
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
            if whole
                && (content_type.get("smime-type").is_some_and(|t| !t.trim().is_empty()) || matches!(encoding.as_str(), "base64" | "binary")) =>
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

/// The parts of the multipart `body` with `content_type`'s boundary, when it has exactly `n`:
/// each one's media type, and whether it holds anything but white space.
fn parts_of(content_type: &header::Params, body: &[u8], n: usize) -> Option<Vec<(String, bool)>> {
    let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
    let parts = crate::parse::split_multipart(body, boundary, n.saturating_add(1));
    if parts.len() != n {
        return None;
    }
    let part = |part: &&[u8]| {
        let (headers, part_body) = header::split(part, false);
        let media_type = headers.get("content-type").map(parse_params).unwrap_or_default().value;
        (media_type, part_body.iter().any(|b| !b.is_ascii_whitespace()))
    };
    Some(parts.iter().map(part).collect())
}

/// A signature's media type without the `x-` of its old form: `application/x-pkcs7-signature`
/// and `application/pkcs7-signature` are the same thing (RFC 8551 §3.5).
fn signature_type(media_type: &str) -> String {
    let media_type = media_type.trim().to_ascii_lowercase();
    match media_type.strip_prefix("application/x-") {
        Some(rest) => format!("application/{rest}"),
        None => media_type,
    }
}

/// RFC 1847 §2.1: two parts, the second the signature, with something in it, of the type
/// `protocol` names, which is S/MIME's (RFC 8551 §3.5) or PGP/MIME's (RFC 3156 §5). The signed
/// part may be empty: an empty body can be signed.
fn well_formed_signed(content_type: &header::Params, body: &[u8]) -> bool {
    let protocol = signature_type(content_type.get("protocol").unwrap_or_default());
    let known = matches!(protocol.as_str(), "application/pkcs7-signature" | "application/pgp-signature");
    known
        && parts_of(content_type, body, 2)
            .is_some_and(|parts| matches!(parts.get(1), Some((signature, true)) if signature_type(signature) == protocol))
}

/// RFC 3156 §4: protocol `application/pgp-encrypted`, and two parts, the control part of that
/// type and the encrypted data as `application/octet-stream`, which holds something.
fn well_formed_encrypted(content_type: &header::Params, body: &[u8]) -> bool {
    let protocol = content_type.get("protocol").map(|p| p.trim().to_ascii_lowercase()).unwrap_or_default();
    protocol == "application/pgp-encrypted"
        && parts_of(content_type, body, 2).is_some_and(|parts| {
            matches!(parts.as_slice(), [(control, _), (data, true)] if control == "application/pgp-encrypted" && data == "application/octet-stream")
        })
}

/// How many parts of each multipart are looked into for one sent as `binary`.
const MAX_PARTS_LOOKED: usize = 64;

/// Whether the message `bytes`, or a part of it as deep as [`MAX_DEPTH`], is sent as `binary`
/// (RFC 2045 §6.2): bytes with no lines, whose LF bytes are data, not line ends.
fn has_binary(bytes: &[u8]) -> bool {
    fn within(headers: &Headers, body: &[u8], depth: usize) -> bool {
        let encoding = headers.get("content-transfer-encoding").map(|e| e.trim().to_ascii_lowercase()).unwrap_or_default();
        if encoding == "binary" {
            return true;
        }
        let content_type = headers.get("content-type").map(parse_params).unwrap_or_default();
        let Some(boundary) = content_type.get("boundary").filter(|b| !b.is_empty() && content_type.value.starts_with("multipart/")) else {
            return false;
        };
        depth < MAX_DEPTH
            && crate::parse::split_multipart(body, boundary, MAX_PARTS_LOOKED).iter().any(|part| {
                let (headers, body) = header::split(part, false);
                within(&headers, body, depth.saturating_add(1))
            })
    }
    let (headers, body) = header::split(bytes, true);
    within(&headers, body, 0)
}

/// `bytes` as a Digital signature is checked against: in CRLF lines ([`crlf`]), but for a
/// message with a part sent as `binary`, which is kept byte for byte.
fn canonical(bytes: &[u8]) -> Vec<u8> {
    if has_binary(bytes) { bytes.to_vec() } else { crlf(bytes) }
}

/// [`parse()`] for a message as its server or a file gave it: a Digitally signed or Encrypted
/// one ([`protection`]) keeps the bytes it came as ([`Message::original`]), in CRLF lines
/// ([`canonical`]), up to [`mailcraft_model::MAX_ORIGINAL`]; any other is read exactly as
/// [`parse()`] reads it. The line of MailCraft's own that a copy put on a server carries first
/// ([`PLACE_HEADER`]) is not kept.
pub fn parse_fetched(bytes: &[u8]) -> Message {
    let mut message = parse(bytes);
    // (Measured in CRLF lines before anything is made of it: no more than the cap is ever held.)
    let bare = bytes.windows(2).filter(|w| matches!(w, [before, b'\n'] if *before != b'\r')).count();
    let bare = bare.saturating_add(usize::from(bytes.first() == Some(&b'\n')));
    if bytes.len().saturating_add(bare) <= mailcraft_model::MAX_ORIGINAL && is_protected(bytes) {
        message.original = Original::new(canonical(without_place(bytes)));
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

/// Longest line a header may have (RFC 5322 §2.1.1), without its CRLF.
const MAX_HEADER_LINE: usize = 998;

/// Push the header line `line` onto `out`, ending in CRLF, folded before white space where it
/// is longer than [`MAX_HEADER_LINE`]. `None` when it can't be folded so (no white space where
/// it would have to be).
fn push_folded(out: &mut Vec<u8>, mut line: &[u8]) -> Option<()> {
    while line.len() > MAX_HEADER_LINE {
        // The last white space a fold can go before, past the line's first byte.
        let at = line.get(1..=MAX_HEADER_LINE)?.iter().rposition(|b| matches!(b, b' ' | b'\t'))?.saturating_add(1);
        out.extend_from_slice(line.get(..at)?);
        out.extend_from_slice(b"\r\n");
        line = line.get(at..)?;
    }
    out.extend_from_slice(line);
    out.extend_from_slice(b"\r\n");
    Some(())
}

/// The fields of the top-level header block of `original` that say what its body is
/// (`MIME-Version`, `Content-Type`, `Content-Transfer-Encoding`, `Content-Disposition`), each as
/// the original has it, folds and all, every line ending in CRLF (a stray CR or NUL in one is
/// dropped, a line too long is folded), with `MIME-Version: 1.0` when it has none; and its body,
/// byte for byte, canonical ([`canonical`]). What [`crate::write()`] puts after the header
/// fields it writes from the message, for a message that keeps its original. `None` when a line
/// can't be written within the length a header line may have.
pub(crate) fn content_of(original: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (_, body) = header::split(original, true);
    let block = original.get(..original.len().saturating_sub(body.len())).unwrap_or_default();
    let mut fields = Vec::new();
    let mut taking = false;
    let mut version = false;
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
                push_folded(&mut fields, &line)?;
            }
            continue;
        }
        let name = line.iter().position(|b| *b == b':').and_then(|at| line.get(..at)).map(|n| n.trim_ascii().to_ascii_lowercase());
        taking = matches!(name.as_deref(), Some(b"mime-version" | b"content-type" | b"content-transfer-encoding" | b"content-disposition"));
        version |= name.as_deref() == Some(b"mime-version");
        if taking {
            push_folded(&mut fields, &line)?;
        }
    }
    if !version {
        fields.splice(0..0, b"MIME-Version: 1.0\r\n".iter().copied());
    }
    let body = if has_binary(original) { body.to_vec() } else { crlf(body) };
    Some((fields, body))
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
