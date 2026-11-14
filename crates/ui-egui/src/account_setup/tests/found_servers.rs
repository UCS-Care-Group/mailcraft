// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #1063, #1112: the servers of an address at a domain nobody knows, found in the domain's own
//! records (its SRV records, RFC 6186; what it publishes for mail programs) or in Mozilla's ISP
//! database: shown in Server settings for the owner to check, with where they came from said; a
//! server outside the owner's domain is named to the owner before the password goes there, and a
//! no sends nothing.

use mailcraft_model::provider::{FoundServer, Via};

use super::*;

fn found(host: &str, port: u16, security: Security, via: Via) -> Option<FoundServer> {
    Some(FoundServer { server: Server { host: host.into(), port, security }, via })
}

/// The wizard on the password page for an address at `lumen.example`, whose records found these.
fn with_found(mut a: MailApp, discovered: Discovered) -> Harness<'static, MailApp> {
    let asked = discoveries(&mut a, Some(Ok(discovered)));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    assert_eq!(page(&h), Some(Page::Password));
    assert_eq!(*asked.lock().unwrap(), ["lumen.example"]);
    h
}

#[test]
fn servers_found_in_the_domains_records_are_shown_to_check_and_said_to_be_found() {
    let mut a = app(false);
    let checked = checks(&mut a, None);
    let discovered = Discovered {
        exchangers: vec!["mx.lumen.example".into()],
        incoming: found("mail.lumen.example", 993, Security::Tls, Via::Srv),
        outgoing: found("mail.lumen.example", 587, Security::StartTls, Via::Srv),
        username: String::new(),
    };
    let mut h = with_found(a, discovered);
    let w = wizard(&h).unwrap();
    // Not a guess, and said to come from the domain's DNS; open for the owner to check.
    assert_eq!((w.found, w.guessed, w.settings_open, w.oauth.is_none()), (Some(Via::Srv), false, true, true));
    assert_eq!(
        (w.username.as_str(), w.incoming.host.as_str(), w.outgoing.host.as_str()),
        ("noor@lumen.example", "mail.lumen.example", "mail.lumen.example")
    );
    assert_eq!(
        (w.incoming.port.as_str(), w.incoming.security, w.outgoing.port.as_str(), w.outgoing.security),
        ("993", Security::Tls, "587", Security::StartTls)
    );
    assert!(says(&h, "found in the DNS records of lumen.example"), "the page doesn't say where the servers came from");
    assert!(!says(&h, "only a guess") && !says(&h, "doesn't know the settings"));
    // In the domain: the password goes without a question.
    type_password(&mut h);
    press(&mut h, Key::Enter);
    assert_eq!(page(&h), Some(Page::Checking));
    let asked = checked.lock().unwrap();
    assert_eq!(asked.len(), 1);
    let (account, secret) = &asked[0];
    assert_eq!(
        (account.incoming.as_ref().map(|s| s.host.as_str()), account.username.as_str(), secret.as_str()),
        (Some("mail.lumen.example"), "noor@lumen.example", TYPED)
    );
}

#[test]
fn a_server_outside_the_domain_is_named_before_the_password_goes_there() {
    // A host far longer than a line, and one that ends as it does: each named in full, so that
    // the two can't be taken for each other.
    let tail = format!("{}.hosted.example", ["mail"; 40].join("."));
    let (long_in, long_out) = (format!("imap.{tail}"), format!("smtp.{tail}"));
    for (incoming, outgoing) in [("imap.hosted.example", "smtp.hosted.example"), (long_in.as_str(), long_out.as_str())] {
        let mut a = app(false);
        let checked = checks(&mut a, None);
        let discovered = Discovered {
            exchangers: vec!["mx.hosted.example".into()],
            incoming: found(incoming, 993, Security::Tls, Via::Srv),
            outgoing: found(outgoing, 465, Security::Tls, Via::Srv),
            username: String::new(),
        };
        let mut h = with_found(a, discovered);
        assert_eq!(wizard(&h).map(|w| w.found), Some(Some(Via::Srv)));
        type_password(&mut h);
        press(&mut h, Key::Enter);
        // Asked first, naming every such host whole; nothing was sent.
        assert_eq!(page(&h), Some(Page::ServerConsent), "{incoming}");
        assert!(says(&h, &format!("{incoming}, {outgoing}")), "{incoming}: the hosts aren't named whole");
        assert!(says(&h, "Send your password to another server?"));
        assert!(checked.lock().unwrap().is_empty());
        // No: back to the servers, and still nothing sent.
        click(&mut h, "Don't send");
        assert_eq!(page(&h), Some(Page::Password));
        assert!(checked.lock().unwrap().is_empty());
        // Yes: the password goes, and the question isn't asked twice.
        press(&mut h, Key::Enter);
        assert_eq!(page(&h), Some(Page::ServerConsent));
        click(&mut h, "Send my sign-in");
        assert_eq!(page(&h), Some(Page::Checking));
        assert_eq!(checked.lock().unwrap().len(), 1);
        assert_eq!(checked.lock().unwrap()[0].0.incoming.as_ref().map(|s| s.host.clone()), Some(incoming.to_string()));
        click(&mut h, "Cancel");
        assert_eq!(page(&h), Some(Page::Password));
        press(&mut h, Key::Enter);
        assert_eq!((page(&h), checked.lock().unwrap().len()), (Some(Page::Checking), 2));
    }
}

#[test]
fn a_host_the_owner_typed_is_their_own_and_what_is_not_found_is_said_to_be_a_guess() {
    let mut a = app(false);
    let checked = checks(&mut a, None);
    // Only the incoming server was found, outside the domain; the outgoing one is a guess.
    let discovered = Discovered {
        exchangers: Vec::new(),
        incoming: found("imap.hosted.example", 993, Security::Tls, Via::Srv),
        outgoing: None,
        username: String::new(),
    };
    let mut h = with_found(a, discovered);
    let w = wizard(&h).unwrap();
    assert_eq!(
        (w.found, w.guessed, w.outgoing.host.as_str(), w.username.as_str()),
        (Some(Via::Srv), true, "smtp.lumen.example", "noor@lumen.example")
    );
    assert!(says(&h, "found in the DNS records") && says(&h, "only a guess"));
    // The owner puts a server of their own choosing in its place: their own, not asked about.
    wizard_mut(&mut h).incoming.host = "imap.other.example".into();
    type_password(&mut h);
    press(&mut h, Key::Enter);
    assert_eq!(page(&h), Some(Page::Checking));
    assert_eq!(checked.lock().unwrap().len(), 1);
}

/// #1112: what the domain publishes for mail programs, and what Mozilla's ISP database lists.
#[test]
fn servers_from_a_published_document_or_the_isp_database_say_so_and_sign_in_as_it_says() {
    for (via, says_where) in [(Via::Autoconfig, "publishes for mail programs"), (Via::Ispdb, "Mozilla's ISP database lists")] {
        let mut a = app(false);
        let checked = checks(&mut a, None);
        let discovered = Discovered {
            exchangers: vec!["mx.lumen.example".into()],
            incoming: found("mail.lumen.example", 993, Security::Tls, via),
            outgoing: found("mail.lumen.example", 465, Security::Tls, via),
            username: "%EMAILLOCALPART%".into(),
        };
        let mut h = with_found(a, discovered);
        let w = wizard(&h).unwrap();
        // The user name is what the source's placeholder makes of the address.
        assert_eq!((w.found, w.guessed, w.settings_open, w.username.as_str()), (Some(via), false, true, "noor"), "{via:?}");
        assert!(says(&h, says_where) && !says(&h, "found in the DNS records") && !says(&h, "only a guess"), "{via:?}");
        // In the domain: the password goes without a question, with that user name.
        type_password(&mut h);
        press(&mut h, Key::Enter);
        assert_eq!(page(&h), Some(Page::Checking), "{via:?}");
        assert_eq!(checked.lock().unwrap()[0].0.username, "noor", "{via:?}");
    }
}

#[test]
fn a_server_outside_the_domain_is_named_whatever_named_it() {
    for (via, says_who) in [(Via::Autoconfig, "publishes for mail programs names"), (Via::Ispdb, "Mozilla's ISP database names")] {
        let mut a = app(false);
        let checked = checks(&mut a, None);
        let discovered = Discovered {
            exchangers: vec!["mx.hosted.example".into()],
            incoming: found("imap.hosted.example", 993, Security::Tls, via),
            outgoing: found("smtp.hosted.example", 465, Security::Tls, via),
            username: "%EMAILADDRESS%".into(),
        };
        let mut h = with_found(a, discovered);
        assert_eq!(wizard(&h).map(|w| (w.found, w.username.clone())), Some((Some(via), "noor@lumen.example".to_string())));
        type_password(&mut h);
        press(&mut h, Key::Enter);
        assert_eq!(page(&h), Some(Page::ServerConsent), "{via:?}");
        assert!(says(&h, "imap.hosted.example, smtp.hosted.example") && says(&h, says_who), "{via:?}: the hosts or their source aren't named");
        assert!(checked.lock().unwrap().is_empty());
        click(&mut h, "Don't send");
        assert_eq!(page(&h), Some(Page::Password));
        assert!(checked.lock().unwrap().is_empty(), "{via:?}");
        press(&mut h, Key::Enter);
        click(&mut h, "Send my sign-in");
        assert_eq!((page(&h), checked.lock().unwrap().len()), (Some(Page::Checking), 1), "{via:?}");
    }
}

/// The two servers from two sources: each is said to come from its own, on the page with the
/// servers and in the question before the password goes to the one outside the domain.
#[test]
fn each_server_is_said_to_come_from_the_source_that_named_it() {
    let mut a = app(false);
    let checked = checks(&mut a, None);
    let discovered = Discovered {
        exchangers: vec!["mx.lumen.example".into()],
        incoming: found("imap.lumen.example", 993, Security::Tls, Via::Autoconfig),
        outgoing: found("smtp.hosted.example", 465, Security::Tls, Via::Ispdb),
        username: String::new(),
    };
    let mut h = with_found(a, discovered);
    assert_eq!(wizard(&h).map(|w| (w.found, w.guessed)), Some((Some(Via::Autoconfig), false)));
    // Each server by its name, with its own source; no line that speaks of both at once.
    assert!(says(&h, "imap.lumen.example is what lumen.example publishes for mail programs"), "the incoming server's source isn't said");
    assert!(says(&h, "smtp.hosted.example is what Mozilla's ISP database lists for lumen.example"), "the outgoing server's source isn't said");
    assert!(says(&h, "Check both servers before you connect.") && !says(&h, "The servers below"));
    type_password(&mut h);
    press(&mut h, Key::Enter);
    assert_eq!(page(&h), Some(Page::ServerConsent));
    // Only the outside host is asked about, and the database is named as who named it.
    assert!(says(&h, "Mozilla's ISP database names smtp.hosted.example"), "the host isn't said to come from the database");
    assert!(!says(&h, "publishes for mail programs names") && !says(&h, "imap.lumen.example"));
    assert!(checked.lock().unwrap().is_empty());
}

#[test]
fn nothing_found_leaves_the_guess_as_it_was() {
    let mut a = app(false);
    checks(&mut a, None);
    let h = with_found(a, Discovered::from_exchangers(vec!["mx.lumen.example".into()]));
    let w = wizard(&h).unwrap();
    assert_eq!((w.found, w.guessed, w.incoming.host.as_str(), w.outgoing.host.as_str()), (None, true, "imap.lumen.example", "smtp.lumen.example"));
    assert!(says(&h, "only a guess") && !says(&h, "found in the DNS records"));
}
