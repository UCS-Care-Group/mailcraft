// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Percent-encoding (RFC 3986 §2.1): which bytes of a part of an address are written as `%XX`.

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Which part of an address is being written.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    UserInfo,
    Path,
    Query,
    Fragment,
    /// What follows `mailto:` or `tel:`.
    Opaque,
}

impl Part {
    /// Whether an ASCII byte (a printable one) has to be written as `%XX` in this part: the
    /// sets of the WHATWG URL Standard, which is what browsers write.
    fn encodes(self, b: u8) -> bool {
        let fragment = matches!(b, b'"' | b'<' | b'>' | b'`');
        let query = matches!(b, b'"' | b'#' | b'<' | b'>' | b'\'');
        let path = query && b != b'\'' || matches!(b, b'?' | b'`' | b'{' | b'}');
        match self {
            Part::Fragment => fragment,
            Part::Query => query,
            Part::Path => path,
            Part::UserInfo => path || matches!(b, b'/' | b';' | b'=' | b'@' | b'[' | b']' | b'|'),
            Part::Opaque => fragment || matches!(b, b'{' | b'}' | b'|' | b'^'),
        }
    }
}

fn hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}

/// `text` written for `part`: spaces, controls, non-ASCII bytes (as UTF-8) and the part's
/// special characters as `%XX`; a `%` that starts a `%XX` stays, any other is written `%25`.
pub(crate) fn push(out: &mut String, text: &str, part: Part) {
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let escape = match b {
            b'%' => !(bytes.get(i + 1).is_some_and(|&x| hex(x)) && bytes.get(i + 2).is_some_and(|&x| hex(x))),
            0..=0x20 | 0x7f..=0xff => true,
            _ => part.encodes(b),
        };
        if escape {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        } else {
            out.push(char::from(b));
        }
    }
}

/// `text` for a query string or a form: everything but letters, digits and `-._*` as `%XX`
/// (a space is `%20`).
pub fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for &b in text.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'*') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
    out
}
