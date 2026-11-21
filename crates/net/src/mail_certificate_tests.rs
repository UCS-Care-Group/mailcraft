// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One certificate the owner trusts for an IMAP or SMTP server (#1413), against the fake IMAP
//! server on this machine. The certificate is made the way a mail bridge on the owner's own
//! computer makes its own (Proton Mail Bridge, observed): self-signed, for `127.0.0.1`, and
//! marked as a certificate authority, which the system's check refuses outright
//! (`CaUsedAsEndEntity` on Linux and FreeBSD) whoever trusts it. Nothing here reaches the network.
use std::sync::Arc;

use mailcraft_model::{Account, Security, Server};
use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose, SanType};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};

use crate::NetError;
use crate::cert::{self, CertificateFault, CertificateProblem};
use crate::fake::{Fake, ImapWorld, MailIo};

const USER: &str = "user@example.org";
const PASSWORD: &str = "invented";

/// The server settings for a bridge's own certificate, valid from `from` to `to` (years), and
/// its DER.
fn bridge_certificate(from: i32, to: i32) -> (Arc<ServerConfig>, Vec<u8>) {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.subject_alt_names = vec![SanType::IpAddress(std::net::IpAddr::from([127, 0, 0, 1]))];
    params.distinguished_name.push(DnType::CountryName, "CH");
    params.distinguished_name.push(DnType::OrganizationName, "Example Bridge");
    params.distinguished_name.push(DnType::CommonName, "127.0.0.1");
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment, KeyUsagePurpose::KeyCertSign];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth, ExtendedKeyUsagePurpose::ClientAuth];
    params.not_before = rcgen::date_time_ymd(from, 1, 1);
    params.not_after = rcgen::date_time_ymd(to, 1, 1);
    let key = KeyPair::generate().unwrap();
    let certificate = params.self_signed(&key).unwrap();
    let der = certificate.der().to_vec();
    let config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(der.clone())], PrivatePkcs8KeyDer::from(key.serialize_der()).into())
        .unwrap();
    (Arc::new(config), der)
}

fn year() -> i32 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    1970 + i32::try_from(now / 31_556_952).unwrap()
}

/// A fake IMAP server on this machine that offers STARTTLS and then shakes hands with `server`.
fn imap(server: Arc<ServerConfig>) -> Fake<ImapWorld> {
    Fake::imap(ImapWorld {
        user: USER.into(),
        password: PASSWORD.into(),
        offer_starttls: true,
        starttls_upgrade: Some(Arc::new(move |stream| -> Box<dyn MailIo> {
            let connection = rustls::ServerConnection::new(server.clone()).unwrap();
            Box::new(rustls::StreamOwned::new(connection, stream))
        })),
        ..Default::default()
    })
}

fn account(port: u16, trusted: &str) -> Account {
    Account {
        email: USER.into(),
        incoming: Some(Server { host: "127.0.0.1".into(), port, security: Security::StartTls, trusted: trusted.into() }),
        ..Default::default()
    }
}

/// What the check said of the certificate, which it must have stopped at; no password went.
fn stopped_at(peer: &Fake<ImapWorld>, trusted: &str) -> CertificateProblem {
    let Err(error) = crate::sync::open_imap(&account(peer.port, trusted), PASSWORD) else { panic!("the connection was allowed") };
    let NetError::Certificate(problem) = error else { panic!("refused, but not at the certificate: {error}") };
    assert!(!signed_in(peer), "a password went to a server whose certificate wasn't accepted");
    *problem
}

fn signed_in(peer: &Fake<ImapWorld>) -> bool {
    peer.world().log.iter().any(|line| line.contains(" LOGIN ") || line.contains("AUTHENTICATE"))
}

#[test]
fn a_bridges_own_certificate_is_shown_for_trust_before_the_password_goes() {
    let (server, der) = bridge_certificate(year() - 1, year() + 10);
    let peer = imap(server);
    let CertificateProblem::NotTrusted(info) = stopped_at(&peer, "") else { panic!("not offered for trust") };
    // Setup shows what it needs to: the fingerprint of exactly this certificate, and its names.
    assert_eq!(info.fingerprint, cert::fingerprint(&der));
    assert!(info.subject.contains("127.0.0.1"), "{}", info.subject);
    assert!(info.issuer.contains("Example Bridge"), "{}", info.issuer);
    // And what `test_account` says of it, which setup is given, is the same question.
    let checked = crate::check_account(&account(peer.port, ""), PASSWORD);
    assert!(matches!(checked, Err((Some(crate::MailServer::Incoming), NetError::Certificate(_)))), "{checked:?}");
}

#[test]
fn the_systems_check_alone_refuses_a_bridges_certificate_as_an_authority() {
    // Why a fingerprint is needed at all: the system's verifier, trusted or not, won't take a
    // certificate authority's certificate as a server's. This is what the owner met before #1413.
    let (server, _) = bridge_certificate(year() - 1, year() + 10);
    let peer = imap(server);
    let server = Server { host: "127.0.0.1".into(), port: peer.port, security: Security::StartTls, ..Default::default() };
    let (stream, host) = crate::transport::open(&server, crate::transport::Service::Imap, &Default::default()).unwrap();
    let plain = mailcraft_proto::imap::Imap::connect(stream).unwrap().starttls().unwrap();
    let Err(NetError::Tls { reason, .. }) = crate::transport::upgrade(plain, &host, None) else { panic!("the system's check took it") };
    #[cfg(all(unix, not(target_vendor = "apple")))]
    assert!(reason.to_string().contains("CaUsedAsEndEntity"), "{reason}");
    assert!(!signed_in(&peer));
}

#[test]
fn the_certificate_the_owner_trusted_connects_and_signs_in() {
    let (server, der) = bridge_certificate(year() - 1, year() + 10);
    let peer = imap(server);
    let trusted = cert::fingerprint(&der);
    let opened = crate::sync::open_imap(&account(peer.port, &trusted), PASSWORD);
    assert!(opened.is_ok(), "{:?}", opened.err().map(|e| e.to_string()));
    assert!(signed_in(&peer));
    // Upper case is the same fingerprint.
    assert_eq!(crate::test_account(&account(peer.port, &trusted.to_ascii_uppercase()), PASSWORD), Ok(()));
}

#[test]
fn another_certificate_than_the_trusted_one_asks_again_and_sends_nothing() {
    let (server, _) = bridge_certificate(year() - 1, year() + 10);
    let (_, other) = bridge_certificate(year() - 1, year() + 10);
    let peer = imap(server);
    let problem = stopped_at(&peer, &cert::fingerprint(&other));
    assert!(matches!(problem, CertificateProblem::Changed(_)), "{problem:?}");
    assert!(problem.can_be_trusted());
}

#[test]
fn an_expired_certificate_is_never_offered_and_no_fingerprint_lets_it_in() {
    let (server, der) = bridge_certificate(year() - 5, year() - 1);
    let peer = imap(server);
    for trusted in [String::new(), cert::fingerprint(&der)] {
        let problem = stopped_at(&peer, &trusted);
        assert!(matches!(problem, CertificateProblem::Unacceptable(CertificateFault::Expired, _)), "{problem:?}");
        assert!(!problem.can_be_trusted());
    }
}

#[test]
fn a_fingerprint_that_is_no_fingerprint_is_refused_before_connecting() {
    let (server, _) = bridge_certificate(year() - 1, year() + 10);
    let peer = imap(server);
    let Err(error) = crate::sync::open_imap(&account(peer.port, "not-a-fingerprint"), PASSWORD) else { panic!("connected") };
    assert!(matches!(error, NetError::Config(_)), "{error}");
    assert!(peer.world().log.is_empty(), "{:?}", peer.world().log);
}
