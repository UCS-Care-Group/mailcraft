// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of an Exchange server account: what a certificate that is not accepted does to a
//! run (never a plain TLS error), how a failed exchange is told, and the credential.

use std::sync::atomic::AtomicBool;

use mailcraft_net::{CertificateFault, CertificateInfo};

use super::*;

fn info() -> CertificateInfo {
    CertificateInfo {
        fingerprint: "ab".repeat(32),
        subject: "mail.example.org".into(),
        issuer: "An Invented CA".into(),
        not_before: 1_700_000_000,
        not_after: 1_900_000_000,
    }
}

fn last() -> EwsMailResult {
    EwsMailResult { errors: vec![NO_SECURE_LINE.into()], partial: false, whole: true, ..Default::default() }
}

#[test]
fn a_certificate_the_owner_may_be_asked_about_ends_the_run_with_the_question() {
    for problem in [CertificateProblem::NotTrusted(info()), CertificateProblem::Changed(info())] {
        let piece = settled(last(), Some(&problem), None);
        assert_eq!(piece.untrusted, "ab".repeat(32));
        assert!(piece.errors.is_empty(), "the line of a failed secure connection gives way to the question");
        assert!(!piece.whole && !piece.auth_refused && !piece.partial);
    }
}

#[test]
fn a_certificate_nobody_may_trust_is_told_in_words_and_never_offered() {
    for fault in [CertificateFault::Expired, CertificateFault::NotYetValid, CertificateFault::WrongName] {
        let problem = CertificateProblem::Unacceptable(fault, info());
        let piece = settled(last(), Some(&problem), None);
        assert!(piece.untrusted.is_empty());
        assert_eq!(piece.errors.len(), 1);
        assert!(piece.errors[0].starts_with("the server's certificate"), "{:?}", piece.errors);
        // (Nothing of the certificate is in the words.)
        assert!(!piece.errors[0].contains("mail.example.org") && !piece.errors[0].contains("Invented"));
    }
}

#[test]
fn a_run_without_a_certificate_problem_is_left_as_it_was_and_a_piece_under_way_is_never_touched() {
    let piece = EwsMailResult { whole: true, partial: false, ..Default::default() };
    assert_eq!(settled(piece.clone(), None, None), piece);
    let under_way = EwsMailResult { partial: true, ..Default::default() };
    assert_eq!(settled(under_way.clone(), Some(&CertificateProblem::NotTrusted(info())), None), under_way);
    let fetched = settled_fetch(GraphFetchResult::default(), Some(&CertificateProblem::NotTrusted(info())), None);
    assert_eq!(fetched.errors.len(), 1);
    assert!(fetched.errors[0].contains("not one this computer trusts"));
}

#[test]
fn a_failed_exchange_is_told_by_its_kind_and_a_certificate_is_kept_apart() {
    let transport = Transport::new(" ");
    assert_eq!(transport.problem(), None);
    assert_eq!(transport.judged(NetError::Connect { host: "mail.example.org".into(), reason: "refused".into() }), TransportError::Unreachable);
    assert_eq!(transport.judged(NetError::Config("x".into())), TransportError::Other);
    assert_eq!(transport.judged(NetError::Tls { host: "mail.example.org".into(), reason: "alert".into() }), TransportError::Tls);
    assert_eq!(transport.problem(), None, "a plain TLS failure is no certificate question");
    assert_eq!(transport.judged(NetError::Certificate(Box::new(CertificateProblem::Changed(info())))), TransportError::Tls);
    assert!(matches!(transport.problem(), Some(CertificateProblem::Changed(_))));
}

#[test]
fn the_credential_is_the_user_name_or_the_address_and_nothing_else_is_accepted() {
    assert!(client("https://mail.example.org/EWS/Exchange.asmx", "", "ada@example.org", "pw").is_ok());
    assert!(client("https://mail.example.org/EWS/Exchange.asmx", "CORP\\ada", "ada@example.org", "pw").is_ok());
    // A colon in the name, a control character in the password, an address that is no address.
    assert!(client("https://mail.example.org/EWS/Exchange.asmx", "a:b", "ada@example.org", "pw").is_err());
    assert!(client("https://mail.example.org/EWS/Exchange.asmx", "ada", "ada@example.org", "p\nw").is_err());
    assert!(client("http://mail.example.org/EWS/Exchange.asmx", "ada", "ada@example.org", "pw").is_err(), "plain http only to this machine");
    // What it says names neither the user nor the password.
    let why = client("https://mail.example.org/EWS/Exchange.asmx", "ada:", "ada@example.org", "secret-pw").err().unwrap();
    assert!(!why.contains("ada") && !why.contains("secret-pw"));
}

#[test]
fn a_run_that_cannot_begin_ends_with_a_line() {
    let request = EwsMailRequest { url: "not an address".into(), email: "ada@example.org".into(), ..Default::default() };
    let mut pieces = Vec::new();
    run_mail_with(&request, "pw", &AtomicBool::new(false), &mut |piece| pieces.push(piece));
    assert_eq!(pieces.len(), 1);
    assert!(!pieces[0].partial && !pieces[0].whole && pieces[0].errors.len() == 1);
}

#[test]
fn a_wait_hears_cancel() {
    let cancel = AtomicBool::new(true);
    let started = std::time::Instant::now();
    assert!(!wait(&cancel, 60_000));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(wait(&AtomicBool::new(false), 120));
}
