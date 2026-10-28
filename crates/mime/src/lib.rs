// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft Internet messages: reading and writing RFC 5322 + MIME (`.eml`) and mbox files.
//!
//! [`parse()`] turns the bytes of a message into a [`mailcraft_model::Message`]; [`write()`] does the
//! reverse. [`parse_mbox`] and [`write_mbox`] do the same for a file of many messages, and
//! [`parse_mailto`] reads the link that starts a new one. Written from the public RFCs: 5322
//! (messages), 2045–2049 (MIME), 2047 (encoded-words), 2231 (parameter values), 4155 (mbox) and
//! 6068 (`mailto:` links).
//!
//! Mail is hostile input. Nothing here fails or panics on any bytes: what can't be read is
//! skipped, sizes and nesting are capped, and the work is linear in the input. Nothing written
//! can be bent into extra header fields by the values it is given.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod address;
mod charset;
mod header;
mod mailto;
mod mbox;
mod media;
mod parse;
pub mod rich;
mod transfer;
mod words;
mod write;

#[cfg(test)]
mod fields_tests;
#[cfg(test)]
mod private_headers_tests;
#[cfg(test)]
mod tests;

pub use mailto::{Mailto, is_mailto, parse_mailto};

/// The header in which a message opened from a file keeps that file's SHA-256 (#505), so that
/// opening the file again finds it. MailCraft's own: never read from a message (a sender could
/// write one) and never written into one.
pub const OPENED_FILE_HEADER: &str = "X-MailCraft-Opened-File-SHA256";

/// The header in which the copy that MailCraft puts on a mail server, of a message moved or
/// copied there from elsewhere, carries that copy's marker (#451): what tells it that copy from
/// the server's own messages, which may look the same. Written into that copy alone
/// ([`write_placed`]), never from a message's kept headers into anything else.
pub const PLACE_HEADER: &str = "X-MailCraft-Place";

/// `message` as [`write()`] writes it, with [`PLACE_HEADER`] carrying `marker` first.
pub fn write_placed(message: &mailcraft_model::Message, opts: &WriteOptions, marker: &str) -> Vec<u8> {
    let mut out = Vec::new();
    header::write_field(&mut out, PLACE_HEADER, marker);
    out.extend(write(message, opts));
    out
}

/// Whether the header block `header` has [`PLACE_HEADER`] carrying `marker`.
pub fn carries_place(header: &[u8], marker: &str) -> bool {
    !marker.is_empty() && header::split(header, true).0.0.iter().any(|f| f.name.eq_ignore_ascii_case(PLACE_HEADER) && f.value == marker)
}
pub use mbox::{MboxMessages, mbox_messages, parse_mbox, write_mbox};
pub use media::media_type_for;
pub use parse::{MAX_CATEGORIES, MAX_RECIPIENTS, parse};
pub use words::{decode_words, encode_words};
pub use write::{write, written_message_id};

/// How [`write()`] and [`write_mbox`] write a message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriteOptions {
    /// Write the Bcc header (saving a draft); leave false when sending.
    pub include_bcc: bool,
    /// Zone for the Date header, minutes east of UTC.
    pub offset_minutes: i32,
}

/// A new `Message-ID`, without angle brackets: `mc.<time>.<entropy>@<domain>`, the time and the
/// entropy in base 36. The domain is `mailcraft.local` when `domain` is empty or isn't a host
/// name. The caller brings the entropy (random bits), so this crate asks the system for nothing.
pub fn new_message_id(domain: &str, now: mailcraft_time::Timestamp, entropy: u64) -> String {
    let domain = domain.trim();
    let label = |l: &str| {
        !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !l.starts_with('-') && !l.ends_with('-')
    };
    let domain = if !domain.is_empty() && domain.len() <= 253 && domain.split('.').all(label) {
        domain.to_ascii_lowercase()
    } else {
        "mailcraft.local".to_string()
    };
    format!("mc.{}.{}@{domain}", base36(u64::try_from(now).unwrap_or(0)), base36(entropy))
}

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut digits = Vec::new();
    loop {
        digits.push(DIGITS.get((n % 36) as usize).copied().unwrap_or(b'0'));
        n /= 36;
        if n == 0 {
            break;
        }
    }
    digits.iter().rev().map(|b| char::from(*b)).collect()
}

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn message_ids() {
        assert_eq!(new_message_id("example.org", 1_057_049_557, 12_345), "mc.hhc8np.9ix@example.org");
        assert_eq!(new_message_id(" Mail.Example.ORG ", 0, 0), "mc.0.0@mail.example.org");
        assert_eq!(new_message_id("example.org", -5, u64::MAX), "mc.0.3w5e11264sgsf@example.org");
        for bad in ["", "  ", "exa mple.org", "a..b", ".org", "org.", "-a.org", "a-.org", "a@b.org", "<x>", "é.org", "a\r\n.org"] {
            assert_eq!(new_message_id(bad, 1, 1), "mc.1.1@mailcraft.local", "{bad:?}");
        }
        assert_eq!(new_message_id(&"a".repeat(64), 1, 1), "mc.1.1@mailcraft.local");
        assert_eq!(new_message_id("localhost", 36, 35), "mc.10.z@localhost");
    }

    #[test]
    fn an_id_survives_a_message() {
        let id = new_message_id("example.org", 1_700_000_000, 0xDEAD_BEEF);
        let m = mailcraft_model::Message { message_id: id.clone(), ..Default::default() };
        assert_eq!(parse(&write(&m, &WriteOptions::default())).message_id, id);
    }
}
