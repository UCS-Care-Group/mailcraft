// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Base64 (RFC 4648), strictly read: what a server sends as a message's `MimeContent`.

/// The bytes `text` stands for, or none when it is not base64: only the alphabet, white space
/// between the groups, and the padding at the end. At most `limit` bytes of text are looked at
/// (more is no answer).
pub(crate) fn decode(text: &str, limit: usize) -> Option<Vec<u8>> {
    if text.len() > limit {
        return None;
    }
    let mut out = Vec::with_capacity(text.len().saturating_mul(3) / 4);
    let (mut bits, mut have, mut pad) = (0u32, 0u32, 0usize);
    for b in text.bytes() {
        let value = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                pad = pad.saturating_add(1);
                continue;
            }
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        // (Nothing follows padding.)
        if pad > 0 {
            return None;
        }
        bits = (bits << 6) | u32::from(value);
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push(u8::try_from((bits >> have) & 0xff).ok()?);
            bits &= (1 << have) - 1;
        }
    }
    (pad <= 2).then_some(out)
}
