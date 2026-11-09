// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The certification authorities this computer trusts, for checking S/MIME Digital signatures
//! (#30): read from the system's own store (the Windows certificate store, the macOS keychain's
//! trust settings, the certificate bundle of Linux and FreeBSD), never fetched, and handed to
//! the engine (`Session::set_trust_anchors`), which never reads a store itself.

/// The most certificates taken from the system.
const MAX_ANCHORS: usize = 4_000;
/// The largest certificate taken.
const MAX_ANCHOR_BYTES: usize = 64 * 1024;

/// The system's trusted certification authorities, each in DER. Empty when the store can't be
/// read; what can't be read of it is left out, and the log says how much (never which).
pub fn system_trust_anchors() -> Vec<Vec<u8>> {
    let found = rustls_native_certs::load_native_certs();
    if !found.errors.is_empty() {
        log::warn!("{} parts of the system's certificate store couldn't be read for checking Digital signatures", found.errors.len());
    }
    found.certs.into_iter().filter(|c| c.len() <= MAX_ANCHOR_BYTES).take(MAX_ANCHORS).map(|c| c.to_vec()).collect()
}
