// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-test TLS trust: a certificate authority made at run time, which the system's verifier is
//! told to trust beside the system's own (`crate::tls`) on this thread for one test. Production
//! trust is unchanged: there is no such hook outside tests.
#![allow(clippy::unwrap_used)] // Test fixture generation only; no production trust bypass.
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use std::{cell::RefCell, sync::Arc};

thread_local! { static ROOTS: RefCell<Option<Vec<CertificateDer<'static>>>> = const { RefCell::new(None) }; }

/// The roots a test trusts beside the system's, when it trusts any.
pub(crate) fn trusted_roots() -> Option<Vec<CertificateDer<'static>>> {
    ROOTS.with(|r| r.borrow().clone())
}

/// A certificate authority made for one test, with a name of its own.
pub(crate) struct Authority {
    issuer: CertifiedIssuer<'static, KeyPair>,
}

impl Authority {
    pub(crate) fn new(name: &str) -> Self {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
        validity(&mut params);
        Authority { issuer: CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap() }
    }

    pub(crate) fn root(&self) -> CertificateDer<'static> {
        self.issuer.der().clone()
    }

    /// A server's settings, with a certificate for `names` that this authority issued.
    pub(crate) fn server(&self, names: &[&str]) -> Arc<ServerConfig> {
        self.server_for(names, vec![ExtendedKeyUsagePurpose::ServerAuth]).0
    }

    /// As [`Authority::server`], with the certificate good for `uses` (and not necessarily for
    /// servers), and the certificate's DER as well.
    pub(crate) fn server_for(&self, names: &[&str], uses: Vec<ExtendedKeyUsagePurpose>) -> (Arc<ServerConfig>, Vec<u8>) {
        let mut params = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
        params.distinguished_name.push(DnType::CommonName, names.first().copied().unwrap_or("server"));
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = uses;
        validity(&mut params);
        let key = KeyPair::generate().unwrap();
        let cert = params.signed_by(&key, &self.issuer).unwrap();
        let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone(), self.root()], PrivatePkcs8KeyDer::from(key.serialize_der()).into())
            .unwrap();
        (Arc::new(config), cert.der().to_vec())
    }
}

/// A year either side of today: short enough for the systems that refuse a server certificate
/// good for more than 825 days.
fn validity(params: &mut CertificateParams) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let year = 1970 + i32::try_from(now / 31_556_952).unwrap();
    params.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
    params.not_after = rcgen::date_time_ymd(year + 1, 1, 1);
}

/// Runs `run` with `roots` trusted beside the system's on this thread.
pub(crate) fn trusting<T>(roots: Vec<CertificateDer<'static>>, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<Vec<CertificateDer<'static>>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            ROOTS.with(|r| *r.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(ROOTS.with(|r| r.replace(Some(roots))));
    run()
}

/// Runs `run` with a fresh authority trusted, handing it a server's settings for this machine.
pub(crate) fn with_tls<T>(run: impl FnOnce(Arc<ServerConfig>) -> T) -> T {
    let authority = Authority::new("MailCraft Test Authority");
    let server = authority.server(&["localhost", "127.0.0.1"]);
    trusting(vec![authority.root()], || run(server))
}
