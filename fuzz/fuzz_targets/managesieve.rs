// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A ManageSieve server that answers with any bytes, through a whole session.
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
    // The sign-in and the script's name are made of the input too, so what the client refuses to
    // send is fuzzed as well.
    let secret = String::from_utf8_lossy(data.get(..16).unwrap_or(data)).into_owned();
    let name = String::from_utf8_lossy(data.get(16..32).unwrap_or_default()).into_owned();
    let Ok(mut sieve) = mailcraft_proto::managesieve::ManageSieve::connect(Server(Cursor::new(data.to_vec()))) else { return };
    let _ = sieve.refresh_capabilities();
    let _ = sieve.authenticate_plain("ada@example.org", &secret);
    let _ = sieve.authenticate_token("ada@example.org", &secret);
    let _ = sieve.list_scripts();
    let _ = sieve.get_script(&name);
    let _ = sieve.get_script("mailcraft");
    let _ = sieve.have_space(&name, 100);
    let _ = sieve.check_script(&secret);
    let _ = sieve.put_script("mailcraft", "require \"vacation\";\r\nvacation \"Away\";\r\n");
    let _ = sieve.set_active(&name);
    let _ = sieve.deactivate();
    let _ = sieve.delete_script(&name);
    let _ = sieve.logout();
});
