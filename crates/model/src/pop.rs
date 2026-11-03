// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Telling a POP server from an IMAP one by its name and port alone, so that Add Account can
//! say "MailCraft has no POP yet" instead of trying IMAP on it and showing a TLS error (#402).
//!
//! Pure: nothing is looked up or sent. A host name typed by the owner is hostile text, so it
//! is only compared, never sliced at a byte offset.

/// The usual POP3 ports: 110 (STARTTLS or none) and 995 (TLS from the first byte).
const PORTS: [u16; 2] = [110, 995];

/// Longest host name looked at (RFC 1035 allows 253); anything longer is not taken for POP.
const MAX_HOST: usize = 253;

/// Whether an incoming server looks like POP: port 110 or 995, or a host whose first label is
/// `pop`, `pop3` or `pop3s` (any case). `port` 0 means "the usual one" and says nothing.
pub fn looks_like_pop(host: &str, port: u16) -> bool {
    if PORTS.contains(&port) {
        return true;
    }
    let host = host.trim();
    if host.len() > MAX_HOST {
        return false;
    }
    let first = host.split('.').next().unwrap_or("");
    ["pop", "pop3", "pop3s"].iter().any(|label| first.eq_ignore_ascii_case(label))
}
