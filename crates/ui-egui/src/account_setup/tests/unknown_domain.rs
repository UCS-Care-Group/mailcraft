// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #761: an address whose domain and mail exchangers nobody here knows. Its servers are a guess
//! made from the domain, never another provider's, and the page says so plainly; the other kinds
//! of account it may be (Microsoft 365, an Exchange server) are offered on the same page; and a
//! server that refuses a password on that connection is said in the interface language.

use super::*;

/// A mail exchanger that is nobody's this program knows.
const HOSTED_MX: &str = "mx.hosted.example";
/// What the IMAP code says when a server announces `LOGINDISABLED`.
const NO_PASSWORD: &str = "sign-in failed: the server doesn't take a password on this connection";

/// The password page for an address at a domain nobody knows.
fn unknown_domain(mut a: MailApp) -> Harness<'static, MailApp> {
    lookups(&mut a, Some(Ok(vec![HOSTED_MX])));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    assert_eq!(page(&h), Some(Page::Password));
    h
}

/// Whether a tile for `kind` is on the page.
fn offers(h: &Harness<'static, MailApp>, kind: Kind) -> bool {
    h.query_by_role_and_label(Role::Button, kind.title()).is_some()
}

#[test]
fn an_unknown_domain_is_guessed_from_its_own_name_and_said_to_be_a_guess() {
    let mut a = app(false);
    checks(&mut a, None);
    let h = unknown_domain(a);
    let w = wizard(&h).unwrap();
    assert!(w.guessed && w.settings_open && w.oauth.is_none() && w.service.is_empty());
    // Its own name, never another provider's servers. (So it was before #761 too; kept here
    // because the issue saw Microsoft's servers for such an address.)
    assert_eq!((w.incoming.host.as_str(), w.outgoing.host.as_str()), ("imap.lumen.example", "smtp.lumen.example"));
    assert!(!w.incoming.host.contains("office365") && !w.outgoing.host.contains("office365"));
    // Said in plain words: the settings aren't known, and the servers are only a guess.
    assert!(says(&h, "doesn't know the settings for this address"), "the page doesn't say the settings aren't known");
    assert!(says(&h, "only a guess"), "the page doesn't say the servers are a guess");
    assert!(!says(&h, "couldn't find the settings"));
}

#[test]
fn an_unknown_domain_offers_microsoft_365_and_an_exchange_server() {
    let mut a = app(false);
    checks(&mut a, None);
    let h = unknown_domain(a);
    assert!(offers(&h, Kind::Microsoft365) && offers(&h, Kind::Exchange));
    // A provider the exchangers name has no such offer: its servers are known.
    let mut a = app(false);
    lookups(&mut a, Some(Ok(vec!["aspmx.l.google.com"])));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@example.org");
    assert_eq!(page(&h), Some(Page::Password));
    assert!(!offers(&h, Kind::Microsoft365) && !offers(&h, Kind::Exchange));
}

#[test]
fn the_microsoft_365_offer_signs_in_on_microsofts_page_and_keeps_no_password() {
    let mut a = app(false);
    a.services.microsoft_client_id = APP_ID.into();
    let signed = sign_ins(&mut a, None);
    checks(&mut a, None);
    let mut h = unknown_domain(a);
    type_password(&mut h);
    click(&mut h, Kind::Microsoft365.title());
    assert_eq!(page(&h), Some(Page::SignIn));
    let w = wizard(&h).unwrap();
    assert_eq!((w.service.as_str(), w.oauth.as_ref().map(|o| o.provider), w.guessed), ("Microsoft 365", Some(OAuthProvider::Microsoft), false));
    assert!(w.password.0.is_empty() && TextEdit::load_state(&h.ctx, password_id()).is_none());
    assert_eq!(signed.lock().unwrap().first().map(|(email, _, _)| email.clone()), Some("noor@lumen.example".to_string()));
}

#[test]
fn the_exchange_server_offer_opens_its_page_and_keeps_no_password() {
    let mut a = app(false);
    checks(&mut a, None);
    let mut h = unknown_domain(a);
    type_password(&mut h);
    click(&mut h, Kind::Exchange.title());
    assert_eq!(page(&h), Some(Page::Exchange));
    let w = wizard(&h).unwrap();
    assert!(w.password.0.is_empty() && w.oauth.is_none() && w.email == "noor@lumen.example");
}

#[test]
fn a_server_that_takes_no_password_is_said_with_the_guess_still_in_view() {
    let mut a = app(false);
    checks(&mut a, Some(Err(NO_PASSWORD)));
    let mut h = unknown_domain(a);
    type_password(&mut h);
    press(&mut h, Key::Enter);
    assert_eq!(page(&h), Some(Page::Password));
    assert!(says(&h, "doesn't take a password on this connection") && says(&h, "only a guess"));
    assert!(offers(&h, Kind::Microsoft365) && offers(&h, Kind::Exchange));
}

#[test]
fn back_from_a_kind_offered_beside_the_guess_leads_to_the_kinds_of_account() {
    // An Exchange server.
    let mut a = app(false);
    checks(&mut a, None);
    let mut h = unknown_domain(a);
    click(&mut h, Kind::Exchange.title());
    assert_eq!(page(&h), Some(Page::Exchange));
    assert!(wizard(&h).is_some_and(|w| !w.guessed));
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Choose));
    // Microsoft 365, in a build that still needs an application ID: its page, then the kinds.
    let mut a = app(false);
    let signed = sign_ins(&mut a, None);
    checks(&mut a, None);
    let mut h = unknown_domain(a);
    type_password(&mut h);
    click(&mut h, Kind::Microsoft365.title());
    assert_eq!(page(&h), Some(Page::ApplicationId));
    assert!(wizard(&h).is_some_and(|w| w.password.0.is_empty() && !w.guessed) && signed.lock().unwrap().is_empty());
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Choose));
}

#[test]
fn a_server_that_takes_no_password_is_said_in_german_on_the_page() {
    let _language = crate::i18n::RestoreLanguage::hold();
    let mut a = app(false);
    a.execute("ui.language", json!({"language": "de"})).unwrap();
    checks(&mut a, Some(Err(NO_PASSWORD)));
    let mut h = unknown_domain(a);
    type_password(&mut h);
    press(&mut h, Key::Enter);
    assert_eq!(page(&h), Some(Page::Password));
    let said = wizard(&h).map(|w| w.error.clone()).unwrap_or_default();
    assert!(said.contains("Passwort") && !said.contains("doesn't take a password"), "{said}");
    assert!(says(&h, &said));
}

#[test]
fn a_refusal_of_a_password_is_said_in_the_interface_language() {
    let _language = crate::i18n::RestoreLanguage::hold();
    crate::i18n::set_current(crate::i18n::Lang::German);
    let said = in_own_words(NO_PASSWORD);
    assert!(!said.is_empty() && said != NO_PASSWORD, "{said}");
    assert!(said.contains("Passwort"), "{said}");
    // What a server said for itself is left as it said it.
    for other in ["sign-in failed: Basic authentication is disabled.", "connection problem: the server took too long to answer", ""] {
        assert_eq!(in_own_words(other), other);
    }
}

#[test]
fn a_google_server_that_takes_no_password_here_isnt_said_to_want_an_app_password() {
    // An app password is a password too: what helps is a connection that takes one.
    let mut a = app(false);
    checks(&mut a, Some(Err(NO_PASSWORD)));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@gmail.com");
    type_password(&mut h);
    press(&mut h, Key::Enter);
    assert_eq!(page(&h), Some(Page::Password));
    assert!(says(&h, "doesn't take a password on this connection"));
    assert!(!says(&h, "Google didn't accept the password") && wizard(&h).is_some_and(|w| !w.app_passwords));
}
