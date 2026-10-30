// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The pinned verifier's decision, with the system's error given by hand: the date and name
//! re-check that is the only guard where the system words its errors loosely (macOS, Windows),
//! and the errors that stay final.
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, OtherError};

use super::*;
use crate::cert::cert_tests::make;

/// A system verifier that refuses everything: only `refused` is under test.
#[derive(Debug)]
struct Refuses;

impl ServerCertVerifier for Refuses {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Err(Error::InvalidCertificate(CertificateError::UnknownIssuer))
    }

    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("test".into()))
    }

    fn verify_tls13_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("test".into()))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        Vec::new()
    }
}

#[derive(Debug)]
struct Odd(&'static str);

impl std::fmt::Display for Odd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Odd {}

fn other(text: &'static str) -> Error {
    Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(Odd(text)))))
}

/// 2026-06-15 12:00:00 UTC, in the middle of the certificates' good year.
fn now() -> UnixTime {
    UnixTime::since_unix_epoch(Duration::from_secs(1_781_524_800))
}

fn name(host: &str) -> ServerName<'static> {
    ServerName::try_from(host.to_owned()).unwrap()
}

/// A self-signed certificate for `names`, good from `from` to `to`, and a verifier that has pinned it.
fn pinned(names: &[&str], from: i32, to: i32) -> (Pinned, CertificateDer<'static>) {
    let der = CertificateDer::from(make(names, names[0], None, (from, 1, 1), (to, 1, 1)));
    (Pinned::new(Arc::new(Refuses), Some(cert::sha256(der.as_ref()))), der)
}

/// What each way the system can word "I don't trust this chain" gives, for a certificate that is
/// otherwise in date and for the name.
fn the_ways_a_system_says_it_does_not_trust() -> Vec<Error> {
    vec![Error::InvalidCertificate(CertificateError::UnknownIssuer), other("untrusted root"), other("0x800B0109")]
}

#[test]
fn a_sound_pinned_certificate_is_accepted_whatever_way_the_system_words_the_distrust() {
    for error in the_ways_a_system_says_it_does_not_trust() {
        let (verifier, der) = pinned(&["mail.example.org"], 2026, 2027);
        assert!(verifier.refused(error.clone(), &der, &name("mail.example.org"), now()).is_ok(), "{error:?}");
        assert!(verifier.take_problem().is_none());
        // The same certificate with no pin, or another one, is the owner's question.
        let unpinned = Pinned::new(Arc::new(Refuses), None);
        assert!(unpinned.refused(error.clone(), &der, &name("mail.example.org"), now()).is_err());
        assert!(matches!(unpinned.take_problem(), Some(CertificateProblem::NotTrusted(_))));
        let elsewhere = Pinned::new(Arc::new(Refuses), Some([7; 32]));
        assert!(elsewhere.refused(error, &der, &name("mail.example.org"), now()).is_err());
        assert!(matches!(elsewhere.take_problem(), Some(CertificateProblem::Changed(_))));
    }
}

#[test]
fn dates_and_name_are_checked_again_whatever_the_system_said() {
    for error in the_ways_a_system_says_it_does_not_trust() {
        // Expired: good from 2020 to 2021, and it is 2026.
        let (verifier, der) = pinned(&["mail.example.org"], 2020, 2021);
        assert!(verifier.refused(error.clone(), &der, &name("mail.example.org"), now()).is_err(), "{error:?}");
        assert!(matches!(verifier.take_problem(), Some(CertificateProblem::Unacceptable(CertificateFault::Expired, _))), "{error:?}");
        // Not yet valid: good from 2030.
        let (verifier, der) = pinned(&["mail.example.org"], 2030, 2031);
        assert!(verifier.refused(error.clone(), &der, &name("mail.example.org"), now()).is_err(), "{error:?}");
        assert!(matches!(verifier.take_problem(), Some(CertificateProblem::Unacceptable(CertificateFault::NotYetValid, _))), "{error:?}");
        // Another name.
        let (verifier, der) = pinned(&["other.example.org"], 2026, 2027);
        assert!(verifier.refused(error.clone(), &der, &name("mail.example.org"), now()).is_err(), "{error:?}");
        assert!(matches!(verifier.take_problem(), Some(CertificateProblem::Unacceptable(CertificateFault::WrongName, _))), "{error:?}");
        // The same certificate is fine a moment earlier or later, inside its dates: the edges are the dates'.
        let (verifier, der) = pinned(&["mail.example.org"], 2026, 2027);
        let later = UnixTime::since_unix_epoch(Duration::from_secs(1_798_761_600 - 1));
        assert!(verifier.refused(error.clone(), &der, &name("mail.example.org"), later).is_ok(), "{error:?}");
        let after = UnixTime::since_unix_epoch(Duration::from_secs(1_798_761_600 + 86_400 * 2));
        assert!(verifier.refused(error, &der, &name("mail.example.org"), after).is_err());
    }
}

#[test]
fn what_the_system_says_in_dates_and_names_is_believed_too() {
    // Even where this program's own check would let it pass.
    let cases = [
        (Error::InvalidCertificate(CertificateError::Expired), CertificateFault::Expired),
        (Error::InvalidCertificate(CertificateError::NotValidYet), CertificateFault::NotYetValid),
        (Error::InvalidCertificate(CertificateError::NotValidForName), CertificateFault::WrongName),
    ];
    for (error, fault) in cases {
        let (verifier, der) = pinned(&["mail.example.org"], 2026, 2027);
        assert!(verifier.refused(error.clone(), &der, &name("mail.example.org"), now()).is_err());
        assert!(matches!(verifier.take_problem(), Some(CertificateProblem::Unacceptable(have, _)) if have == fault), "{error:?}");
    }
}

#[test]
fn errors_that_are_not_about_trust_stay_final_pinned_or_not() {
    let finals = [
        Error::InvalidCertificate(CertificateError::BadSignature),
        Error::InvalidCertificate(CertificateError::Revoked),
        Error::InvalidCertificate(CertificateError::InvalidPurpose),
        Error::InvalidCertificate(CertificateError::UnhandledCriticalExtension),
        Error::InvalidCertificate(CertificateError::BadEncoding),
        // How the platform verifier words a purpose error on Linux and FreeBSD.
        other("certificate had invalid extensions"),
        Error::General("x".into()),
        Error::DecryptError,
    ];
    for error in finals {
        let (verifier, der) = pinned(&["mail.example.org"], 2026, 2027);
        let refused = verifier.refused(error.clone(), &der, &name("mail.example.org"), now());
        assert_eq!(format!("{:?}", refused.unwrap_err()), format!("{error:?}"));
        assert!(verifier.take_problem().is_none(), "{error:?}");
    }
}

#[test]
fn a_certificate_that_cannot_be_read_is_never_pinned() {
    let verifier = Pinned::new(Arc::new(Refuses), Some(cert::sha256(b"junk")));
    let junk = CertificateDer::from(b"junk".to_vec());
    assert!(verifier.refused(Error::InvalidCertificate(CertificateError::UnknownIssuer), &junk, &name("mail.example.org"), now()).is_err());
    assert!(verifier.take_problem().is_none());
}

/// A system verifier that says whether the pinned check was under way while it looked.
#[derive(Debug)]
struct Watches(std::sync::atomic::AtomicBool);

impl ServerCertVerifier for Watches {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        self.0.store(pin_check_under_way(), std::sync::atomic::Ordering::SeqCst);
        Err(Error::InvalidCertificate(CertificateError::UnknownIssuer))
    }

    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("test".into()))
    }

    fn verify_tls13_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("test".into()))
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        Vec::new()
    }
}

#[test]
fn the_systems_look_is_marked_only_for_the_pinned_certificate_and_only_while_it_looks() {
    let der = CertificateDer::from(make(&["mail.example.org"], "mail.example.org", None, (2026, 1, 1), (2027, 1, 1)));
    // (A pin for another certificate is no match: the refusal is a real one.)
    for (pin, marked) in [(Some(cert::sha256(der.as_ref())), true), (Some(cert::sha256(b"another")), false), (None, false)] {
        let system = Arc::new(Watches(std::sync::atomic::AtomicBool::new(false)));
        let verifier = Pinned::new(system.clone(), pin);
        assert!(!pin_check_under_way());
        let _ = verifier.verify_server_cert(&der, &[], &name("mail.example.org"), &[], now());
        assert_eq!(system.0.load(std::sync::atomic::Ordering::SeqCst), marked);
        assert!(!pin_check_under_way(), "cleared when the look is over");
    }
}
