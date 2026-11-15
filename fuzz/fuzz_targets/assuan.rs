// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A hostile gpg-agent, and hostile S-expressions and socket files, are bounded and never panic.
#![no_main]

use std::io::{Read, Result, Write};

use mailcraft_assuan::{Digest, GpgAgent, HashAlgorithm, Keygrip, PinentryHints, Sexp};

/// The agent's side: hands out the fuzzer's bytes and swallows what the client writes.
struct Server<'a>(&'a [u8]);

impl Read for Server<'_> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let n = buf.len().min(self.0.len());
        let (now, rest) = self.0.split_at(n);
        buf[..n].copy_from_slice(now);
        self.0 = rest;
        Ok(n)
    }
}

impl Write for Server<'_> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> Result<()> {
        Ok(())
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    if let Ok(sexp) = mailcraft_assuan::sexp::parse(data) {
        let _ = mailcraft_assuan::sexp::parse(&sexp.to_canonical());
    }
    let _ = mailcraft_assuan::socket_file::parse(data);

    let Ok(mut agent) = GpgAgent::open(Server(data)) else { return };
    let Some(grip) = Keygrip::parse("0123456789ABCDEF0123456789ABCDEF01234567") else { return };
    let hints = PinentryHints { display: Some(":0".into()), ..PinentryHints::default() };
    let _ = agent.set_pinentry_hints(&hints);
    let _ = agent.version();
    let _ = agent.have_key(&grip);
    let _ = agent.key_info(&grip);
    let _ = agent.list_keys();
    let _ = agent.read_key(&grip);
    if let Some(digest) = Digest::new(HashAlgorithm::Sha256, &[0x5A; 32]) {
        let _ = agent.sign(&grip, &digest, Some("Sign a message"));
    }
    let ciphertext =
        Sexp::List(vec![Sexp::token("enc-val"), Sexp::List(vec![Sexp::token("rsa"), Sexp::List(vec![Sexp::token("a"), Sexp::Atom(vec![1, 2, 3])])])]);
    let _ = agent.decrypt(&grip, &ciphertext, None);
});
