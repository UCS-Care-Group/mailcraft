// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The certification authorities this computer trusts **for mail**, for checking S/MIME Digital
//! signatures (#30): read from the system's own store, never fetched, and handed to the engine
//! (`Session::set_trust_anchors`), which never reads a store itself. Never the set trusted for web
//! sites: an authority may be trusted for one and not the other.
//!
//! - Linux and the other Unix systems: the bundle of authorities trusted for mail that the
//!   system's trust store extracts (p11-kit's `email-ca-bundle.pem`), at the places the
//!   distributions put it. A system that extracts none (Debian's `ca-certificates` holds the web
//!   sites' set alone) has none here.
//! - FreeBSD: none. Its authorities (`certctl`'s in the base system, `ca_root_nss`'s
//!   `/usr/local/share/certs/ca-root-nss.crt` from ports, and p11-kit's, which reads that) are
//!   the ones trusted for web sites, and nothing there keeps a set for mail.
//! - Windows: the authorities of the Trusted Root Certification Authorities store whose uses,
//!   as Windows keeps them, include secure mail (or aren't limited).
//! - macOS: none yet; the keychain's trust settings for mail aren't read. Signers there are "not
//!   verified", and the result says no authority for mail could be read.

/// The most certificates taken from the system.
const MAX_ANCHORS: usize = 4_000;
/// The largest certificate taken.
const MAX_ANCHOR_BYTES: usize = 64 * 1024;
/// The largest bundle read.
#[cfg(all(unix, not(target_os = "macos")))]
const MAX_BUNDLE_BYTES: u64 = 16 * 1024 * 1024;

/// Where the distributions keep the bundle of authorities trusted for mail.
#[cfg(all(unix, not(target_os = "macos")))]
pub const MAIL_BUNDLES: [&str; 3] = [
    // Arch, and others on p11-kit's `trust extract-compat`.
    "/etc/ca-certificates/extracted/email-ca-bundle.pem",
    // Fedora, Red Hat, CentOS, openSUSE's `ca-certificates`.
    "/etc/pki/ca-trust/extracted/pem/email-ca-bundle.pem",
    "/var/lib/ca-certificates/email-ca-bundle.pem",
];

/// The system's certification authorities trusted for mail, each in DER. Empty when there are
/// none, or they can't be read; the log says how many were found (never which).
pub fn system_trust_anchors() -> Vec<Vec<u8>> {
    let found = read();
    log::info!("{} certification authorities trusted for mail, for checking Digital signatures", found.len());
    found.into_iter().filter(|c| c.len() <= MAX_ANCHOR_BYTES).take(MAX_ANCHORS).collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read() -> Vec<Vec<u8>> {
    MAIL_BUNDLES.iter().find_map(|path| bundle(std::path::Path::new(path))).unwrap_or_default()
}

/// The certificates of the PEM bundle at `path`, when it is there and can be read.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn bundle(path: &std::path::Path) -> Option<Vec<Vec<u8>>> {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject;
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_BUNDLE_BYTES).read_to_end(&mut bytes).ok()?;
    let certs: Vec<Vec<u8>> = CertificateDer::pem_slice_iter(&bytes).take(MAX_ANCHORS).filter_map(Result::ok).map(|c| c.as_ref().to_vec()).collect();
    (!certs.is_empty()).then_some(certs)
}

#[cfg(windows)]
fn read() -> Vec<Vec<u8>> {
    use schannel::cert_context::ValidUses;
    /// Secure mail (RFC 5280's id-kp-emailProtection).
    const EMAIL_PROTECTION: &str = "1.3.6.1.5.5.7.3.4";
    let store = match schannel::cert_store::CertStore::open_current_user("ROOT") {
        Ok(store) => store,
        Err(e) => {
            log::warn!("the system's certification authorities couldn't be read for checking Digital signatures: {e}");
            return Vec::new();
        }
    };
    store
        .certs()
        .filter(|c| match c.valid_uses() {
            Ok(ValidUses::All) => true,
            Ok(ValidUses::Oids(oids)) => oids.iter().any(|o| o == EMAIL_PROTECTION),
            Err(_) => false,
        })
        .take(MAX_ANCHORS)
        .map(|c| c.to_der().to_vec())
        .collect()
}

#[cfg(any(target_os = "macos", not(any(unix, windows))))]
fn read() -> Vec<Vec<u8>> {
    // ponytail: the keychain's trust settings for S/MIME aren't read yet; add them through
    // security-framework's trust settings when it can ask for the S/MIME policy safely.
    Vec::new()
}
