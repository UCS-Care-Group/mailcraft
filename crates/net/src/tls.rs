// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The one check of a server's certificate, for mail and for pictures alike (#501).
//!
//! Certificates are checked by the operating system's own verifier (`rustls-platform-verifier`):
//! on Windows and macOS the system decides, with the authorities it trusts, and may fetch an
//! intermediate certificate a server left out from the address its certificate names (as Outlook
//! and the browsers do); on Linux and FreeBSD the system's certificate store is read and the chain
//! is checked against it. Either way the certificate must match the server's name.
//!
//! There is no second set of roots to fall back on: when the system's verifier can't be set up
//! (a Linux or FreeBSD with no certificate store at all), every secure connection is refused and
//! says why, rather than quietly trusting a list the owner's system doesn't.

use std::sync::{Arc, OnceLock};

use rustls::ClientConfig;
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::CryptoProvider;
use rustls_platform_verifier::Verifier;

/// Who a connection is for: the settings differ only in what is said during the handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// IMAP, SMTP, and the web servers the mail goes through (Graph, JMAP, OAuth, DNS over HTTPS).
    Mail,
    /// A picture's web server: told that HTTP/1.1 is the one way of speaking that is spoken here.
    Picture,
}

/// The TLS settings for `purpose`: the `ring` provider, TLS 1.2 and 1.3, and the system's verifier.
pub(crate) fn client_config(purpose: Purpose) -> Result<Arc<ClientConfig>, String> {
    #[cfg(test)]
    if let Some(roots) = crate::tls_test_support::trusted_roots() {
        // A test's own certificate authority, trusted beside the system's through the same verifier.
        let provider = provider();
        let verifier = Verifier::new_with_extra_roots(roots, provider.clone()).map_err(|e| e.to_string())?;
        return build(provider, Arc::new(verifier), purpose);
    }
    static MAIL: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    static PICTURE: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let cache = match purpose {
        Purpose::Mail => &MAIL,
        Purpose::Picture => &PICTURE,
    };
    if let Some(config) = cache.get() {
        return Ok(config.clone());
    }
    // Not kept when it fails: a certificate store installed meanwhile is found next time.
    let provider = provider();
    let verifier = Verifier::new(provider.clone()).map_err(|e| format!("the system's certificate check couldn't be set up ({e})"))?;
    let config = build(provider, Arc::new(verifier), purpose)?;
    Ok(cache.get_or_init(|| config).clone())
}

/// The system's verifier, as `client_config` builds it (and with a test's own roots in a test).
fn system_verifier(provider: &Arc<CryptoProvider>) -> Result<Arc<dyn ServerCertVerifier>, String> {
    #[cfg(test)]
    if let Some(roots) = crate::tls_test_support::trusted_roots() {
        let verifier = Verifier::new_with_extra_roots(roots, provider.clone()).map_err(|e| e.to_string())?;
        return Ok(Arc::new(verifier));
    }
    static SYSTEM: OnceLock<Arc<Verifier>> = OnceLock::new();
    if let Some(verifier) = SYSTEM.get() {
        return Ok(verifier.clone());
    }
    let verifier = Verifier::new(provider.clone()).map_err(|e| format!("the system's certificate check couldn't be set up ({e})"))?;
    Ok(SYSTEM.get_or_init(|| Arc::new(verifier)).clone())
}

/// The TLS settings for a server whose owner may have chosen to trust one certificate by its
/// fingerprint (`pin`): the system's check, and that one certificate when the system doesn't
/// trust it but it is in date and for the server's name (`crate::pinned`). With the settings
/// comes the verifier, which says afterwards why a certificate was refused. Built for each
/// connection: the verifier holds what it saw.
pub(crate) fn pinned_config(pin: Option<[u8; 32]>) -> Result<(Arc<ClientConfig>, Arc<crate::pinned::Pinned>), String> {
    let provider = provider();
    let verifier = Arc::new(crate::pinned::Pinned::new(system_verifier(&provider)?, pin));
    let config = build(provider, verifier.clone(), Purpose::Mail)?;
    Ok((config, verifier))
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn build(provider: Arc<CryptoProvider>, verifier: Arc<dyn ServerCertVerifier>, purpose: Purpose) -> Result<Arc<ClientConfig>, String> {
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        // "Dangerous" only in rustls's naming: the verifier is the system's own, and checks the
        // chain, the dates, the purpose and the server's name.
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    if purpose == Purpose::Picture {
        // The one way of speaking that is spoken here, said during the handshake: a server that
        // has others as well then knows which to use.
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
    }
    Ok(Arc::new(config))
}
