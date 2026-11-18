// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The halves of a Digitally signed message (#30), for its Digital signature to be checked
//! (`mailcraft-smime`, and OpenPGP's checker): of `multipart/signed` (RFC 1847 §2.1), the signed
//! part exactly as it came, MIME header and all, and the signature part's bytes; of S/MIME's
//! `application/pkcs7-mime` (RFC 8551 §3.2), the CMS it carries. Read from the bytes the message
//! came as ([`crate::Message::original`]); nothing here checks anything.

use crate::header::{self, Headers, parse_params};
use crate::transfer;

/// How deep into the first part of nested multiparts a signed message is looked for, as
/// [`crate::protection`] looks.
const MAX_DEPTH: usize = 4;

/// Which kind of Digital signature a message carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignedKind {
    /// `multipart/signed` with `application/pkcs7-signature`: S/MIME, detached.
    SmimeDetached,
    /// `application/pkcs7-mime`: S/MIME's CMS in one part (signed data, or enveloped data,
    /// which the CMS itself tells).
    SmimeOpaque,
    /// `multipart/signed` with `application/pgp-signature`: PGP/MIME (RFC 3156 §5).
    PgpDetached,
}

/// The halves of a Digitally signed message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedParts {
    pub kind: SignedKind,
    /// The signed part as it came, header and body, without the line end before the next
    /// boundary (RFC 1847 §2.1). Empty for [`SignedKind::SmimeOpaque`], whose CMS holds it.
    pub content: Vec<u8>,
    /// The signature part's bytes, its transfer encoding undone (the CMS or the OpenPGP
    /// signature); for [`SignedKind::SmimeOpaque`], the CMS.
    pub signature: Vec<u8>,
    /// The signed part is the whole message body (it is the message's top level): `false` when
    /// it is only the first part of a wrapper (a list's footer added after it, say), whose other
    /// parts nobody signed.
    pub whole: bool,
    /// The message's header names exactly one From mailbox (one field, one address): a
    /// signature can be for its sender only then.
    pub one_from: bool,
}

/// The halves of the message `bytes` when it is Digitally signed (or, for
/// [`SignedKind::SmimeOpaque`], may be): at its top level, or the first part of a multipart
/// wrapping it, as deep as [`MAX_DEPTH`] ([`SignedParts::whole`] says which), never inside a
/// `multipart/alternative` (whose other versions are shown instead as often). `None` for any other
/// message. Never fails.
pub fn signed_parts(bytes: &[u8]) -> Option<SignedParts> {
    let (headers, body) = header::split(bytes, true);
    let mut parts = of_part(&headers, body, 0)?;
    let froms: Vec<&str> = headers.0.iter().filter(|f| f.name.eq_ignore_ascii_case("from")).map(|f| f.value.as_str()).collect();
    parts.one_from = matches!(froms.as_slice(), [one] if crate::address::parse_list(one).len() == 1);
    Some(parts)
}

fn of_part(headers: &Headers, body: &[u8], depth: usize) -> Option<SignedParts> {
    let content_type = headers.get("content-type").map(parse_params).unwrap_or_default();
    let media_type = content_type.value.as_str();
    let disposition = headers.get("content-disposition").map(parse_params).unwrap_or_default();
    let pkcs7 = matches!(media_type, "application/pkcs7-mime" | "application/x-pkcs7-mime");
    // (A part of such a type sent along as a file is not the message's: see `protection`.)
    if disposition.value == "attachment" && (depth > 0 || !pkcs7) {
        return None;
    }
    match media_type {
        "multipart/signed" => {
            let protocol = content_type.get("protocol").unwrap_or_default().trim().to_ascii_lowercase();
            let kind = match protocol.as_str() {
                "application/pkcs7-signature" | "application/x-pkcs7-signature" => SignedKind::SmimeDetached,
                "application/pgp-signature" => SignedKind::PgpDetached,
                _ => return None,
            };
            let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
            let parts = crate::parse::split_multipart(body, boundary, 3);
            let [content, signature] = parts.as_slice() else { return None };
            let (sig_headers, sig_body) = header::split(signature, false);
            // The second part is the signature only when it is of the type `protocol` names (RFC
            // 1847 §2.1): any other is a part of the message, shown as one, and nobody's signature.
            let sig_type = sig_headers.get("content-type").map(parse_params).unwrap_or_default().value;
            let fits = match kind {
                SignedKind::SmimeDetached => matches!(sig_type.as_str(), "application/pkcs7-signature" | "application/x-pkcs7-signature"),
                _ => sig_type == "application/pgp-signature",
            };
            if !fits {
                return None;
            }
            Some(SignedParts { kind, content: content.to_vec(), signature: decoded(&sig_headers, sig_body), whole: depth == 0, one_from: false })
        }
        _ if pkcs7 => {
            // (`smime-type` names what is inside; the CMS says it too, and is what is believed.)
            let decoded = decoded(headers, body);
            (!decoded.is_empty()).then_some(SignedParts {
                kind: SignedKind::SmimeOpaque,
                content: Vec::new(),
                signature: decoded,
                whole: depth == 0,
                one_from: false,
            })
        }
        // (Never into `multipart/alternative`: a signed first version beside an unsigned one
        // would lend the other its signature.)
        multipart if multipart.starts_with("multipart/") && multipart != "multipart/alternative" && depth < MAX_DEPTH => {
            let boundary = content_type.get("boundary").filter(|b| !b.is_empty())?;
            let first = crate::parse::split_multipart(body, boundary, 1);
            let (part_headers, part_body) = header::split(first.first()?, false);
            of_part(&part_headers, part_body, depth.saturating_add(1))
        }
        _ => None,
    }
}

/// `body` with the transfer encoding its `headers` name undone: base64 or quoted-printable;
/// anything else as it is.
fn decoded(headers: &Headers, body: &[u8]) -> Vec<u8> {
    let encoding = headers.get("content-transfer-encoding").map(|e| e.trim().to_ascii_lowercase()).unwrap_or_default();
    match encoding.as_str() {
        "base64" => transfer::base64_decode(body),
        "quoted-printable" => transfer::qp_decode(body),
        _ => body.to_vec(),
    }
}
