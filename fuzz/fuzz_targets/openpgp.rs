// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A message checked for its OpenPGP Digital signature, its Public keys read, and the same bytes
//! read as key material (public and secret) and as an `Autocrypt:` header value.
#![no_main]

use std::sync::LazyLock;

use mailcraft_openpgp::{KeyRing, PublicKey};

/// RFC 9580 Appendix A.3: the sample version 6 certificate, for signatures to be looked up in.
const CERT: &str = "-----BEGIN PGP PUBLIC KEY BLOCK-----

xioGY4d/4xsAAAAg+U2nu0jWCmHlZ3BqZYfQMxmZu52JGggkLq2EVD34laPCsQYf
GwoAAABCBYJjh3/jAwsJBwUVCg4IDAIWAAKbAwIeCSIhBssYbE8GCaaX5NUt+mxy
KwwfHifBilZwj2Ul7Ce62azJBScJAgcCAAAAAK0oIBA+LX0ifsDm185Ecds2v8lw
gyU2kCcUmKfvBXbAf6rhRYWzuQOwEn7E/aLwIwRaLsdry0+VcallHhSu4RN6HWaE
QsiPlR4zxP/TP7mhfVEe7XWPxtnMUMtf15OyA51YBM4qBmOHf+MZAAAAIIaTJINn
+eUBXbki+PSAld2nhJh/LVmFsS+60WyvXkQ1wpsGGBsKAAAALAWCY4d/4wKbDCIh
BssYbE8GCaaX5NUt+mxyKwwfHifBilZwj2Ul7Ce62azJAAAAAAQBIKbpGG2dWTX8
j+VjFM21J0hqWlEg+bdiojWnKfA5AQpWUWtnNwDEM0g12vYxoWM8Y81W+bHBw805
I8kWVkXU6vFOi+HWvv/ira7ofJu16NnoUkhclkUrk0mXubZvyl4GBg==
-----END PGP PUBLIC KEY BLOCK-----
";

static KEYS: LazyLock<KeyRing> = LazyLock::new(|| PublicKey::parse(CERT.as_bytes()).unwrap_or_default().into_iter().collect());

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    let _ = mailcraft_openpgp::verify(data, "someone@example.org", &KEYS, 4_102_444_800, |_, _| true);
    let _ = mailcraft_openpgp::keys_in_message(data);
    let _ = PublicKey::parse(data);
    let _ = mailcraft_openpgp::parse_autocrypt(&String::from_utf8_lossy(data));
    let _ = mailcraft_openpgp::secret::read(data);
    let _ = mailcraft_openpgp::secret::holds_secret(data);
});
