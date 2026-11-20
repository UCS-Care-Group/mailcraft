// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Telling a Digitally signed or Encrypted message by what was read of it (#30): the parts that
//! S/MIME (RFC 8551) and PGP/MIME (RFC 3156) carry, and OpenPGP armour (RFC 9580 §6.2) in text.
//! What a message's own MIME says is read by `mailcraft_mime::protection`; this is for a message
//! of which only the parsed parts are here.

/// Most bytes of a text looked through for armour: a message whose armour starts further in
/// isn't taken for an inline-PGP one.
pub const MAX_ARMOUR_SCAN: usize = 4 << 20;

/// The lines that open an OpenPGP message or a cleartext-signed one.
const ARMOUR: [&[u8]; 2] = [b"-----BEGIN PGP MESSAGE-----", b"-----BEGIN PGP SIGNED MESSAGE-----"];

/// Whether `text` has OpenPGP armour (`-----BEGIN PGP MESSAGE-----` or `-----BEGIN PGP SIGNED
/// MESSAGE-----`) as a line of its own, at the start of the line, in its first
/// [`MAX_ARMOUR_SCAN`] bytes. A line quoted in a reply (`> -----BEGIN PGP…`) or mentioned in a
/// sentence is not armour.
pub fn has_armour(text: &[u8]) -> bool {
    has_armour_within(text, MAX_ARMOUR_SCAN)
}

/// [`has_armour`] in the first `most` bytes of `text` alone.
pub fn has_armour_within(text: &[u8], most: usize) -> bool {
    let text = text.get(..most).unwrap_or(text);
    text.split(|b| *b == b'\n').any(|line| {
        let line = line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line);
        let end = line.iter().rposition(|b| !matches!(b, b' ' | b'\t' | b'\r')).map_or(0, |i| i.saturating_add(1));
        let line = line.get(..end).unwrap_or_default();
        ARMOUR.contains(&line)
    })
}

/// Whether a part of media type `mime` named `name` carries a Digital signature or encrypted
/// content: `application/pkcs7-signature`, `application/pkcs7-mime` (and their `x-` forms),
/// `application/pgp-signature`, `application/pgp-encrypted`, or a file named `smime.p7s` or
/// `smime.p7m`.
pub fn is_protection_part(mime: &str, name: &str) -> bool {
    let mime = mime.trim().to_ascii_lowercase();
    let mime = mime.split(';').next().unwrap_or_default().trim();
    let name = name.trim().to_ascii_lowercase();
    matches!(
        mime,
        "application/pkcs7-signature"
            | "application/x-pkcs7-signature"
            | "application/pkcs7-mime"
            | "application/x-pkcs7-mime"
            | "application/pgp-signature"
            | "application/pgp-encrypted"
    ) || name == "smime.p7s"
        || name == "smime.p7m"
}
