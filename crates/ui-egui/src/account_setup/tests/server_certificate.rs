// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1413: an IMAP or SMTP server whose certificate the system doesn't trust. The check stops at
//! the certificate and the page shows it; only with the box ticked is it trusted, for that server
//! alone, and the account is tried again with it; the account is added only once a check works,
//! and keeps what was trusted. Back trusts nothing.

use super::*;
use crate::AccountCheckRefusal;
use crate::ews_setup::EwsCertificate;

const TICK: &str = "Trust this certificate for this account";

/// A certificate a mail bridge on this computer shows, for the incoming or the outgoing server.
fn certificate(outgoing: bool) -> AccountCheckRefusal {
    let fingerprint = if outgoing { "cd" } else { "ab" }.repeat(32);
    AccountCheckRefusal::Certificate {
        outgoing,
        certificate: Box::new(EwsCertificate {
            url: "127.0.0.1".into(),
            fingerprint,
            subject: "127.0.0.1, An Invented Bridge".into(),
            issuer: "127.0.0.1, An Invented Bridge".into(),
            not_before: 1_700_000_000,
            not_after: 2_300_000_000,
            changed: false,
        }),
    }
}

/// A host that answers each check with the next of `answers`, in turn. The log is each account it
/// was handed, with its secret.
fn checks_in_turn(app: &mut MailApp, answers: Vec<Result<(), AccountCheckRefusal>>) -> Log<(Account, String)> {
    let asked: Log<(Account, String)> = Default::default();
    let log = asked.clone();
    let answers = Arc::new(Mutex::new(std::collections::VecDeque::from(answers)));
    app.services.test_account = Some(Box::new(move |account, secret| {
        log.lock().unwrap().push((account, secret));
        let (tx, rx) = mpsc::channel();
        if let Some(answer) = answers.lock().unwrap().pop_front() {
            tx.send(answer).unwrap();
        }
        rx
    }));
    asked
}

/// The password page of an address at a domain nobody knows, its servers set to a bridge's on
/// this computer, and Connect pressed.
fn connect_to_bridge(a: MailApp) -> Harness<'static, MailApp> {
    let mut a = a;
    lookups(&mut a, Some(Ok(vec!["mx.hosted.example"])));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    assert_eq!(page(&h), Some(Page::Password));
    let w = wizard_mut(&mut h);
    (w.incoming.host, w.incoming.port, w.incoming.security) = ("127.0.0.1".into(), "1143".into(), Security::StartTls);
    (w.outgoing.host, w.outgoing.port, w.outgoing.security) = ("127.0.0.1".into(), "1025".into(), Security::StartTls);
    type_password(&mut h);
    press(&mut h, Key::Enter);
    h
}

fn trusted(account: &Account) -> (String, String) {
    let of = |s: &Option<Server>| s.as_ref().map(|s| s.trusted.clone()).unwrap_or_default();
    (of(&account.incoming), of(&account.outgoing))
}

fn added(h: &Harness<'static, MailApp>) -> Option<Account> {
    h.state().session.mailbox.accounts.iter().find(|a| a.email == "noor@lumen.example").cloned()
}

#[test]
fn each_servers_certificate_is_asked_about_and_the_account_keeps_what_was_trusted() {
    let mut a = app(false);
    let asked = checks_in_turn(&mut a, vec![Err(certificate(false)), Err(certificate(true)), Ok(())]);
    let mut h = connect_to_bridge(a);
    assert_eq!(page(&h), Some(Page::ServerCertificate));
    for shown in ["Trust this server's certificate?", "127.0.0.1", "An Invented Bridge", "ab:ab:ab:ab", "won't connect to it"] {
        assert!(says(&h, shown), "{shown}");
    }
    // It promises no question later: a mail server's certificate that changes is refused, and said.
    assert!(!says(&h, "you are asked again"));
    // Not trusted, and not tried again, until the box is ticked.
    h.get_by_role_and_label(Role::Button, "Continue").click();
    h.run_steps(5);
    assert_eq!((page(&h), asked.lock().unwrap().len()), (Some(Page::ServerCertificate), 1), "Continue waits for the tick");
    assert!(added(&h).is_none());

    // The incoming server's: tried again with it, and the outgoing server's is asked about next.
    h.get_by_role_and_label(Role::CheckBox, TICK).click();
    h.run_steps(3);
    click(&mut h, "Continue");
    h.run_steps(5);
    assert_eq!(page(&h), Some(Page::ServerCertificate));
    assert!(says(&h, "cd:cd:cd:cd"));
    {
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert_eq!(trusted(&asked[1].0), ("ab".repeat(32), String::new()));
        assert_eq!(asked[1].1, TYPED);
    }
    assert!(added(&h).is_none(), "nothing is added while a check stops");
    // The box starts unticked again for the other server.
    h.get_by_role_and_label(Role::Button, "Continue").click();
    h.run_steps(5);
    assert_eq!(asked.lock().unwrap().len(), 2);

    // The outgoing server's: the check works, and the account is added with both.
    h.get_by_role_and_label(Role::CheckBox, TICK).click();
    h.run_steps(3);
    click(&mut h, "Continue");
    h.run_steps(5);
    assert_eq!(trusted(&asked.lock().unwrap()[2].0), ("ab".repeat(32), "cd".repeat(32)));
    let account = added(&h).expect("the account was added");
    assert_eq!(trusted(&account), ("ab".repeat(32), "cd".repeat(32)));
}

#[test]
fn back_from_the_certificate_trusts_nothing_and_adds_nothing() {
    let mut a = app(false);
    let asked = checks_in_turn(&mut a, vec![Err(certificate(false))]);
    let mut h = connect_to_bridge(a);
    assert_eq!(page(&h), Some(Page::ServerCertificate));
    h.get_by_role_and_label(Role::CheckBox, TICK).click();
    h.run_steps(3);
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Password));
    let w = wizard(&h).unwrap();
    assert!(w.server_certificate.is_none() && w.incoming.trusted.is_empty() && w.outgoing.trusted.is_empty());
    assert_eq!(asked.lock().unwrap().len(), 1);
    assert!(added(&h).is_none());
}

#[test]
fn words_from_the_host_are_said_on_the_password_page_as_before() {
    // A certificate nobody may trust (expired, for another name) comes as words: no question.
    let mut a = app(false);
    let words = mailcraft_i18n::Text::verbatim("the server's certificate has expired".to_string());
    checks_in_turn(&mut a, vec![Err(AccountCheckRefusal::Said(words))]);
    let h = connect_to_bridge(a);
    assert_eq!(page(&h), Some(Page::Password));
    assert!(says(&h, "the server's certificate has expired"));
    assert!(h.query_by_role_and_label(Role::CheckBox, TICK).is_none());
}
