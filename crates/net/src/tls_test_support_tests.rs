// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Per-test TLS trust, with a runtime-generated loopback certificate; production trust is unchanged.
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::{cell::RefCell, sync::Arc};
thread_local! { static CLIENT: RefCell<Option<Arc<ClientConfig>>> = const { RefCell::new(None) }; }
pub(crate) fn client_config() -> Option<Arc<ClientConfig>> {
    CLIENT.with(|c| c.borrow().clone())
}
#[allow(clippy::unwrap_used)] // Test fixture generation only; no production trust bypass.
pub(crate) fn with_tls<T>(run: impl FnOnce(Arc<ServerConfig>) -> T) -> T {
    let key = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
    let cert = key.cert.der().clone();
    let mut roots = RootCertStore::empty();
    roots.add(cert.clone()).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], rustls::pki_types::PrivatePkcs8KeyDer::from(key.signing_key.serialize_der()).into())
        .unwrap();
    struct Restore(Option<Arc<ClientConfig>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CLIENT.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CLIENT.with(|c| c.replace(Some(Arc::new(client)))));
    run(Arc::new(server))
}
