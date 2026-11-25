// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The local Sieve check, from any bytes: a whole script and a block for MailCraft's script, with
//! and without the server's extensions. Every problem's bytes are within the script, on character
//! boundaries.
#![no_main]

use mailcraft_engine::sieve_check::{Problem, check, check_block};

fn sound(script: &str, problems: &[Problem]) {
    for p in problems {
        assert!(p.at.start <= p.at.end && p.at.end <= script.len());
        assert!(script.is_char_boundary(p.at.start) && script.is_char_boundary(p.at.end));
        assert!(p.line >= 1 && p.column >= 1);
        let _ = p.what().to_string();
    }
}

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let Ok(script) = std::str::from_utf8(data) else { return };
    let server = vec!["fileinto".to_string(), "vacation".to_string(), "vnd.example.x".to_string()];
    for result in [check(script, None), check(script, Some(&server)), check_block(script, None), check_block(script, Some(&server))] {
        match result {
            Ok(c) => sound(script, &c.warnings),
            Err(p) => sound(script, &p),
        }
    }
});
