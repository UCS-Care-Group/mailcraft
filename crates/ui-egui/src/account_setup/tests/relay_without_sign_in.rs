// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #836: the outgoing server may be a relay that takes mail without a sign-in.

use super::*;

/// A mail exchanger that is nobody's this program knows.
const HOSTED_MX: &str = "mx.hosted.example";

/// Account Settings for an IMAP account with these servers.
fn settings_for(incoming: Server, outgoing: Option<Server>) -> (Harness<'static, MailApp>, Id) {
    let mut a = app(true);
    let server = |s: &Server| json!({"host": s.host, "port": s.port, "security": s.security.code()});
    let mut params = json!({"email": "noor@lumen.example", "incoming": server(&incoming)});
    if let Some(outgoing) = &outgoing {
        params["outgoing"] = server(outgoing);
    }
    let id = a.execute("account.add", params).unwrap()["id"].as_u64().unwrap();
    let h = open(a, json!({"id": id}));
    (h, id)
}

#[test]
fn an_outgoing_relay_needs_no_sign_in_when_the_owner_says_so() {
    let incoming = Server { host: "imap.lumen.example".into(), port: 0, security: Security::Tls };
    let relay = Server { host: "relay.lumen.example".into(), port: 25, security: Security::StartTls };
    let (mut h, id) = settings_for(incoming, Some(relay));
    h.run();
    let tick = "My outgoing server needs no sign-in";
    assert!(h.query_by_label(tick).is_some());
    h.get_by_label(tick).click();
    h.run();
    let w = wizard(&h).unwrap();
    assert!(w.outgoing_without_sign_in);
    assert_eq!(w.edit_command(), Some(("account.edit", json!({"id": id, "outgoingWithoutSignIn": true}))));
    assert!(w.account().unwrap().outgoing_without_sign_in);
    // Without an outgoing server there is nothing to tick, and nothing of it is tried.
    wizard_mut(&mut h).outgoing.host.clear();
    h.run();
    assert!(h.query_by_label(tick).is_none());
    assert!(!wizard(&h).unwrap().account().is_some_and(|a| a.outgoing_without_sign_in));
}

#[test]
fn a_new_account_on_a_relay_is_added_with_it_and_one_signing_in_on_a_providers_page_never_is() {
    let mut w = AccountSetup::blank(Page::Password);
    w.email = "noor@lumen.example".into();
    w.incoming = ServerFields { host: "imap.lumen.example".into(), port: String::new(), security: Security::Tls };
    w.outgoing = ServerFields { host: "relay.lumen.example".into(), port: "25".into(), security: Security::StartTls };
    w.outgoing_without_sign_in = true;
    let (_, params) = w.add_command("").unwrap();
    assert_eq!(params["outgoingWithoutSignIn"], true);
    let mut a = app(true);
    let id = a.execute("account.add", params).unwrap()["id"].as_u64().unwrap();
    assert!(a.session.mailbox.account(id).unwrap().outgoing_without_sign_in);

    w.oauth = Some(OAuth::default());
    let (_, params) = w.add_command("").unwrap();
    assert!(params.get("outgoingWithoutSignIn").is_none());
    assert!(!w.account().unwrap().outgoing_without_sign_in);
}

#[test]
fn a_relay_tick_does_not_follow_the_owner_to_a_providers_page() {
    // A custom domain, its Server settings, the tick, Back, and then an address at a provider that
    // is known: the tick is hidden there, so it mustn't stay set and keep the password from the
    // provider's own SMTP server.
    let mut a = app(false);
    lookups(&mut a, Some(Ok(vec![HOSTED_MX])));
    let mut h = open(a, json!({}));
    connect(&mut h, "noor@lumen.example");
    assert_eq!(page(&h), Some(Page::Password));
    let tick = "My outgoing server needs no sign-in";
    h.get_by_label(tick).click();
    h.run();
    assert!(wizard(&h).unwrap().outgoing_without_sign_in);
    click(&mut h, "Back");
    assert_eq!(page(&h), Some(Page::Address));

    wizard_mut(&mut h).email = "noor@gmx.net".into();
    h.run();
    press(&mut h, Key::Enter);
    assert_eq!(page(&h), Some(Page::Password));
    let w = wizard(&h).unwrap();
    assert!(!w.settings_open && !w.guessed);
    assert!(!w.outgoing_without_sign_in);
    let (_, params) = w.add_command("").unwrap();
    assert!(params.get("outgoingWithoutSignIn").is_none(), "{params}");
    assert!(!w.account().unwrap().outgoing_without_sign_in);
}

#[test]
fn another_outgoing_host_is_saved_with_what_the_tick_shows() {
    let incoming = Server { host: "imap.lumen.example".into(), port: 0, security: Security::Tls };
    let relay = Server { host: "relay.lumen.example".into(), port: 25, security: Security::StartTls };
    let (mut h, _) = settings_for(incoming, Some(relay));
    h.run();
    h.get_by_label("My outgoing server needs no sign-in").click();
    h.run();
    // Saved with the tick, as the page shows it.
    let (cmd, params) = wizard(&h).unwrap().edit_command().unwrap();
    assert_eq!((cmd, &params["outgoingWithoutSignIn"]), ("account.edit", &json!(true)));
    // A host that is only another one: the tick is still in the command, whichever way it is.
    wizard_mut(&mut h).outgoing.host = "relay2.lumen.example".into();
    h.run();
    let (_, params) = wizard(&h).unwrap().edit_command().unwrap();
    assert_eq!(params["outgoing"]["host"], "relay2.lumen.example");
    assert_eq!(params["outgoingWithoutSignIn"], true);
    wizard_mut(&mut h).outgoing_without_sign_in = false;
    let (_, params) = wizard(&h).unwrap().edit_command().unwrap();
    assert_eq!(params["outgoingWithoutSignIn"], false);
}

#[test]
fn the_page_says_the_connection_test_cannot_tell_whether_the_relay_wants_a_sign_in() {
    let incoming = Server { host: "imap.lumen.example".into(), port: 0, security: Security::Tls };
    let relay = Server { host: "relay.lumen.example".into(), port: 25, security: Security::StartTls };
    let (mut h, _) = settings_for(incoming, Some(relay));
    h.run();
    let hint = "The connection test only checks that this server answers";
    // Only where the tick is set: otherwise the test signs in, and there is nothing to add.
    assert!(!says(&h, hint));
    h.get_by_label("My outgoing server needs no sign-in").click();
    h.run();
    assert!(says(&h, hint));
    wizard_mut(&mut h).outgoing_without_sign_in = false;
    h.run();
    assert!(!says(&h, hint));
}

#[test]
fn the_tick_takes_the_keyboard_like_the_fields_around_it() {
    let incoming = Server { host: "imap.lumen.example".into(), port: 0, security: Security::Tls };
    let relay = Server { host: "relay.lumen.example".into(), port: 25, security: Security::StartTls };
    let (mut h, _) = settings_for(incoming, Some(relay));
    h.run();
    let tick = "My outgoing server needs no sign-in";
    h.get_by_label(tick).focus();
    h.run_steps(2);
    // Enter on the focused tick is the tick's (it is not taken for OK), and the page stays.
    press(&mut h, Key::Enter);
    assert!(wizard(&h).is_some_and(|w| w.outgoing_without_sign_in), "Enter toggles the focused tick");
    assert!(wizard(&h).is_some(), "the settings stay open");
}
