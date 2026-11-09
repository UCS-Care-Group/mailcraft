// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a sign-in token may go (AU3-19), as a build that isn't this crate's own test build has
//! it: every OAuth provider against every kind of server a mailbox file could name. Only the
//! provider's own mail servers take its token; another provider's, any other host, an address
//! on this computer and a bare IP address are refused before anything is connected to.

use std::net::TcpListener;

use mailcraft_model::sync::SyncRequest;
use mailcraft_model::{Account, AccountKind, OAuth, OAuthProvider, Security, Server};
use mailcraft_net::token_may_go_to;

/// Every provider. (A new one fails to build here until it has its row.)
fn providers() -> Vec<OAuthProvider> {
    let all = [OAuthProvider::Microsoft, OAuthProvider::Google];
    for provider in all {
        match provider {
            OAuthProvider::Microsoft | OAuthProvider::Google => {}
        }
    }
    all.to_vec()
}

/// The provider's own IMAP and SMTP servers.
fn own_hosts(provider: OAuthProvider) -> [&'static str; 2] {
    match provider {
        OAuthProvider::Microsoft => ["outlook.office365.com", "smtp.office365.com"],
        OAuthProvider::Google => ["imap.gmail.com", "smtp.gmail.com"],
    }
}

/// The servers no token goes to: somebody else's, look-alikes, this computer, bare addresses.
const REFUSED: &[&str] = &[
    "imap.attacker.example",
    "outlook.office365.com.attacker.example",
    "office365.com-mail.example",
    "imap.gmail.com.attacker.example",
    // Gmail's API takes the Google token for one call of its own (#911), never as a mail server.
    "gmail.googleapis.com",
    "127.0.0.1",
    "127.1.2.3",
    "localhost",
    "LOCALHOST.",
    "::1",
    "[::1]",
    "192.0.2.10",
    "10.0.0.1",
    "2001:db8::1",
    "[2001:db8::1]",
];

fn account(provider: OAuthProvider, host: &str) -> Account {
    let server = Server { host: host.into(), port: 993, security: Security::Tls };
    Account {
        id: 1,
        kind: AccountKind::Imap,
        email: "avery@mail.example".into(),
        name: "Avery".into(),
        incoming: Some(server.clone()),
        outgoing: Some(Server { port: 465, ..server }),
        oauth: Some(OAuth { provider, ..Default::default() }),
        ..Default::default()
    }
}

/// Whether the token of `provider` may go to `host` for IMAP and for SMTP.
fn allowed(provider: OAuthProvider, host: &str) -> [bool; 2] {
    let a = account(provider, host);
    let server = a.incoming.clone().unwrap();
    [token_may_go_to(&a, &server, "imap.gmail.com").is_ok(), token_may_go_to(&a, &server, "smtp.gmail.com").is_ok()]
}

#[test]
fn a_token_goes_to_its_providers_own_servers_and_nowhere_else() {
    for provider in providers() {
        let [imap, smtp] = own_hosts(provider);
        // Each of the provider's servers takes the token for its own protocol.
        assert!(allowed(provider, imap)[0], "{provider:?} {imap}");
        assert!(allowed(provider, smtp)[1], "{provider:?} {smtp}");
        // Another provider's servers never do.
        for other in providers().into_iter().filter(|other| *other != provider) {
            for host in own_hosts(other) {
                assert_eq!(allowed(provider, host), [false, false], "{provider:?} token to {other:?}'s {host}");
            }
        }
        for host in REFUSED {
            assert_eq!(allowed(provider, host), [false, false], "{provider:?} token to {host}");
        }
    }
}

#[test]
fn an_account_without_a_sign_in_page_is_not_judged_by_its_host() {
    // (A password goes where the account says, encrypted: this check is about tokens alone.)
    let mut a = account(OAuthProvider::Microsoft, "127.0.0.1");
    a.oauth = None;
    let server = a.incoming.clone().unwrap();
    assert!(token_may_go_to(&a, &server, "imap.gmail.com").is_ok());
}

#[test]
fn a_microsoft_account_at_a_loopback_server_is_refused_before_anything_connects() {
    for host in ["127.0.0.1", "localhost"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut a = account(OAuthProvider::Microsoft, host);
        for server in [a.incoming.as_mut().unwrap(), a.outgoing.as_mut().unwrap()] {
            server.port = port;
            server.security = Security::None;
        }
        // Send/Receive (IMAP) with a token in hand.
        let request = SyncRequest { account: a.clone(), access_token: "dummy-access-token".into(), ..Default::default() };
        let result = mailcraft_net::sync(&request);
        assert!(result.errors.iter().any(|e| e.contains("Microsoft")), "{host}: {:?}", result.errors);
        assert!(result.errors.iter().all(|e| !e.contains("dummy-access-token")));
        // Sending one message (SMTP).
        let message = mailcraft_model::Message {
            from: mailcraft_model::Address::new("Avery", "avery@mail.example"),
            to: vec![mailcraft_model::Address::new("Bo", "bo@example.org")],
            subject: "Hello".into(),
            body: mailcraft_model::Body { text: "Hi".into(), html: None },
            ..Default::default()
        };
        let sent = mailcraft_net::send(&a, "dummy-access-token", &message).unwrap_err().to_string();
        assert!(sent.contains("Microsoft"), "{host}: {sent}");
        // Nothing reached the listener either way.
        assert_eq!(listener.accept().map(|_| ()).map_err(|e| e.kind()), Err(std::io::ErrorKind::WouldBlock), "{host}");
    }
}
