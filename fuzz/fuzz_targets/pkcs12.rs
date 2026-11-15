// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A PKCS #12 file opened with a passphrase (#30 R13), and the same bytes read as certificates to
//! import and looked at for a private key.
#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = mailcraft_smime::pkcs12::open(data, "example-pass");
    let _ = mailcraft_smime::pkcs12::open(data, "");
    let _ = mailcraft_smime::pkcs12::looks_like(data);
    let _ = mailcraft_smime::import::certificates_in(data);
    let _ = mailcraft_smime::import::certificate_info(data);
    let _ = mailcraft_smime::import::holds_private_key(data);
});
