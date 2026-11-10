// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Fuzzes the NTLM sign-in and the choice of it (#252, E4): a hostile server's CHALLENGE, as
//! arbitrary bytes (Base64-encoded, and as raw text), and its `WWW-Authenticate` headers.
//!
//! Invariants: nothing panics, and a message made from whatever came is a well-formed header value.

#![no_main]

use mailcraft_ews::auth;
use mailcraft_ews::ntlm::{Context, Credentials, Handshake};

/// Standard Base64 (RFC 4648), for the bytes the fuzzer gives.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let at = |n: usize| u32::from(chunk.get(n).copied().unwrap_or(0));
        let group = (at(0) << 16) | (at(1) << 8) | at(2);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((group >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(credentials) = Credentials::new("EXAMPLE\\ada", "pw") else { return };
    let text = String::from_utf8_lossy(data).into_owned();
    // Whether the server's time and a binding are there varies with the input.
    let binding = [data.first().copied().unwrap_or(0); 32];
    let context = Context {
        client_challenge: [7; 8],
        unix_seconds: u64::from(data.get(1).copied().unwrap_or(0)) << 32,
        host: "mail.example.org",
        channel_binding: (data.first().copied().unwrap_or(0) % 2 == 0).then_some(&binding[..]),
    };
    for token in [base64(data), text.clone()] {
        let (handshake, _) = Handshake::negotiate(&credentials);
        if let Ok(answer) = handshake.authenticate(&token, &context) {
            assert!(answer.expose().starts_with("NTLM "));
            assert!(!answer.expose().chars().any(char::is_control));
        }
    }
    let headers: Vec<String> = text.split('\n').map(str::to_string).collect();
    let _ = auth::choose(&headers);
    let _ = auth::ntlm_token(&headers);
});
