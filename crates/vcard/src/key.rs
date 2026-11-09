// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `KEY`: an S/MIME certificate or an OpenPGP public key on a card, kept as bytes.
//!
//! Read in every form the versions have: vCard 2.1 (`KEY;X509;ENCODING=BASE64:`), 3.0
//! (`KEY;ENCODING=b;TYPE=PGP:`, RFC 2426) and 4.0 (`KEY:data:application/pgp-keys;base64,…` or
//! `KEY;MEDIATYPE=application/pgp-keys:https://…`, RFC 6350), and a key written as text (an
//! ASCII-armoured OpenPGP key, a PEM certificate). Nothing is checked here and nothing is fetched:
//! an address is kept as an address. Written as vCard 3.0.

use mailcraft_model::{ContactKey, KeyKind};

use crate::lines::ContentLine;
use crate::text::{base64_bytes, quoted_printable, unescape};

/// The schemes an address of a key may have; any other address is dropped.
const SCHEMES: [&str; 5] = ["http", "https", "ldap", "ldaps", "openpgp4fpr"];
const PEM_BEGIN: &str = "-----BEGIN CERTIFICATE-----";
const PEM_END: &str = "-----END CERTIFICATE-----";
const PGP_BEGIN: &str = "-----BEGIN PGP PUBLIC KEY BLOCK-----";

/// Most characters of a value read for a key: base64 of the largest key kept, with room for
/// line breaks and a `data:` header. A longer value holds a key too large to keep.
fn max_value() -> usize {
    ContactKey::MAX_BYTES.saturating_div(3).saturating_add(1).saturating_mul(4).saturating_mul(2).saturating_add(256)
}

/// The kind a type or media type names: `X509`, `PGP`, `application/pkix-cert`,
/// `application/pgp-keys` and the spellings writers use for them.
fn kind_named(name: &str) -> Option<KeyKind> {
    match name.trim().to_ascii_lowercase().as_str() {
        "x509" | "x.509" | "x-x509" | "application/pkix-cert" | "application/x-x509-ca-cert" | "application/x-x509-user-cert" => Some(KeyKind::X509),
        "pgp" | "x-pgp" | "gpg" | "openpgp" | "application/pgp-keys" | "application/pgp" | "application/x-pgp-keys" => Some(KeyKind::OpenPgp),
        _ => None,
    }
}

/// The key of one `KEY` line; `None` for one that holds nothing kept (an unknown kind, junk, a key
/// too large, an address of another scheme).
pub(crate) fn read(line: &ContentLine) -> Option<ContactKey> {
    if line.value.len() > max_value() {
        return None;
    }
    let stated = line.types().iter().chain(line.param("MEDIATYPE").map(str::to_string).iter()).find_map(|t| kind_named(t));
    match line.param("ENCODING").unwrap_or("").to_ascii_uppercase().as_str() {
        "B" | "BASE64" => return inline(base64_bytes(&line.value), stated, false),
        "QUOTED-PRINTABLE" | "Q" => return inline(quoted_printable(&line.value), stated, false),
        _ => {}
    }
    let value = unescape(&line.value);
    let value = value.trim();
    if starts_with_ci(value, "data:") {
        let (named, bytes) = data_uri(value.get(5..).unwrap_or(""))?;
        return inline(bytes, named.or(stated), false);
    }
    if value.starts_with("-----BEGIN") || line.param("VALUE").is_some_and(|v| v.eq_ignore_ascii_case("text")) {
        return inline(value.as_bytes().to_vec(), stated, true);
    }
    let uri = checked_uri(value)?;
    let media_type = match stated {
        Some(KeyKind::X509) => ContactKey::X509_MEDIA_TYPE,
        Some(KeyKind::OpenPgp) => ContactKey::OPENPGP_MEDIA_TYPE,
        _ => "",
    };
    Some(ContactKey { kind: KeyKind::Uri, uri, media_type: media_type.into(), ..Default::default() })
}

fn starts_with_ci(s: &str, start: &str) -> bool {
    s.get(..start.len()).is_some_and(|head| head.eq_ignore_ascii_case(start))
}

/// A key held in the card itself, of the kind it says or, when it says none, of the kind its bytes
/// show (a PEM certificate or a DER sequence; an armoured key or an OpenPGP public-key packet).
/// A PEM certificate written as text (`as_text`) is kept as the DER it wraps; bytes are kept as
/// they are, so that what is written reads back as itself.
fn inline(bytes: Vec<u8>, stated: Option<KeyKind>, as_text: bool) -> Option<ContactKey> {
    let text = std::str::from_utf8(&bytes).ok().map(str::trim_start);
    let pem = as_text && text.is_some_and(|t| t.starts_with(PEM_BEGIN));
    let kind = stated.or_else(|| match (text, bytes.first()) {
        (Some(t), _) if t.starts_with(PEM_BEGIN) => Some(KeyKind::X509),
        (Some(t), _) if t.starts_with(PGP_BEGIN) => Some(KeyKind::OpenPgp),
        (_, Some(0x30)) => Some(KeyKind::X509),
        // An OpenPGP public-key packet, in the old format (tag 6) or the new one.
        (_, Some(0x98..=0x9b | 0xc6)) => Some(KeyKind::OpenPgp),
        _ => None,
    })?;
    let data = match (kind, text) {
        (KeyKind::X509, Some(t)) if pem => {
            let body = t.get(PEM_BEGIN.len()..).unwrap_or("");
            base64_bytes(body.split(PEM_END).next().unwrap_or(""))
        }
        _ => bytes,
    };
    (!data.is_empty() && data.len() <= ContactKey::MAX_BYTES).then(|| ContactKey { kind, data, ..Default::default() })
}

/// The kind its media type names and the bytes of a `data:` URI (after the scheme):
/// `application/pgp-keys;base64,mQIN…` or percent-encoded text.
fn data_uri(rest: &str) -> Option<(Option<KeyKind>, Vec<u8>)> {
    let (head, data) = rest.split_once(',')?;
    let mut parts = head.split(';');
    let named = parts.next().and_then(kind_named);
    let base64 = parts.any(|p| p.trim().eq_ignore_ascii_case("base64"));
    Some((named, if base64 { base64_bytes(data) } else { percent_decoded(data) }))
}

/// `%41` as `A`; a `%` without two hex digits after it is kept.
fn percent_decoded(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let hex = |at: usize| bytes.get(at).and_then(|b| char::from(*b).to_digit(16)).and_then(|d| u8::try_from(d).ok());
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        if b == b'%'
            && let (Some(high), Some(low)) = (hex(i.saturating_add(1)), hex(i.saturating_add(2)))
        {
            out.push(high.saturating_mul(16).saturating_add(low));
            i = i.saturating_add(3);
        } else {
            out.push(b);
            i = i.saturating_add(1);
        }
    }
    out
}

/// The address when it is one a key may be at: a known scheme, no white space or control
/// characters or backslashes, not too long.
pub(crate) fn checked_uri(value: &str) -> Option<String> {
    let (scheme, rest) = value.split_once(':')?;
    let known = SCHEMES.iter().any(|s| s.eq_ignore_ascii_case(scheme));
    // (A backslash would be read back as an escape.)
    let clean = !rest.is_empty() && !value.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\');
    (known && clean && value.len() <= ContactKey::MAX_URI).then(|| value.to_string())
}

/// The `KEY` line of a key, as vCard 3.0 writes it; `None` for a key that would not read back as
/// itself (an unknown kind, no bytes or too many, an address that isn't one).
pub(crate) fn line(key: &ContactKey) -> Option<String> {
    let encoded = |kind: &str| {
        (!key.data.is_empty() && key.data.len() <= ContactKey::MAX_BYTES)
            .then(|| format!("KEY;TYPE={kind};ENCODING=b:{}", mailcraft_charset::base64::STANDARD.encode(&key.data)))
    };
    match key.kind {
        KeyKind::X509 => encoded("X509"),
        KeyKind::OpenPgp => encoded("PGP"),
        KeyKind::Uri => {
            let uri = checked_uri(&key.uri)?;
            let kind = match key.media_type.as_str() {
                ContactKey::X509_MEDIA_TYPE => ";TYPE=X509",
                ContactKey::OPENPGP_MEDIA_TYPE => ";TYPE=PGP",
                _ => "",
            };
            Some(format!("KEY;VALUE=uri{kind}:{uri}"))
        }
        KeyKind::Unknown => None,
    }
}
