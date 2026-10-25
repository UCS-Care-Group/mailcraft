// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The app proves it holds the control key before the MCP bridge sends the key (or any request):
//! the bridge sends a random nonce (`auth.challenge`), the app answers HMAC-SHA256 over it keyed
//! with the control key. A program that only listens on the port, and has not read the key file,
//! cannot answer, and so is never sent the key. See `docs/control-protocol.md`, Keys.

/// What the HMAC covers: this label, a space, and the nonce as it was sent.
const LABEL: &str = "mailcraft-control-v1 ";

/// The shortest and longest nonce (hex digits) the app answers: the bridge sends 64.
const NONCE_LEN: std::ops::RangeInclusive<usize> = 16..=128;

/// A new nonce: 32 random bytes from the operating system, as hex. `None` without them (and in
/// the browser, where nothing bridges to the app: this module only has to build there).
#[cfg(not(target_arch = "wasm32"))]
pub fn new_nonce() -> Option<String> {
    let mut b = [0u8; 32];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b).ok()?;
    Some(hex(&b))
}

#[cfg(target_arch = "wasm32")]
pub fn new_nonce() -> Option<String> {
    None
}

/// Whether `nonce` is one the app answers: hex digits only, of a bounded length.
pub fn valid_nonce(nonce: &str) -> bool {
    NONCE_LEN.contains(&nonce.len()) && nonce.bytes().all(|c| c.is_ascii_hexdigit())
}

/// The proof of holding `key` for `nonce`, as hex.
#[cfg(not(target_arch = "wasm32"))]
pub fn proof(key: &str, nonce: &str) -> String {
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    hex(ring::hmac::sign(&k, message(nonce).as_bytes()).as_ref())
}

/// Whether `given` (hex) is the proof of holding `key` for `nonce`; compared in constant time.
/// An empty key proves nothing.
#[cfg(not(target_arch = "wasm32"))]
pub fn proof_ok(key: &str, nonce: &str, given: &str) -> bool {
    if key.is_empty() {
        return false;
    }
    let Some(tag) = unhex(given) else { return false };
    let k = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key.as_bytes());
    ring::hmac::verify(&k, message(nonce).as_bytes(), &tag).is_ok()
}

#[cfg(target_arch = "wasm32")]
pub fn proof(_key: &str, _nonce: &str) -> String {
    String::new()
}

/// In the browser nothing is proved: never true.
#[cfg(target_arch = "wasm32")]
pub fn proof_ok(_key: &str, _nonce: &str, _given: &str) -> bool {
    false
}

#[cfg(not(target_arch = "wasm32"))]
fn message(nonce: &str) -> String {
    format!("{LABEL}{nonce}")
}

#[cfg(not(target_arch = "wasm32"))]
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() || !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|d| u8::from_str_radix(d, 16).ok())).collect()
}
