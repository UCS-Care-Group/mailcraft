// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An Encrypted message's body as it is sent (#30 part 7): S/MIME's `application/pkcs7-mime;
//! smime-type=enveloped-data` (RFC 8551 §3.3, the CMS EnvelopedData in base64) or PGP/MIME's
//! `multipart/encrypted` (RFC 3156 §4: a control part and the armoured OpenPGP message). The
//! encryption itself is made elsewhere (`mailcraft-smime`, `mailcraft-openpgp`): this is only the
//! MIME around it, and the entity that is encrypted ([`entity_to_encrypt`]).
//!
//! What comes out is the content fields of the message (`MIME-Version`, `Content-Type`, ...) and
//! its body, which a message keeps as the bytes it goes as ([`mailcraft_model::Message::original`]):
//! every path that sends it, and the copy kept in Sent Items, write those bytes, as for a Digitally
//! signed one ([`crate::multipart_signed`]).

use mailcraft_model::Message;

use crate::protected::{Protection, content_of, protection};
use crate::signing::{LayoutError, boundary};
use crate::transfer;

/// Why a message that was to go Encrypted isn't, said by every path that sends it before
/// anything goes (#30 part 7): it is never sent in the clear.
pub const NOT_ENCRYPTED: &str = "it was to be Encrypted and wasn't, so it wasn't sent";

/// Whether `message` was to be Encrypted at Send ([`Message::encrypt_with`]) and what it would
/// be written as isn't an Encrypted message: no encrypted body kept, or one that isn't S/MIME's
/// enveloped data or PGP/MIME's `multipart/encrypted`; or a Bcc recipient has no message of their
/// own that is ([`Message::blind_copies`]: one message for everybody would name them in its key
/// lines). Every path that sends mail refuses such a message.
pub fn encryption_missing(message: &Message) -> bool {
    match &message.encrypt_with {
        Some(_) => {
            body_not_encrypted(message)
                || message.blind_addresses().iter().any(|address| {
                    !message.blind_copies.iter().any(|c| c.address == *address && c.original.is_loaded() && is_encrypted(c.original.data.as_slice()))
                })
        }
        // Encrypt asked for and nothing decided (a message put in the Outbox otherwise than with
        // Send, a recipient's key gone since): it isn't sent in the clear.
        None => message.encrypt == Some(true) && !message.draft,
    }
}

/// Whether `message`, to be Encrypted ([`Message::encrypt_with`]), has a body that isn't an Encrypted
/// message: the check for one message of its own ([`Message::blind_messages`]).
pub fn body_not_encrypted(message: &Message) -> bool {
    message.encrypt_with.is_some() && !(crate::writes_original(message) && message.original_bytes().is_some_and(is_encrypted))
}

/// Whether the message `bytes` is an Encrypted message: PGP/MIME's
/// `multipart/encrypted`, or S/MIME's enveloped data (not signed data).
pub fn is_encrypted(bytes: &[u8]) -> bool {
    match protection(bytes) {
        Some(Protection::Encrypted) => true,
        Some(Protection::Pkcs7) => {
            let (headers, _) = crate::header::split(bytes, true);
            headers
                .get("content-type")
                .map(crate::header::parse_params)
                .is_some_and(|p| p.get("smime-type").is_some_and(|t| t.trim().eq_ignore_ascii_case("enveloped-data")))
        }
        _ => false,
    }
}

/// The entity that is encrypted for `message` (RFC 8551 §3.1, RFC 3156 §4): the signed body when
/// the message was Digitally signed first ([`Message::original`] holds a `multipart/signed`, so
/// the signature goes inside the encryption, as RFC 8551 §3.6 has it), else the message's own
/// content ([`crate::signable`]). Content fields, an empty line and the body, in CRLF lines,
/// without a line end after it. The outer `MIME-Version` isn't part of it.
pub fn entity_to_encrypt(message: &Message) -> Vec<u8> {
    if crate::writes_original(message)
        && let Some((fields, body)) = message.original_bytes().and_then(content_of)
    {
        let mut out = Vec::with_capacity(fields.len().saturating_add(body.len()).saturating_add(2));
        let mut skipping = false;
        for line in fields.split_inclusive(|b| *b == b'\n') {
            if !matches!(line.first(), Some(b' ' | b'\t')) {
                skipping = line.get(..13).is_some_and(|name| name.eq_ignore_ascii_case(b"MIME-Version:"));
            }
            if !skipping {
                out.extend_from_slice(line);
            }
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&body);
        while out.ends_with(b"\r\n\r\n") {
            out.truncate(out.len().saturating_sub(2));
        }
        return out;
    }
    crate::signable(message)
}

/// The content fields and body of S/MIME's enveloped data: the CMS ContentInfo `der` (from
/// `mailcraft_smime::encrypt`) in base64.
pub fn smime_enveloped(der: &[u8]) -> Result<Vec<u8>, LayoutError> {
    if der.is_empty() {
        return Err(LayoutError::Empty);
    }
    let mut out = b"MIME-Version: 1.0\r\n\
Content-Type: application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\"\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Disposition: attachment; filename=\"smime.p7m\"\r\n\
Content-Description: S/MIME Encrypted Message\r\n\r\n"
        .to_vec();
    out.extend_from_slice(&transfer::base64_lines(der));
    out.extend_from_slice(b"\r\n");
    Ok(out)
}

/// The content fields and body of PGP/MIME's Encrypted message: the armoured OpenPGP message
/// (`-----BEGIN PGP MESSAGE-----`, from `mailcraft_openpgp::encrypt`) as the second part of a
/// `multipart/encrypted`, with the control part `Version: 1` before it. The boundary is made from
/// the content and occurs in neither part.
pub fn multipart_encrypted(armoured: &[u8]) -> Result<Vec<u8>, LayoutError> {
    let text = std::str::from_utf8(armoured).map_err(|_| LayoutError::NotArmoured)?;
    let ok = text.trim_start().starts_with("-----BEGIN PGP MESSAGE-----")
        && text.trim_end().ends_with("-----END PGP MESSAGE-----")
        && text.bytes().all(|b| matches!(b, b'\r' | b'\n' | 0x20..=0x7E));
    if !ok {
        return Err(LayoutError::NotArmoured);
    }
    let lines: Vec<&str> = text.trim().lines().map(|l| l.trim_end_matches('\r')).collect();
    let control = b"Content-Type: application/pgp-encrypted\r\nContent-Description: PGP/MIME version identification\r\n\r\nVersion: 1".to_vec();
    let mut data = b"Content-Type: application/octet-stream; name=\"encrypted.asc\"\r\n\
Content-Description: OpenPGP encrypted message\r\n\
Content-Disposition: inline; filename=\"encrypted.asc\"\r\n\r\n"
        .to_vec();
    data.extend_from_slice(lines.join("\r\n").as_bytes());
    let boundary = boundary(&[&control, &data], "encrypted").ok_or(LayoutError::Boundary)?;
    let mut out = Vec::with_capacity(data.len().saturating_add(512));
    out.extend_from_slice(b"MIME-Version: 1.0\r\n");
    out.extend_from_slice(
        format!("Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\";\r\n boundary=\"{boundary}\"\r\n\r\n").as_bytes(),
    );
    out.extend_from_slice(b"This is an OpenPGP/MIME Encrypted message (RFC 4880 and 3156).\r\n\r\n");
    for part in [&control, &data] {
        out.extend_from_slice(b"--");
        out.extend_from_slice(boundary.as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(part);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary.as_bytes());
    out.extend_from_slice(b"--\r\n");
    Ok(out)
}
