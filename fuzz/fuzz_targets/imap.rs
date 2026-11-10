// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An IMAP server that answers with any bytes, through a whole session.
#![no_main]

use std::io::{Cursor, Read, Write};

/// Reads the server's bytes; what the client writes goes nowhere.
struct Server(Cursor<Vec<u8>>);

impl Read for Server {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Server {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    // The sign-in is made of the input too, so what the client refuses to send is fuzzed as well.
    let secret = String::from_utf8_lossy(data.get(..16).unwrap_or(data)).into_owned();
    let Ok(mut imap) = mailcraft_proto::imap::Imap::connect(Server(Cursor::new(data.to_vec()))) else { return };
    let _ = imap.capabilities();
    let _ = imap.login("ada", &secret);
    let _ = imap.list();
    let _ = imap.select("INBOX", true);
    let _ = imap.uid_fetch_flags("1:*");
    let _ = imap.uid_fetch_flags_gmail("1:*");
    let _ = imap.uid_search_gmail("1:500", "category:social");
    let _ = imap.uid_fetch_sizes("1:*");
    let _ = imap.uid_fetch_message(1);
    let _ = imap.uid_fetch_header(1);
    let _ = imap.uid_store_set("1:3,7", false, &["\\Flagged"]);
    let _ = imap.uid_move("1:3", "Archive");
    let _ = imap.uid_copy("7", "Entw&APw-rfe");
    let _ = imap.uid_expunge("7");
    let _ = imap.append("Sent", &["\\Seen"], b"Subject: x\r\n\r\n");
    let _ = imap.logout();
});
