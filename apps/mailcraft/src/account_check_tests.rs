// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What this host hands account setup when the check of an account's mail servers stops at a
//! certificate (#1413): the question for the server that showed it, when the owner may trust it;
//! words otherwise. Nothing here reaches the network.

use mailcraft_model::{Account, Security, Server};
use mailcraft_net::{CertificateFault, CertificateInfo, CertificateProblem, MailServer, NetError};
use mailcraft_ui_egui::AccountCheckRefusal;

use super::*;

fn bridge() -> Account {
    let server = |port| Some(Server { host: " 127.0.0.1 ".into(), port, security: Security::StartTls, ..Default::default() });
    Account { email: "ada@example.org".into(), incoming: server(1143), outgoing: server(1025), ..Default::default() }
}

fn info() -> CertificateInfo {
    CertificateInfo {
        fingerprint: "ab".repeat(32),
        subject: "127.0.0.1, An Invented Bridge".into(),
        issuer: "127.0.0.1, An Invented Bridge".into(),
        not_before: 1_700_000_000,
        not_after: 2_300_000_000,
    }
}

fn refusal(server: Option<MailServer>, problem: CertificateProblem) -> AccountCheckRefusal {
    account_check_refusal(&bridge(), server, NetError::Certificate(Box::new(problem)))
}

#[test]
fn a_certificate_the_owner_may_trust_is_the_question_for_the_server_that_showed_it() {
    for (server, outgoing) in [(MailServer::Incoming, false), (MailServer::Outgoing, true)] {
        for (problem, changed) in [(CertificateProblem::NotTrusted(info()), false), (CertificateProblem::Changed(info()), true)] {
            let AccountCheckRefusal::Certificate { outgoing: said_outgoing, certificate } = refusal(Some(server), problem) else {
                panic!("not asked")
            };
            assert_eq!((said_outgoing, certificate.changed), (outgoing, changed));
            assert_eq!((certificate.url.as_str(), certificate.fingerprint.as_str()), ("127.0.0.1", "ab".repeat(32).as_str()));
            assert_eq!((certificate.not_before, certificate.not_after), (1_700_000_000, 2_300_000_000));
        }
    }
}

#[test]
fn a_certificate_nobody_may_trust_and_every_other_error_are_said_in_words() {
    for fault in [CertificateFault::Expired, CertificateFault::NotYetValid, CertificateFault::WrongName] {
        let said = refusal(Some(MailServer::Incoming), CertificateProblem::Unacceptable(fault, info()));
        assert!(matches!(said, AccountCheckRefusal::Said(_)), "{fault:?}: {said:?}");
    }
    // Not said of a server: there is nobody to ask about it.
    assert!(matches!(refusal(None, CertificateProblem::NotTrusted(info())), AccountCheckRefusal::Said(_)));
    let other = account_check_refusal(&bridge(), Some(MailServer::Incoming), NetError::SignInCancelled);
    assert_eq!(other, AccountCheckRefusal::Said(NetError::SignInCancelled.said()));
}

#[test]
fn an_account_that_signs_in_on_its_providers_page_is_never_offered_a_certificate() {
    // Its servers are the provider's own: a certificate the system doesn't trust there is said,
    // never offered for trust.
    let oauth = Some(mailcraft_model::OAuth { provider: mailcraft_model::OAuthProvider::Google, ..Default::default() });
    let account = Account { oauth, ..bridge() };
    for problem in [CertificateProblem::NotTrusted(info()), CertificateProblem::Changed(info())] {
        let said = account_check_refusal(&account, Some(MailServer::Incoming), NetError::Certificate(Box::new(problem)));
        assert!(matches!(said, AccountCheckRefusal::Said(_)), "{said:?}");
    }
}
