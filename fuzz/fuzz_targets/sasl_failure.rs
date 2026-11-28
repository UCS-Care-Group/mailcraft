// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The reasons a hostile mail server gives for refusing a token sign-in (RFC 7628 §3.2.2) are
//! bounded and never panic, as given and in base64.
#![no_main]

use mailcraft_charset::base64::STANDARD;
use mailcraft_proto::SaslFailure;

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = SaslFailure::read(data);
    if let Some(failure) = SaslFailure::read(STANDARD.encode(data).as_bytes()) {
        assert!(failure.status.len() <= 512 && failure.scope.len() <= 512 && failure.openid_configuration.len() <= 512);
        assert!(!failure.scope.contains(['\r', '\n', '"', '\\']));
    }
});
