// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Google OAuth regression tests use a fake token service and an in-process loopback browser.
//! The test identities are invented example.org addresses. No Google account is contacted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;
use crate::fake::{FakeHttp, json_answer};

const PATIENCE: Duration = Duration::from_secs(10);
const STAND_IN_SECRET: &str = "not-a-real-desktop-secret";

fn google() -> OAuth {
    OAuth { provider: OAuthProvider::Google, client_id: "test-google-desktop-client".into(), ..Default::default() }
}

fn endpoint(server: &FakeHttp) -> Endpoints {
    Endpoints { authorize: server.url("/authorize"), token: server.url("/token"), ..Endpoints::of(&google()) }
}

fn jwt(address: &str) -> String {
    let claims = serde_json::json!({ "email": address, "name": "Example User", "email_verified": true });
    format!("header.{}.signature", URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes()))
}

fn good(address: &str, expires: u32) -> Vec<u8> {
    json_answer(
        "200 OK",
        &serde_json::json!({
            "token_type": "Bearer",
            "scope": "https://mail.google.com/ openid email",
            "expires_in": expires,
            "access_token": "fake-access",
            "refresh_token": "fake-refresh",
            "id_token": jwt(address),
        })
        .to_string(),
    )
}

fn form(server: &FakeHttp, n: usize) -> HashMap<String, String> {
    http::pairs(server.request(n).split_once("\r\n\r\n").unwrap().1).into_iter().collect()
}

type Seen = Arc<Mutex<String>>;

/// Run the real loopback listener with a fake browser on a different thread.
/// A bad state is ignored rather than cancelling or substituting the owner's code.
fn browser(bad_state: bool) -> (impl Fn(&str) + use<>, Seen) {
    let seen = Seen::default();
    let state = seen.clone();
    let opener = move |url: &str| {
        *state.lock().unwrap() = url.into();
        let args: HashMap<String, String> = http::pairs(url.split_once('?').unwrap().1).into_iter().collect();
        let redirect = args["redirect_uri"].clone();
        let port: u16 = redirect.strip_prefix("http://127.0.0.1:").unwrap().parse().unwrap();
        let selected = if bad_state { "not-the-same-state".to_string() } else { args["state"].clone() };
        std::thread::spawn(move || {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            socket.set_read_timeout(Some(PATIENCE)).unwrap();
            let request = format!("GET /?code=synthetic-code&state={selected} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
            socket.write_all(request.as_bytes()).unwrap();
            let mut answer = Vec::new();
            let _ = socket.read_to_end(&mut answer);
        });
    };
    (opener, seen)
}

#[test]
fn google_is_a_distinct_mail_only_provider() {
    assert_eq!(OAuthProvider::parse("gmail"), Some(OAuthProvider::Google));
    assert_eq!(OAuthProvider::parse("GoogleWorkspace"), Some(OAuthProvider::Google));
    assert_eq!(serde_json::to_string(&OAuthProvider::Google).unwrap(), r#""google""#);
    assert_eq!(OAuthProvider::Microsoft.code(), "microsoft");
    // An existing Gmail app-password account is not silently converted to OAuth.
    let existing = mailcraft_model::provider::by_domain("gmail.com").unwrap();
    assert_eq!(existing.oauth, None);
    let chosen = mailcraft_model::provider::google(true);
    assert_eq!(chosen.oauth, Some(OAuthProvider::Google));
    assert_eq!(chosen.incoming.host, existing.incoming.host);
    assert_eq!(chosen.outgoing.host, existing.outgoing.host);
    let ends = Endpoints::of(&google());
    assert_eq!(ends.authorize, "https://accounts.google.com/o/oauth2/v2/auth");
    assert_eq!(ends.token, "https://oauth2.googleapis.com/token");
    assert_eq!(ends.scope, "https://mail.google.com/ openid email");
    assert!(ends.consent.is_empty() && ends.lesser.is_empty());
    assert!(Url::parse(&ends.authorize).is_ok());
    assert!(Url::parse(&ends.token).is_ok());
    assert!(matches!(sign_in_for(&google(), "one@example.org", &|_| {}, &AtomicBool::new(false), Resource::Graph), Err(NetError::Config(_))));
    assert!(matches!(refresh_for(&google(), "fake-refresh", Resource::GraphMail), Err(NetError::Config(_))));
}

#[test]
fn google_authorization_uses_pkce_state_ipv4_loopback_and_desktop_secret() {
    let server = FakeHttp::answering(good("one@example.org", 3600));
    let (open, seen) = browser(false);
    let got = sign_in_google_at(&endpoint(&server), &google(), STAND_IN_SECRET, "one@example.org", &open, &AtomicBool::new(false)).unwrap();
    assert_eq!((got.access.as_str(), got.refresh.as_str(), got.email.as_str()), ("fake-access", "fake-refresh", "one@example.org"));
    assert_eq!(server.asked(), 1);
    let page = seen.lock().unwrap().clone();
    assert!(page.starts_with(&server.url("/authorize")));
    let query: HashMap<String, String> = http::pairs(page.split_once('?').unwrap().1).into_iter().collect();
    assert_eq!(query["access_type"], "offline");
    assert_eq!(query["prompt"], "consent select_account");
    assert_eq!(query["scope"], GOOGLE_SCOPE);
    assert_eq!(query["login_hint"], "one@example.org");
    assert_eq!(query["code_challenge_method"], "S256");
    assert!(query["redirect_uri"].starts_with("http://127.0.0.1:"));
    assert!(query["state"].len() >= 32);
    let sent = form(&server, 0);
    assert_eq!(sent["grant_type"], "authorization_code");
    assert_eq!(sent["client_id"], "test-google-desktop-client");
    assert_eq!(sent["client_secret"], STAND_IN_SECRET);
    assert_eq!(sent["code"], "synthetic-code");
    assert_eq!(sent["redirect_uri"], query["redirect_uri"]);
    assert_eq!(challenge(&sent["code_verifier"]), query["code_challenge"]);
    assert!(!page.contains(&sent["code_verifier"]));
    assert!(!page.contains(STAND_IN_SECRET));
    assert!(!format!("{got:?}").contains("fake-access"));
    assert!(!format!("{got:?}").contains("fake-refresh"));
}

#[test]
fn google_cancelled_and_mismatched_state_never_reach_token_server() {
    let server = FakeHttp::answering(good("one@example.org", 3600));
    let untouched = Mutex::new(String::new());
    let open = |page: &str| *untouched.lock().unwrap() = page.into();
    let cancel = AtomicBool::new(true);
    assert_eq!(sign_in_google_at(&endpoint(&server), &google(), STAND_IN_SECRET, "one@example.org", &open, &cancel), Err(NetError::SignInCancelled));
    assert!(untouched.lock().unwrap().is_empty());

    let (open, _) = browser(true);
    let rejected = google::sign_in_google_refused(
        &endpoint(&server),
        &google(),
        STAND_IN_SECRET,
        "one@example.org",
        &open,
        &AtomicBool::new(false),
        Duration::from_millis(300),
    );
    assert!(matches!(rejected, Err(Refused { error: NetError::SignInTimedOut, .. })));
    assert_eq!(server.asked(), 0);

    let cancel = Arc::new(AtomicBool::new(false));
    let copied = cancel.clone();
    let open = move |_: &str| {
        let flag = copied.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            flag.store(true, Ordering::Relaxed);
        });
    };
    let cancelled = google::sign_in_google_refused(&endpoint(&server), &google(), STAND_IN_SECRET, "one@example.org", &open, &cancel, PATIENCE);
    assert!(matches!(cancelled, Err(Refused { error: NetError::SignInCancelled, .. })));
    assert_eq!(server.asked(), 0);
}

#[test]
fn google_denied_consent_never_exchanges_a_code() {
    let server = FakeHttp::answering(good("one@example.org", 3600));
    let open = |url: &str| {
        let query: HashMap<String, String> = http::pairs(url.split_once('?').unwrap().1).into_iter().collect();
        let port: u16 = query["redirect_uri"].strip_prefix("http://127.0.0.1:").unwrap().parse().unwrap();
        let state = query["state"].clone();
        std::thread::spawn(move || {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            let request = format!("GET /?error=access_denied&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
            socket.write_all(request.as_bytes()).unwrap();
            let mut answer = Vec::new();
            let _ = socket.read_to_end(&mut answer);
        });
    };
    let error = sign_in_google_at(&endpoint(&server), &google(), STAND_IN_SECRET, "one@example.org", &open, &AtomicBool::new(false)).unwrap_err();
    assert!(error.is_refused_sign_in());
    assert_eq!(server.asked(), 0);
    let classified =
        google::sign_in_google_refused(&endpoint(&server), &google(), STAND_IN_SECRET, "one@example.org", &open, &AtomicBool::new(false), PATIENCE)
            .map_err(SignInEnd::from);
    assert_eq!(classified, Err(SignInEnd::Declined));
    assert_eq!(server.asked(), 0);
}

#[test]
fn google_refuses_malformed_missing_scope_and_unidentified_successes() {
    let malformed = FakeHttp::answering(json_answer("200 OK", "not json"));
    let (open, _) = browser(false);
    let error = sign_in_google_at(&endpoint(&malformed), &google(), STAND_IN_SECRET, "", &open, &AtomicBool::new(false)).unwrap_err();
    assert!(matches!(error, NetError::Proto(ProtoError::Protocol(_))));

    let under = FakeHttp::answering(json_answer(
        "200 OK",
        r#"{"token_type":"Bearer","scope":"openid email","access_token":"under-scoped","id_token":"a.b.c"}"#,
    ));
    let (open, _) = browser(false);
    let err = sign_in_google_at(&endpoint(&under), &google(), STAND_IN_SECRET, "", &open, &AtomicBool::new(false)).unwrap_err();
    assert!(err.is_refused_sign_in());

    let unknown = FakeHttp::answering(json_answer(
        "200 OK",
        r#"{"token_type":"Bearer","scope":"https://mail.google.com/ openid email","access_token":"fake-access"}"#,
    ));
    let (open, _) = browser(false);
    let err = sign_in_google_at(&endpoint(&unknown), &google(), STAND_IN_SECRET, "", &open, &AtomicBool::new(false)).unwrap_err();
    assert!(err.is_refused_sign_in());
}

#[test]
fn google_refresh_preserves_account_identity_scope_and_expiry() {
    let server = FakeHttp::answering(json_answer(
        "200 OK",
        // RFC 6749 permits omitting `scope` when it is unchanged from the original grant.
        r#"{"token_type":"Bearer","access_token":"next-access","expires_in":0}"#,
    ));
    let old = "synthetic-refresh-one";
    let refreshed = refresh_google_at(&endpoint(&server), &google(), STAND_IN_SECRET, old).unwrap();
    assert_eq!((refreshed.access.as_str(), refreshed.refresh.as_str()), ("next-access", old));
    assert!(!refreshed.fresh(mailcraft_time::now()));
    let sent = form(&server, 0);
    assert_eq!(sent["grant_type"], "refresh_token");
    assert_eq!(sent["client_secret"], STAND_IN_SECRET);
    assert_eq!(sent["refresh_token"], old);
    assert!(!format!("{refreshed:?}").contains(old));
    let error = refresh_google_at(&endpoint(&server), &google(), STAND_IN_SECRET, "").unwrap_err();
    assert!(error.is_refused_sign_in());
    assert_eq!(server.asked(), 1);
}

#[test]
fn google_refresh_error_redacts_all_sent_credentials() {
    let old = "synthetic-refresh-private";
    let fake_error = format!(r#"{{"error":"invalid_grant","error_description":"Received {old} and {STAND_IN_SECRET} by mistake"}}"#);
    let server = FakeHttp::answering(json_answer("400 Bad Request", &fake_error));
    let error = refresh_google_at(&endpoint(&server), &google(), STAND_IN_SECRET, old).unwrap_err();
    let output = format!("{error} {error:?}");
    assert!(!output.contains(old) && !output.contains(STAND_IN_SECRET), "{output}");
    assert!(error.is_refused_sign_in());
}

#[test]
fn google_unconfigured_registration_never_opens_browser() {
    let server = FakeHttp::answering(good("one@example.org", 3600));
    let called = AtomicBool::new(false);
    let open = |_: &str| called.store(true, Ordering::Relaxed);
    let oauth = OAuth { client_id: String::new(), ..google() };
    assert!(matches!(sign_in_google_at(&endpoint(&server), &oauth, STAND_IN_SECRET, "", &open, &AtomicBool::new(false)), Err(NetError::Config(_))));
    assert!(matches!(sign_in_google_at(&endpoint(&server), &google(), "", "", &open, &AtomicBool::new(false)), Err(NetError::Config(_))));
    assert!(!called.load(Ordering::Relaxed));
    assert_eq!(server.asked(), 0);
}

#[test]
fn two_google_accounts_refresh_independently() {
    let first = FakeHttp::answering(json_answer(
        "200 OK",
        r#"{"token_type":"Bearer","scope":"https://mail.google.com/","access_token":"a-one","expires_in":3600}"#,
    ));
    let second = FakeHttp::answering(json_answer(
        "200 OK",
        r#"{"token_type":"Bearer","scope":"https://mail.google.com/","access_token":"a-two","expires_in":3600}"#,
    ));
    let one = google();
    let two = OAuth { client_id: "other-google-desktop-registration".into(), ..google() };
    let a = refresh_google_at(&endpoint(&first), &one, STAND_IN_SECRET, "r-one").unwrap();
    let b = refresh_google_at(&endpoint(&second), &two, "other-test-secret", "r-two").unwrap();
    assert_eq!((a.access.as_str(), a.refresh.as_str()), ("a-one", "r-one"));
    assert_eq!((b.access.as_str(), b.refresh.as_str()), ("a-two", "r-two"));
    assert_eq!(form(&first, 0)["client_id"], "test-google-desktop-client");
    assert_eq!(form(&first, 0)["refresh_token"], "r-one");
    assert_eq!(form(&second, 0)["client_id"], "other-google-desktop-registration");
    assert_eq!(form(&second, 0)["refresh_token"], "r-two");
}

#[test]
fn google_refresh_invalid_grant_is_a_non_sensitive_revocation_notice() {
    let old = "synthetic-long-refresh-secret-to-erase";
    let echoed = format!(r#"{{"error":"invalid_grant","error_description":"Received {old} and {STAND_IN_SECRET} from the caller"}}"#);
    let server = FakeHttp::answering(json_answer("400 Bad Request", &echoed));
    let error = refresh_google_at(&endpoint(&server), &google(), STAND_IN_SECRET, old).unwrap_err();
    assert!(error.is_refused_sign_in());
    assert!(error.is_revoked_google_refresh());
    assert_eq!(error, NetError::Proto(ProtoError::Auth(GOOGLE_REFRESH_REVOKED.into())));
    let words = format!("{error} {error:?}");
    assert!(!words.contains(old));
    assert!(!words.contains(STAND_IN_SECRET));
    assert!(!words.contains("Received"));

    // A temporary denial or a connection problem must never trigger deletion of a saved token.
    let temporary = FakeHttp::answering(json_answer(
        "503 Service Unavailable",
        r#"{"error":"temporarily_unavailable","error_description":"The server is busy"}"#,
    ));
    let denied = refresh_google_at(&endpoint(&temporary), &google(), STAND_IN_SECRET, old).unwrap_err();
    assert!(!denied.is_revoked_google_refresh());
    assert!(!denied.is_refused_sign_in());
}

/// Connect the code received from a real localhost OAuth callback to both synthetic mail servers.
/// No Google service, real account or outbound email is involved.
fn integrated_google_servers(token: &str) -> (crate::fake::FakeImap, crate::fake::FakeSmtp) {
    use crate::fake::{Fake, FakeMailbox, FakeMessage, ImapWorld, SmtpWorld};
    let message = FakeMessage {
        uid: 1,
        flags: "\\Seen".into(),
        internal_date: " 1-Jul-2003 09:52:37 +0000".into(),
        body: b"From: Other <other@example.org>\r\nTo: One <one@example.org>\r\nSubject: Synthetic hello\r\nDate: Tue, 01 Jul 2003 10:52:37 +0200\r\nMessage-ID: <synthetic-1@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nSynthetic message.".to_vec(),
        claimed_size: None,
    };
    let imap = Fake::imap(ImapWorld {
        user: "one@example.org".into(),
        password: "unused-imap-app-password".into(),
        token: token.into(),
        mailboxes: vec![FakeMailbox {
            raw: "INBOX".into(),
            flags: "\\HasNoChildren".into(),
            uid_validity: 13,
            messages: vec![message],
            refuse_open: false,
            next_uid: 0,
        }],
        ..Default::default()
    });
    let smtp = Fake::smtp(SmtpWorld {
        user: "one@example.org".into(),
        password: "unused-smtp-app-password".into(),
        token: token.into(),
        auth: true,
        ..Default::default()
    });
    (imap, smtp)
}

fn integrated_google_account(imap: &crate::fake::FakeImap, smtp: &crate::fake::FakeSmtp) -> mailcraft_model::Account {
    use mailcraft_model::{Account, AccountKind, Security, Server};
    let loopback = |port| Server { host: "127.0.0.1".into(), port, security: Security::None };
    Account {
        id: 91,
        name: "Synthetic owner".into(),
        email: "one@example.org".into(),
        kind: AccountKind::Imap,
        incoming: Some(loopback(imap.port)),
        outgoing: Some(loopback(smtp.port)),
        // For OAuth, the IMAP/SMTP auth user is the verified account identity, not a password.
        username: String::new(),
        oauth: Some(google()),
        ..Default::default()
    }
}

#[test]
fn google_integration_pkce_callback_token_authenticates_imap_and_smtp_without_a_password() {
    use mailcraft_model::sync::SyncRequest;
    use mailcraft_model::{Address, Message};

    let provider = FakeHttp::answering(good("one@example.org", 3600));
    let (open, _) = browser(false);
    let tokens = sign_in_google_at(&endpoint(&provider), &google(), STAND_IN_SECRET, "one@example.org", &open, &AtomicBool::new(false)).unwrap();
    assert_eq!(tokens.email, "one@example.org");
    assert_eq!(provider.asked(), 1);
    let (imap, smtp) = integrated_google_servers(&tokens.access);
    let account = integrated_google_account(&imap, &smtp);

    // The access token obtained from the OAuth callback is sufficient for *both* protocols.
    assert_eq!(crate::test_account(&account, &tokens.access), Ok(()));
    let result = crate::sync(&SyncRequest {
        account: account.clone(),
        access_token: tokens.access.clone(),
        password: "must-not-leak-app-password".into(),
        ..Default::default()
    });
    assert!(result.errors.is_empty(), "synthetic IMAP sync errors: {:?}", result.errors);
    let message =
        Message { id: 2, to: vec![Address::new("Other", "other@example.org")], subject: "Synthetic SMTP test".into(), ..Default::default() };
    assert_eq!(crate::send(&account, &tokens.access, &message), Ok(()));
    assert_eq!(smtp.world().delivered.len(), 1);
    let (i, s) = (imap.world(), smtp.world());
    assert!(i.log.iter().any(|line| line.contains("AUTHENTICATE XOAUTH2")));
    assert!(s.log.iter().any(|line| line.starts_with("AUTH XOAUTH2")));
    assert!(i.log.iter().all(|line| !line.contains("LOGIN") && !line.contains("must-not-leak-app-password")));
    assert!(s.log.iter().all(|line| !line.contains("AUTH PLAIN") && !line.contains("must-not-leak-app-password")));
    assert!(!i.log.iter().chain(s.log.iter()).any(|line| line.contains(tokens.access.as_str())));
    assert_eq!(form(&provider, 0)["grant_type"], "authorization_code");
}

#[test]
fn google_integration_refused_expired_access_recovers_with_fake_refresh_and_no_token_leak() {
    let (imap, smtp) = integrated_google_servers("rotated-access");
    let account = integrated_google_account(&imap, &smtp);
    let expired = "expired-access-must-stay-private";
    let denied = crate::test_account(&account, expired).unwrap_err();
    assert!(denied.is_refused_sign_in());
    assert!(!format!("{denied} {denied:?}").contains(expired));
    assert_eq!(smtp.world().delivered.len(), 0);
    let response = json_answer("200 OK", r#"{"token_type":"Bearer","access_token":"rotated-access","expires_in":3600}"#);
    let provider = FakeHttp::answering(response);
    let fresh = refresh_google_at(&endpoint(&provider), &google(), STAND_IN_SECRET, "synthetic-refresh-unchanged").unwrap();
    assert_eq!(fresh.refresh, "synthetic-refresh-unchanged");
    assert_eq!(fresh.access, "rotated-access");
    assert_eq!(form(&provider, 0)["grant_type"], "refresh_token");
    assert_eq!(crate::test_account(&account, &fresh.access), Ok(()));
    let message = mailcraft_model::Message {
        id: 3,
        to: vec![mailcraft_model::Address::new("Other", "other@example.org")],
        subject: "Synthetic reconnected SMTP".into(),
        ..Default::default()
    };
    assert_eq!(crate::send(&account, &fresh.access, &message), Ok(()));
    assert_eq!(smtp.world().delivered.len(), 1);
    assert!(!imap.world().log.iter().chain(smtp.world().log.iter()).any(|line| line.contains(expired)));
}
