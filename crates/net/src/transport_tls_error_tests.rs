// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #977: a server with no cipher suite in common says so in plain words; other failures read as before.

use super::*;
use rustls::{AlertDescription, CertificateError, Error, PeerIncompatible};

fn from(error: Error) -> std::io::Error {
    std::io::Error::new(ErrorKind::InvalidData, error)
}

const PLAIN: &str =
    "the server offers only older encryption (CBC suites) that MailCraft doesn't use; its administrator can turn on the AES-GCM suites";

#[test]
fn no_suite_in_common_is_said_in_plain_words() {
    for error in [
        Error::AlertReceived(AlertDescription::HandshakeFailure),
        Error::AlertReceived(AlertDescription::InsufficientSecurity),
        Error::PeerIncompatible(PeerIncompatible::NoCipherSuitesInCommon),
    ] {
        assert_eq!(handshake_problem(&from(error)).to_string(), PLAIN);
    }
}

#[test]
fn other_tls_errors_are_unchanged() {
    let bad = from(Error::InvalidCertificate(CertificateError::Expired));
    assert_eq!(handshake_problem(&bad).to_string(), bad.to_string());
    let alert = from(Error::AlertReceived(AlertDescription::BadCertificate));
    assert_eq!(handshake_problem(&alert).to_string(), alert.to_string());
    let closed = std::io::Error::from(ErrorKind::UnexpectedEof);
    assert_eq!(handshake_problem(&closed).to_string(), "the server closed the connection");
}

#[test]
fn the_words_carry_no_address() {
    assert!(!PLAIN.contains("example") && !PLAIN.contains("://"));
}
