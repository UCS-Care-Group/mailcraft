// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Trust by fingerprint (#252, E5) against a TLS web server on this machine: what the system
//! trusts, one chosen certificate, a changed one, and the certificates that are never accepted.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};

use super::*;
use crate::cert::CertificateFault;
use crate::cert::cert_tests::make;
use crate::fake::FakeHttp;
use crate::tls_test_support::{Authority, trusting};

fn year() -> i32 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    1970 + i32::try_from(now / 31_556_952).unwrap()
}

/// A TLS server on this machine that answers every request that reaches it with 200 and "hello".
struct Server {
    port: u16,
    /// Requests that got as far as being read: a handshake that failed is none.
    requests: Arc<AtomicUsize>,
    /// What the last request was, as the server read it.
    last: Arc<Mutex<String>>,
}

impl Server {
    fn start(config: Arc<ServerConfig>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (requests, last) = (Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(String::new())));
        let (count, seen) = (requests.clone(), last.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Ok(connection) = rustls::ServerConnection::new(config.clone()) else { continue };
                let mut tls = rustls::StreamOwned::new(connection, stream);
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                let mut head_end = None;
                while head_end.is_none() {
                    match tls.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            request.extend_from_slice(&buf[..n]);
                            head_end = request.windows(4).position(|w| w == b"\r\n\r\n");
                        }
                    }
                }
                if head_end.is_none() {
                    continue;
                }
                count.fetch_add(1, Ordering::SeqCst);
                *seen.lock().unwrap() = String::from_utf8_lossy(&request).into_owned();
                let _ = tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
                let _ = tls.flush();
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        });
        Server { port, requests, last }
    }

    fn url(&self) -> String {
        format!("https://localhost:{}/autodiscover/autodiscover.xml", self.port)
    }

    fn asked(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// The server settings for a self-signed certificate, and its DER.
fn self_signed(names: &[&str], from: i32, to: i32) -> (Arc<ServerConfig>, Vec<u8>) {
    let mut params = CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>()).unwrap();
    params.distinguished_name.push(rcgen::DnType::CommonName, names.first().copied().unwrap_or("server"));
    params.distinguished_name.push(rcgen::DnType::OrganizationName, "Example Org");
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = rcgen::date_time_ymd(from, 1, 1);
    params.not_after = rcgen::date_time_ymd(to, 1, 1);
    let key = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let der = cert.der().to_vec();
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(der.clone())], PrivatePkcs8KeyDer::from(key.serialize_der()).into())
        .unwrap();
    (Arc::new(config), der)
}

fn get(url: &str, trusted: &str) -> Result<Response, NetError> {
    let fields = [("Authorization", "Basic c2VjcmV0OnNlY3JldA=="), ("Accept", "text/xml")];
    send(&Request { method: "POST", url, fields: &fields, body: b"<a/>", max_response: 4096, trusted })
}

fn problem(result: Result<Response, NetError>) -> CertificateProblem {
    match result {
        Err(NetError::Certificate(problem)) => *problem,
        other => panic!("not a certificate problem: {other:?}"),
    }
}

#[test]
fn a_certificate_the_system_trusts_needs_no_fingerprint() {
    let authority = Authority::new("MailCraft Test Authority");
    let server = Server::start(authority.server(&["localhost"]));
    trusting(vec![authority.root()], || {
        let answer = get(&server.url(), "").unwrap();
        assert_eq!((answer.status, answer.body.as_slice()), (200, &b"hello"[..]));
        // A fingerprint of some other certificate changes nothing: the system's trust comes first.
        let other = cert::fingerprint(&make(&["x.example.org"], "x", None, (year() - 1, 1, 1), (year() + 1, 1, 1)));
        assert_eq!(get(&server.url(), &other).unwrap().status, 200);
    });
    assert_eq!(server.asked(), 2);
}

#[test]
fn a_certificate_the_system_does_not_trust_is_refused_with_what_setup_shows_and_nothing_is_sent() {
    let (config, der) = self_signed(&["localhost"], year() - 1, year() + 1);
    let server = Server::start(config);
    let CertificateProblem::NotTrusted(info) = problem(get(&server.url(), "")) else { panic!("not offered for trust") };
    assert_eq!(info.fingerprint, cert::fingerprint(&der));
    assert_eq!((info.subject.as_str(), info.issuer.as_str()), ("localhost (Example Org)", "localhost (Example Org)"));
    assert!(info.not_before < info.not_after && info.not_after > 0);
    // The handshake ended at the certificate: not a byte of the request, the password with it, went.
    assert_eq!(server.asked(), 0);
    assert!(server.last.lock().unwrap().is_empty());
    let problem = CertificateProblem::NotTrusted(info);
    assert!(problem.can_be_trusted());
    assert!(!problem.to_string().contains("localhost") && !problem.to_string().contains("Example"));
}

#[test]
fn exactly_the_chosen_certificate_is_accepted() {
    let (config, der) = self_signed(&["localhost"], year() - 1, year() + 1);
    let server = Server::start(config);
    let fingerprint = cert::fingerprint(&der);
    let answer = get(&server.url(), &fingerprint).unwrap();
    assert_eq!((answer.status, answer.body.as_slice()), (200, &b"hello"[..]));
    // In any case, with spaces around it.
    assert_eq!(get(&server.url(), &format!(" {} ", fingerprint.to_uppercase())).unwrap().status, 200);
    assert_eq!(server.asked(), 2);
    // The request went as written (the credential reaches a server that has the chosen certificate).
    let seen = server.last.lock().unwrap().clone();
    assert!(
        seen.starts_with("POST /autodiscover/autodiscover.xml HTTP/1.1\r\n") && seen.contains("Authorization: Basic c2VjcmV0OnNlY3JldA==\r\n"),
        "{seen}"
    );
}

#[test]
fn a_changed_certificate_is_refused_and_asks_again() {
    let (first, first_der) = self_signed(&["localhost"], year() - 1, year() + 1);
    let (second, second_der) = self_signed(&["localhost"], year() - 1, year() + 1);
    assert_ne!(first_der, second_der);
    let trusted = cert::fingerprint(&first_der);
    let (old, new) = (Server::start(first), Server::start(second));
    assert_eq!(get(&old.url(), &trusted).unwrap().status, 200);
    let CertificateProblem::Changed(info) = problem(get(&new.url(), &trusted)) else { panic!("not a change") };
    assert_eq!(info.fingerprint, cert::fingerprint(&second_der));
    assert_eq!(new.asked(), 0);
    // A fingerprint that is nearly the right one is another certificate.
    let mut near = trusted.clone().into_bytes();
    near[0] = if near[0] == b'0' { b'1' } else { b'0' };
    assert!(matches!(problem(get(&old.url(), &String::from_utf8(near).unwrap())), CertificateProblem::Changed(_)));
}

#[test]
fn certificates_out_of_date_or_for_another_server_are_never_accepted_by_fingerprint() {
    let (expired, expired_der) = self_signed(&["localhost"], year() - 4, year() - 2);
    let (early, early_der) = self_signed(&["localhost"], year() + 2, year() + 4);
    let (wrong, wrong_der) = self_signed(&["mail.example.org"], year() - 1, year() + 1);
    for (config, der, fault) in [
        (expired, expired_der, CertificateFault::Expired),
        (early, early_der, CertificateFault::NotYetValid),
        (wrong, wrong_der, CertificateFault::WrongName),
    ] {
        let server = Server::start(config);
        let fingerprint = cert::fingerprint(&der);
        // Not with its own fingerprint, and not without one.
        for trusted in [fingerprint.as_str(), ""] {
            let found = problem(get(&server.url(), trusted));
            assert!(
                matches!(&found, CertificateProblem::Unacceptable(have, info) if *have == fault && info.fingerprint == fingerprint),
                "{fault:?}: {found:?}"
            );
            assert!(!found.can_be_trusted());
        }
        assert_eq!(server.asked(), 0, "{fault:?}");
    }
}

#[test]
fn a_certificate_the_system_trusts_for_another_name_is_not_offered_either() {
    let authority = Authority::new("MailCraft Test Authority");
    let server = Server::start(authority.server(&["mail.example.org"]));
    let found = trusting(vec![authority.root()], || problem(get(&server.url(), "")));
    assert!(matches!(found, CertificateProblem::Unacceptable(CertificateFault::WrongName, _)), "{found:?}");
}

#[test]
fn the_fingerprint_must_be_one_and_nothing_is_sent_when_it_is_not() {
    let plain = FakeHttp::answering(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec());
    for bad in ["abc", "zz", &"g".repeat(64), &"a".repeat(63)] {
        let error = get("https://localhost:1/", bad).unwrap_err();
        assert!(matches!(error, NetError::Config(_)), "{bad}: {error:?}");
        assert!(!error.to_string().contains(bad));
    }
    // (Plain HTTP to this machine takes no fingerprint into account; nothing is encrypted there.)
    assert_eq!(get(&plain.url("/x"), "").unwrap().status, 200);
}

#[test]
fn only_https_or_this_machine() {
    for url in ["http://mail.example.org/x", "ftp://mail.example.org/x", "https://user:pw@mail.example.org/x", "mail.example.org", ""] {
        assert!(matches!(get(url, ""), Err(NetError::Config(_))), "{url}");
    }
}

#[test]
fn what_is_not_a_tls_server_is_a_connection_failure_not_a_certificate_question() {
    let plain = FakeHttp::answering(b"HTTP/1.1 200 OK\r\n\r\n".to_vec());
    let error = get(&format!("https://localhost:{}/x", plain.port), "").unwrap_err();
    assert!(matches!(error, NetError::Tls { .. } | NetError::Connect { .. } | NetError::Proto(_)), "{error:?}");
    assert!(certificate_problem(&error).is_none());
}

#[test]
fn the_answers_headers_that_are_read_come_back() {
    let answer = b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Negotiate\r\nWWW-Authenticate: NTLM\r\nWWW-Authenticate: Basic realm=\"mail\"\r\nLocation: https://other.example.org/x\r\nRetry-After: 7\r\nX-EWS-Policy-Reason: blocked\r\nContent-Length: 0\r\n\r\n".to_vec();
    let plain = FakeHttp::answering(answer);
    let response = get(&plain.url("/x"), "").unwrap();
    assert_eq!(response.status, 401);
    assert_eq!(response.challenges, ["Negotiate", "NTLM", "Basic realm=\"mail\""]);
    assert_eq!(
        (response.location.as_deref(), response.retry_after.as_deref(), response.policy_reason.as_deref()),
        (Some("https://other.example.org/x"), Some("7"), Some("blocked"))
    );
    // No redirect is followed: the status and the Location are handed back as they are.
    let redirect = FakeHttp::answering(b"HTTP/1.1 302 Found\r\nLocation: https://other.example.org/y\r\nContent-Length: 0\r\n\r\n".to_vec());
    assert_eq!(get(&redirect.url("/x"), "").unwrap().status, 302);
    assert_eq!(redirect.asked(), 1);
    // And the request carried no cookie and no referrer.
    let request = plain.request(0).to_lowercase();
    assert!(!request.contains("cookie") && !request.contains("referer"), "{request}");
    assert!(!format!("{response:?}").contains("other.example"));
}

#[test]
fn mail_connections_and_pictures_are_unchanged_by_all_this() {
    // The usual settings still refuse a certificate nobody trusts, with no way in by fingerprint.
    let (config, _) = self_signed(&["localhost"], year() - 1, year() + 1);
    let server = Server::start(config);
    let url = http::Url::parse(&server.url()).unwrap();
    let plain = http::Request { method: "GET", headers: &[], body: None, patience: Duration::from_secs(10), max_body: 1024 };
    let error = http::request(&url, &plain).unwrap_err();
    assert!(matches!(error, NetError::Tls { .. }), "{error:?}");
    assert_eq!(server.asked(), 0);
    let config = crate::tls::client_config(crate::tls::Purpose::Mail).unwrap();
    assert!(config.alpn_protocols.is_empty());
}

/// Presents one certificate and signs with the key of another.
#[derive(Debug)]
struct Impostor(Arc<rustls::sign::CertifiedKey>);

impl rustls::server::ResolvesServerCert for Impostor {
    fn resolve(&self, _: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }
}

#[test]
fn a_server_that_shows_the_chosen_certificate_but_cannot_sign_with_its_key_is_refused() {
    let (_, der) = self_signed(&["localhost"], year() - 1, year() + 1);
    // The right certificate on the wire, the signature of a stranger's key.
    let stranger = KeyPair::generate().unwrap();
    let provider = rustls::crypto::ring::default_provider();
    let key = provider.key_provider.load_private_key(PrivatePkcs8KeyDer::from(stranger.serialize_der()).into()).unwrap();
    let certified = rustls::sign::CertifiedKey::new(vec![CertificateDer::from(der.clone())], key);
    let config = ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(Impostor(Arc::new(certified))));
    let server = Server::start(Arc::new(config));
    let error = get(&server.url(), &cert::fingerprint(&der)).unwrap_err();
    // Refused as a failed handshake: not accepted, and not a certificate to offer for trust.
    assert!(matches!(error, NetError::Tls { .. }), "{error:?}");
    assert_eq!(server.asked(), 0);
}

/// Where the platform verifier is webpki over the system's store (Linux, FreeBSD), a certificate
/// made for clients only, issued by an authority the test trusts, goes through the real verifier:
/// it says "invalid extensions", and not even the owner's choice of that certificate gets it in.
#[cfg(all(unix, not(target_vendor = "apple")))]
#[test]
fn a_certificate_that_is_not_for_servers_is_refused_even_when_pinned() {
    let authority = Authority::new("MailCraft Test Authority");
    let (config, der) = authority.server_for(&["localhost"], vec![ExtendedKeyUsagePurpose::ClientAuth]);
    let server = Server::start(config);
    let fingerprint = cert::fingerprint(&der);
    trusting(vec![authority.root()], || {
        // Neither with its fingerprint nor without it is it accepted or offered.
        for trusted in [fingerprint.as_str(), ""] {
            let error = get(&server.url(), trusted).unwrap_err();
            assert!(!matches!(error, NetError::Certificate(_)), "offered for trust: {error:?}");
            assert!(matches!(error, NetError::Tls { .. }), "{error:?}");
        }
    });
    assert_eq!(server.asked(), 0);
}

#[path = "https_held_tests.rs"]
mod held;
