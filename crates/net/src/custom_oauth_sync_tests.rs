// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An account that signs in with an own identity provider, end to end against the fake IMAP and
//! SMTP servers on loopback: which way its token is given, and where it may go.

use mailcraft_model::sync::{FolderState, SyncRequest};
use mailcraft_model::{Account, AccountKind, Address, FolderKind, Message, OAuth, OAuthProvider, SaslChoice, Security, Server};

use crate::fake::{Fake, FakeImap, FakeMailbox, FakeSmtp, ImapWorld, SmtpWorld, oauthbearer_parts};
use crate::{NetError, send, sync, test_account};

const TOKEN: &str = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJhbGljZSJ9.c2ln";
const USER: &str = "alice";

fn servers(oauthbearer: bool, xoauth2: bool) -> (FakeImap, FakeSmtp) {
    let inbox =
        FakeMailbox { raw: "INBOX".into(), flags: "\\HasNoChildren".into(), uid_validity: 1, messages: Vec::new(), refuse_open: false, next_uid: 0 };
    let imap = Fake::imap(ImapWorld {
        user: USER.into(),
        token: TOKEN.into(),
        oauthbearer,
        no_xoauth2: !xoauth2,
        mailboxes: vec![inbox],
        ..Default::default()
    });
    let smtp = Fake::smtp(SmtpWorld { user: USER.into(), token: TOKEN.into(), oauthbearer, no_xoauth2: !xoauth2, ..Default::default() });
    (imap, smtp)
}

fn account(imap: &FakeImap, smtp: &FakeSmtp, mechanism: SaslChoice) -> Account {
    let server = |port: u16| Some(Server { host: "127.0.0.1".into(), port, security: Security::None });
    Account {
        id: 3,
        email: "alice@example.org".into(),
        username: USER.into(),
        kind: AccountKind::Imap,
        incoming: server(imap.port),
        outgoing: server(smtp.port),
        oauth: Some(OAuth {
            provider: OAuthProvider::Custom,
            client_id: "mailcraft".into(),
            authorize: "https://id.example.org/auth".into(),
            token: "https://id.example.org/token".into(),
            mechanism,
            ..OAuth::default()
        }),
        ..Default::default()
    }
}

fn hello() -> Message {
    Message {
        id: 9,
        from: Address::new("Alice", "alice@example.org"),
        to: vec![Address::new("Bob", "bob@example.org")],
        subject: "Hello".into(),
        ..Default::default()
    }
}

fn used(log: &[String]) -> Vec<&str> {
    log.iter().filter_map(|l| l.split_whitespace().find(|w| *w == "OAUTHBEARER" || *w == "XOAUTH2")).collect()
}

#[test]
fn oauthbearer_is_preferred_when_offered_and_carries_the_servers_host_and_port() {
    let (imap, smtp) = servers(true, true);
    let a = account(&imap, &smtp, SaslChoice::Auto);
    assert_eq!(test_account(&a, TOKEN), Ok(()));
    assert_eq!(send(&a, TOKEN, &hello()), Ok(()));
    let imap_log = imap.world().log.clone();
    assert_eq!(used(&imap_log).first(), Some(&"OAUTHBEARER"), "{imap_log:?}");
    assert!(used(&smtp.world().log).iter().all(|m| *m == "OAUTHBEARER"));
    // What went out names the server it went to.
    let payload = imap_log.iter().find(|l| !l.contains(' ') && l.len() > 40).cloned().unwrap_or_default();
    let (user, host, port, token) = oauthbearer_parts(&payload);
    assert_eq!((user.as_str(), host.as_str(), port, token.as_str()), (USER, "127.0.0.1", imap.port.to_string(), TOKEN));
    assert_eq!(smtp.world().delivered.len(), 1);
}

#[test]
fn xoauth2_is_used_when_it_is_all_the_server_takes_or_what_the_settings_say() {
    let (imap, smtp) = servers(false, true);
    let a = account(&imap, &smtp, SaslChoice::Auto);
    assert_eq!(test_account(&a, TOKEN), Ok(()));
    assert!(used(&imap.world().log).iter().all(|m| *m == "XOAUTH2"));
    let (imap, smtp) = servers(true, true);
    let a = account(&imap, &smtp, SaslChoice::XOAuth2);
    assert_eq!(test_account(&a, TOKEN), Ok(()));
    assert_eq!(send(&a, TOKEN, &hello()), Ok(()));
    assert!(used(&imap.world().log).iter().all(|m| *m == "XOAUTH2"));
    assert!(used(&smtp.world().log).iter().all(|m| *m == "XOAUTH2"));
}

#[test]
fn a_mechanism_the_server_doesnt_offer_is_an_error_and_the_token_never_goes() {
    for (bearer, xoauth2, choice) in [(false, true, SaslChoice::OAuthBearer), (true, false, SaslChoice::XOAuth2), (false, false, SaslChoice::Auto)] {
        let (imap, smtp) = servers(bearer, xoauth2);
        // (A server that takes no token at all: one that says so.)
        if !bearer && !xoauth2 {
            imap.world().token = String::new();
            smtp.world().token = String::new();
        }
        let a = account(&imap, &smtp, choice);
        let error = test_account(&a, TOKEN).unwrap_err();
        assert!(matches!(error, NetError::Proto(mailcraft_proto::ProtoError::Auth(_))), "{choice:?}: {error:?}");
        assert!(!imap.world().log.iter().any(|l| l.contains("AUTHENTICATE")), "{choice:?}: {:?}", imap.world().log);
        let error = send(&a, TOKEN, &hello()).unwrap_err();
        assert!(!format!("{error:?}").contains(TOKEN));
        assert!(!smtp.world().log.iter().any(|l| l.starts_with("AUTH")), "{choice:?}: {:?}", smtp.world().log);
    }
}

#[test]
fn a_refused_token_says_why_without_showing_it() {
    let (imap, smtp) = servers(true, false);
    let a = account(&imap, &smtp, SaslChoice::Auto);
    let error = test_account(&a, "eyJ.stale.token").unwrap_err();
    assert!(error.is_refused_sign_in(), "{error:?}");
    assert!(!format!("{error} {error:?}").contains("eyJ.stale.token"));
    // The server's reasons were answered as RFC 7628 has it.
    assert!(imap.world().log.iter().any(|l| l == "AQ=="), "{:?}", imap.world().log);
    let error = send(&a, "eyJ.stale.token", &hello()).unwrap_err();
    assert!(!format!("{error} {error:?}").contains("eyJ.stale.token"));
    assert!(smtp.world().log.iter().any(|l| l == "AQ=="));
}

#[test]
fn mail_comes_down_and_goes_out_on_the_token() {
    let (imap, smtp) = servers(true, true);
    let a = account(&imap, &smtp, SaslChoice::Auto);
    let folders: Vec<FolderState> =
        FolderKind::STANDARD.iter().zip(100..).map(|(kind, id)| FolderState { id, kind: *kind, ..Default::default() }).collect();
    let request = SyncRequest { account: a, access_token: TOKEN.into(), folders, outbox: vec![hello()], ..Default::default() };
    let result = sync(&request);
    assert_eq!(result.errors, Vec::<String>::new());
    assert_eq!(result.sent, [9]);
}

#[test]
fn the_token_never_goes_to_a_server_the_account_doesnt_name() {
    let (imap, smtp) = servers(true, true);
    let mut a = account(&imap, &smtp, SaslChoice::Auto);
    // Edited to point at Google's server after signing in: refused before anything connects.
    a.incoming = Some(Server { host: "imap.gmail.com".into(), port: 993, security: Security::Tls });
    let error = test_account(&a, TOKEN).unwrap_err();
    assert!(matches!(error, NetError::Config(_)), "{error:?}");
    assert_eq!(imap.world().connections, 0);
}

#[test]
fn starttls_missing_is_refused_before_any_token_is_sent() {
    let (imap, smtp) = servers(true, true);
    let mut a = account(&imap, &smtp, SaslChoice::Auto);
    a.incoming.as_mut().unwrap().security = Security::StartTls;
    a.outgoing.as_mut().unwrap().security = Security::StartTls;
    assert!(matches!(test_account(&a, TOKEN), Err(NetError::Tls { .. })));
    assert!(!imap.world().log.iter().any(|l| l.contains("AUTHENTICATE")));
    assert!(matches!(send(&a, TOKEN, &hello()), Err(NetError::Tls { .. })));
    assert!(!smtp.world().log.iter().any(|l| l.starts_with("AUTH")));
}
