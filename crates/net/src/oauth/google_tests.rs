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
