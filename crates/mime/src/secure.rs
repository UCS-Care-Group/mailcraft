//! The MIME side of signed and encrypted mail: S/MIME (RFC 8551) and PGP/MIME (RFC 3156, with the
//! `multipart/signed` and `multipart/encrypted` of RFC 1847).
//!
//! Reading: [`detect`] tells a protected message from its top-level `Content-Type` and keeps the
//! entity as it came ([`Secured`]); [`signed_parts`] splits a `multipart/signed` into the exact
//! bytes that were signed and the signature; [`encrypted_payload`] takes out what is encrypted.
//! Writing: [`content_entity`] writes a message's body and attachments as one MIME entity, and
//! the `wrap_*` functions put a signature or an encrypted payload around it.
//!
//! No cryptography is done here (`mailcraft-crypto` does it). Everything read is hostile: the
//! splitting is the same bounded splitting as the rest of the reader.

use std::sync::Arc;

use mailcraft_model::{Message, Protection, Secured};

use crate::header::{self, Headers, parse_params};
use crate::{charset, transfer};

/// What protects a message, from its top-level content fields, if anything.
pub(crate) fn protection(headers: &Headers) -> Option<Protection> {
    let ct = headers.get("content-type").map(parse_params)?;
    let protocol = ct.get("protocol").unwrap_or("").trim().to_ascii_lowercase();
    match ct.value.as_str() {
        "multipart/signed" => match protocol.as_str() {
            "application/pkcs7-signature" | "application/x-pkcs7-signature" => Some(Protection::SmimeSigned),
            "application/pgp-signature" => Some(Protection::PgpSigned),
            _ => None,
        },
        "multipart/encrypted" if protocol == "application/pgp-encrypted" => Some(Protection::PgpEncrypted),
        "application/pkcs7-mime" | "application/x-pkcs7-mime" => {
            match ct.get("smime-type").unwrap_or("").trim().to_ascii_lowercase().as_str() {
                "signed-data" => Some(Protection::SmimeSignedData),
                "enveloped-data" | "authenveloped-data" => Some(Protection::SmimeEncrypted),
                // Without the parameter (older senders): by the file name, as RFC 8551 §3.2.2
                // has it; an `.p7m` is enveloped more often than not, and the CMS itself says
                // which it is when it is opened.
                "" => Some(Protection::SmimeEncrypted),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The protected entity of a message: its content fields as they came and its body, CRLF.
pub(crate) fn entity(headers: &Headers, body: &[u8]) -> Arc<Vec<u8>> {
    let mut out = Vec::with_capacity(body.len() + 256);
    for name in ["Content-Type", "Content-Transfer-Encoding", "Content-Disposition"] {
        if let Some(value) = headers.get(name) {
            header::write_field(&mut out, name, value);
        }
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&crlf(body));
    Arc::new(out)
}

/// Line ends as CRLF, bare CR and LF alike.
fn crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 32);
    let mut prev = 0u8;
    for &b in bytes {
        if b == b'\n' && prev != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        prev = b;
    }
    out
}

/// What [`signed_parts`] gives: the signed entity exactly as it was signed (canonical CRLF) and
/// the signature (transfer encoding undone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedParts {
    pub content: Vec<u8>,
    pub signature: Vec<u8>,
}

/// Split a `multipart/signed` entity ([`Secured::raw`]) into what is signed and the signature.
/// `None` when it isn't one, or has no second part.
pub fn signed_parts(raw: &[u8]) -> Option<SignedParts> {
    let (headers, body) = header::split(raw, false);
    let ct = headers.get("content-type").map(parse_params)?;
    if ct.value != "multipart/signed" {
        return None;
    }
    let boundary = ct.get("boundary").filter(|b| !b.is_empty())?;
    let parts = crate::parse::split_multipart(body, boundary, 2);
    let content = parts.first()?;
    let (sig_headers, sig_body) = header::split(parts.get(1)?, false);
    Some(SignedParts { content: crlf(content), signature: decoded(&sig_headers, sig_body) })
}

fn decoded(headers: &Headers, body: &[u8]) -> Vec<u8> {
    match headers.get("content-transfer-encoding").map(|e| e.trim().to_ascii_lowercase()).as_deref() {
        Some("base64") => transfer::base64_decode(body),
        Some("quoted-printable") => transfer::qp_decode(body),
        _ => body.to_vec(),
    }
}

/// What is encrypted (or wrapped in a signature) in a protected entity: the CMS of an
/// `application/pkcs7-mime` (DER, from its base64), or the OpenPGP message of a
/// `multipart/encrypted` (its second part, armored text).
pub fn encrypted_payload(raw: &[u8]) -> Option<Vec<u8>> {
    let (headers, body) = header::split(raw, false);
    let ct = headers.get("content-type").map(parse_params)?;
    match ct.value.as_str() {
        "application/pkcs7-mime" | "application/x-pkcs7-mime" => Some(decoded(&headers, body)),
        "multipart/encrypted" => {
            let boundary = ct.get("boundary").filter(|b| !b.is_empty())?;
            let parts = crate::parse::split_multipart(body, boundary, 2);
            let (h, b) = header::split(parts.get(1)?, false);
            Some(decoded(&h, b))
        }
        _ => None,
    }
}

/// A message's body and attachments as one MIME entity (content fields, an empty line, the
/// content), CRLF: what is signed or encrypted when the message goes out.
pub fn content_entity(message: &Message) -> Vec<u8> {
    crate::write::body_entity(message)
}

/// A `multipart/signed` around `entity` with an S/MIME signature (DER).
pub fn wrap_smime_signed(entity: &[u8], signature: &[u8]) -> Secured {
    let sig_part = [
        b"Content-Type: application/pkcs7-signature; name=\"smime.p7s\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"smime.p7s\"\r\n\r\n".as_slice(),
        &transfer::base64_lines(signature),
    ]
    .concat();
    signed(entity, &sig_part, "application/pkcs7-signature", "sha-256", Protection::SmimeSigned)
}

/// A `multipart/signed` around `entity` with an OpenPGP signature (armored).
pub fn wrap_pgp_signed(entity: &[u8], signature: &str) -> Secured {
    let sig_part = [
        b"Content-Type: application/pgp-signature; name=\"signature.asc\"\r\nContent-Description: OpenPGP digital signature\r\nContent-Disposition: attachment; filename=\"signature.asc\"\r\n\r\n".as_slice(),
        &crlf(signature.as_bytes()),
    ]
    .concat();
    signed(entity, &sig_part, "application/pgp-signature", "pgp-sha256", Protection::PgpSigned)
}

fn signed(entity: &[u8], sig_part: &[u8], protocol: &str, micalg: &str, kind: Protection) -> Secured {
    let children = vec![entity.to_vec(), sig_part.to_vec()];
    let boundary = crate::write::boundary(&children, &mut 0);
    let mut raw =
        format!("Content-Type: multipart/signed; protocol=\"{protocol}\"; micalg={micalg};\r\n boundary=\"{boundary}\"\r\n\r\n").into_bytes();
    if kind == Protection::SmimeSigned {
        raw.extend_from_slice(b"This is a signed message.\r\n\r\n");
    }
    for child in &children {
        raw.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        // Exactly what was signed; the line break after it belongs to the delimiter (RFC 2046).
        raw.extend_from_slice(child);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Secured { kind, raw: Arc::new(raw) }
}

/// An `application/pkcs7-mime` entity holding enveloped data (DER).
pub fn wrap_smime_encrypted(cms: &[u8]) -> Secured {
    let mut raw = b"Content-Type: application/pkcs7-mime; smime-type=enveloped-data; name=\"smime.p7m\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"smime.p7m\"\r\n\r\n".to_vec();
    raw.extend_from_slice(&transfer::base64_lines(cms));
    raw.extend_from_slice(b"\r\n");
    Secured { kind: Protection::SmimeEncrypted, raw: Arc::new(raw) }
}

/// A `multipart/encrypted` entity holding an OpenPGP message (armored).
pub fn wrap_pgp_encrypted(armored: &str) -> Secured {
    let control = b"Content-Type: application/pgp-encrypted\r\nContent-Description: PGP/MIME version identification\r\n\r\nVersion: 1\r\n".to_vec();
    let data = [b"Content-Type: application/octet-stream; name=\"encrypted.asc\"\r\nContent-Description: OpenPGP encrypted message\r\nContent-Disposition: inline; filename=\"encrypted.asc\"\r\n\r\n".as_slice(), &crlf(armored.as_bytes())].concat();
    let children = vec![control, data];
    let boundary = crate::write::boundary(&children, &mut 0);
    let mut raw = format!("Content-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\";\r\n boundary=\"{boundary}\"\r\n\r\nThis is an OpenPGP/MIME encrypted message (RFC 4880 and 3156)\r\n").into_bytes();
    for child in &children {
        raw.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        raw.extend_from_slice(child);
        if !child.ends_with(b"\r\n") {
            raw.extend_from_slice(b"\r\n");
        }
    }
    raw.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Secured { kind: Protection::PgpEncrypted, raw: Arc::new(raw) }
}

/// Read a MIME entity that came out of a signature or an encryption: its body, its attachments,
/// and its own protection when it is protected in turn (signed, then encrypted). The entity has
/// content fields only; nothing in it can change who the message is from.
pub fn read_entity(entity: &[u8]) -> Message {
    crate::parse::parse_entity(entity)
}

/// Whether a text body is an inline OpenPGP message or signed text, and which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InlinePgp {
    Signed,
    Encrypted,
}

/// An inline OpenPGP block in a text body (`-----BEGIN PGP SIGNED MESSAGE-----` or
/// `-----BEGIN PGP MESSAGE-----` at the start of a line), and the block itself.
pub fn inline_pgp(text: &str) -> Option<(InlinePgp, &str)> {
    for (marker, end, kind) in [
        ("-----BEGIN PGP SIGNED MESSAGE-----", "-----END PGP SIGNATURE-----", InlinePgp::Signed),
        ("-----BEGIN PGP MESSAGE-----", "-----END PGP MESSAGE-----", InlinePgp::Encrypted),
    ] {
        let Some(start) = text.find(marker) else { continue };
        if start != 0 && !text.get(..start).is_some_and(|before| before.ends_with('\n')) {
            continue;
        }
        let rest = text.get(start..)?;
        let stop = rest.find(end).map(|i| i + end.len()).unwrap_or(rest.len());
        return Some((kind, rest.get(..stop)?));
    }
    None
}

/// The text of a decrypted inline OpenPGP message, read as UTF-8 (what OpenPGP programs write)
/// with line ends as `\n`.
pub fn inline_text(bytes: &[u8]) -> String {
    charset::unix_lines(&charset::decode(bytes, "utf-8"))
}

#[cfg(test)]
mod tests;
