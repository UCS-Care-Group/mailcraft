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
    let media_type = content_type.value.as_str();
    let whole = body.iter().any(|b| !b.is_ascii_whitespace());
    match media_type {
        "multipart/signed" if whole => Some(Protection::Signed),
        "multipart/encrypted" if whole => Some(Protection::Encrypted),
        "application/pkcs7-mime" | "application/x-pkcs7-mime" if whole => Some(Protection::Pkcs7),
        // RFC 2045: a part that doesn't say what it is, is plain text.
        "text/plain" | "" => {
            let encoding = headers.get("content-transfer-encoding").map(|e| e.trim().to_ascii_lowercase()).unwrap_or_default();
            let text = match encoding.as_str() {
                "base64" => transfer::base64_decode(body),
                "quoted-printable" => transfer::qp_decode(body),
                _ => return mailcraft_model::protected::has_armour(body).then_some(Protection::InlinePgp),
            };
            mailcraft_model::protected::has_armour(&text).then_some(Protection::InlinePgp)
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

/// [`parse()`] for a message as its server or a file gave it: a Digitally signed or Encrypted
/// one ([`protection`]) keeps the bytes it came as ([`Message::original`]), up to
/// [`mailcraft_model::MAX_ORIGINAL`]; any other is read exactly as [`parse()`] reads it. The line
/// of MailCraft's own that a copy put on a server carries first ([`PLACE_HEADER`]) is not kept.
pub fn parse_fetched(bytes: &[u8]) -> Message {
    let mut message = parse(bytes);
    if bytes.len() <= mailcraft_model::MAX_ORIGINAL && is_protected(bytes) {
        message.original = Original::new(without_place(bytes).to_vec());
    }
    message
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
