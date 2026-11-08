// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! #836: Server settings offer no encryption for a server on this computer alone (a mail bridge
//! on 127.0.0.1), say so for as long as it is chosen, and never save it for a server on the
//! network; and the outgoing server may be a relay that takes mail without a sign-in.

use super::*;

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

/// Whether the encryption choices of the page include None.
fn offers_none(h: &Harness<'static, MailApp>) -> bool {
    h.query_by_role_and_label(Role::RadioButton, "None").is_some()
}

#[test]
fn none_is_offered_for_a_server_on_this_computer_and_said_to_be_unencrypted() {
    let bridge = Server { host: "127.0.0.1".into(), port: 1143, security: Security::Tls };
    let (mut h, id) = settings_for(bridge, None);
    h.run();
    assert!(offers_none(&h));
    assert!(!says(&h, "Not encrypted"));
    // Chosen: said under the row as long as it is, and saved.
    wizard_mut(&mut h).incoming.security = Security::None;
    h.run();
    assert!(says(&h, "Not encrypted: for a server on this computer only"));
    let w = wizard(&h).unwrap();
    assert_eq!(w.edit_command(), Some(("account.edit", json!({"id": id, "incoming": {"host": "127.0.0.1", "port": 1143, "security": "none"}}))));

    // A server on the network isn't offered it.
    wizard_mut(&mut h).incoming = ServerFields { host: "imap.lumen.example".into(), port: String::new(), security: Security::Tls };
    h.run();
    assert!(!offers_none(&h));
}

#[test]
fn none_for_a_server_on_the_network_stays_in_view_and_is_never_saved() {
    let bridge = Server { host: "localhost".into(), port: 1143, security: Security::None };
    let (mut h, _) = settings_for(bridge.clone(), None);
    // The name changes to one on the network: None stays chosen and in view, in red words.
    // Nothing flips back by itself.
    wizard_mut(&mut h).incoming.host = "imap.lumen.example".into();
    h.run();
    assert!(offers_none(&h) && says(&h, "Not encrypted: only a server on this computer can be reached this way."));
    assert_eq!(wizard(&h).unwrap().incoming.security, Security::None);

    // And the page won't save it.
    let mut a = app(true);
    let id = a.execute("account.add", json!({"email": "noor@lumen.example", "incoming": {"host": "localhost", "security": "none"}})).unwrap()["id"]
        .as_u64()
        .unwrap();
    let mut w = AccountSetup::open(&a, &json!({"id": id})).unwrap();
    w.incoming.host = "imap.lumen.example".into();
    assert_eq!(w.edit_command(), None);
    assert!(w.account().is_none() && w.add_command("").is_none());
    assert!(!w.act(&mut a, Action::Go));
    assert!(w.error.contains("never sends your mail or password unencrypted"), "{}", w.error);
    assert_eq!(a.session.mailbox.account(id).unwrap().incoming, Some(Server { port: 0, ..bridge }));
    w.incoming.security = Security::StartTls;
    assert!(w.act(&mut a, Action::Go));
    assert_eq!(a.session.mailbox.account(id).unwrap().incoming.as_ref().unwrap().security, Security::StartTls);
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
    // Without an outgoing server there is nothing to tick.
    wizard_mut(&mut h).outgoing.host.clear();
    h.run();
    assert!(h.query_by_label(tick).is_none());
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
