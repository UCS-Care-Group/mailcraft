// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in with an own identity provider, against a fake token service and an in-process
//! loopback browser. Invented example.org identities; no real provider is contacted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mailcraft_model::ClientAuth;

use super::*;
use crate::fake::{FakeHttp, json_answer};

const PATIENCE: Duration = Duration::from_secs(10);
const SECRET: &str = "not-a-real-client-secret";

fn own(server: &FakeHttp) -> OAuth {
    OAuth {
        provider: OAuthProvider::Custom,
        client_id: "mailcraft-desktop".into(),
        issuer: server.url("/realms/mail"),
        authorize: server.url("/auth"),
        token: server.url("/token"),
        revoke: server.url("/revoke"),
        scopes: "mail".into(),
        ..OAuth::default()
    }
}

fn jwt(address: &str) -> String {
    let claims = serde_json::json!({ "email": address, "name": "Alice Example" });
    format!("header.{}.signature", URL_SAFE_NO_PAD.encode(claims.to_string().as_bytes()))
}

fn good(refresh: &str) -> Vec<u8> {
    let mut body = serde_json::json!({
        "token_type": "Bearer",
        "expires_in": 300,
        "access_token": "fake-access",
        "id_token": jwt("alice@example.org"),
    });
    if !refresh.is_empty() {
        body["refresh_token"] = serde_json::json!(refresh);
    }
    json_answer("200 OK", &body.to_string())
}

fn form(server: &FakeHttp, n: usize) -> HashMap<String, String> {
    http::pairs(server.request(n).split_once("\r\n\r\n").unwrap().1).into_iter().collect()
}

type Seen = Arc<Mutex<String>>;

/// A browser on another thread that comes back to the loopback port with a code, and the state
/// the page was given (or another).
fn browser(right_state: bool) -> (impl Fn(&str) + use<>, Seen) {
    browser_adding(right_state, "")
}

/// [`browser`], coming back with `extra` (`&iss=…`, say) after the code and state.
fn browser_adding(right_state: bool, extra: &str) -> (impl Fn(&str) + use<>, Seen) {
    let extra = extra.to_string();
    let seen = Seen::default();
    let kept = seen.clone();
    let opener = move |url: &str| {
        *kept.lock().unwrap() = url.into();
        let args: HashMap<String, String> = http::pairs(url.split_once('?').unwrap().1).into_iter().collect();
        let port: u16 = args["redirect_uri"].strip_prefix("http://127.0.0.1:").unwrap().trim_end_matches('/').parse().unwrap();
        let state = if right_state { args["state"].clone() } else { "somebody-elses".into() };
        let extra = extra.clone();
        std::thread::spawn(move || {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            socket.set_read_timeout(Some(PATIENCE)).unwrap();
            let request = format!("GET /?code=synthetic-code&state={state}{extra} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n");
            socket.write_all(request.as_bytes()).unwrap();
            let mut answer = Vec::new();
            let _ = socket.read_to_end(&mut answer);
        });
    };
    (opener, seen)
}

fn page_args(seen: &Seen) -> HashMap<String, String> {
    let page = seen.lock().unwrap().clone();
    http::pairs(page.split_once('?').unwrap().1).into_iter().collect()
}

#[test]
fn a_whole_sign_in_against_the_providers_own_addresses() {
    let server = FakeHttp::answering(good("fake-refresh"));
    let (open, seen) = browser(true);
    let cancel = AtomicBool::new(false);
    let tokens = sign_in_custom(&own(&server), "", "alice@example.org", &open, &cancel).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str(), tokens.email.as_str()), ("fake-access", "fake-refresh", "alice@example.org"));
    // The page: the client, PKCE S256, the IP-literal redirect, what it is asked for.
    let args = page_args(&seen);
    assert!(seen.lock().unwrap().starts_with(&server.url("/auth?")));
    assert_eq!((args["client_id"].as_str(), args["code_challenge_method"].as_str()), ("mailcraft-desktop", "S256"));
    assert_eq!(args["scope"], "openid email offline_access mail");
    assert!(args["redirect_uri"].starts_with("http://127.0.0.1:") && args["redirect_uri"].ends_with('/'));
    assert_eq!(args["login_hint"], "alice@example.org");
    // The trade: the code, the verifier whose hash the page got, the same redirect, no secret.
    let trade = form(&server, 0);
    assert!(server.request(0).starts_with("POST /token "));
    assert_eq!((trade["grant_type"].as_str(), trade["code"].as_str()), ("authorization_code", "synthetic-code"));
    assert_eq!(challenge(&trade["code_verifier"]), args["code_challenge"]);
    assert_eq!(trade["redirect_uri"], args["redirect_uri"]);
    assert!(!trade.contains_key("client_secret"));
}

#[test]
fn a_client_secret_goes_to_the_token_address_alone_and_never_into_an_error() {
    let server =
        FakeHttp::answering(json_answer("401 Unauthorized", &format!(r#"{{"error":"invalid_client","error_description":"bad secret {SECRET}"}}"#)));
    let (open, seen) = browser(true);
    let cancel = AtomicBool::new(false);
    let error = sign_in_custom(&own(&server), SECRET, "alice@example.org", &open, &cancel).unwrap_err();
    assert_eq!(authorization(&server, 0).as_deref(), Some(basic("mailcraft-desktop", SECRET).as_str()));
    assert!(!seen.lock().unwrap().contains(SECRET), "the secret went through the browser");
    // In fixed words: the provider's own text is never shown.
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains(SECRET) && !shown.contains("bad secret"), "{shown}");
    assert!(shown.contains("client id"), "{shown}");
}

#[test]
fn a_code_with_the_wrong_state_is_never_traded() {
    let server = FakeHttp::answering(good("r"));
    let (open, _) = browser(false);
    let cancel = AtomicBool::new(false);
    let outcome = custom::sign_in_custom_refused(&own(&server), "", "", &open, &cancel, Duration::from_millis(1_500));
    assert!(matches!(outcome.map_err(|r| r.error), Err(NetError::SignInTimedOut)));
    assert_eq!(server.asked(), 0);
}

#[test]
fn a_fixed_redirect_port_is_the_one_used() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let server = FakeHttp::answering(good("r"));
    let (open, seen) = browser(true);
    let cancel = AtomicBool::new(false);
    sign_in_custom(&OAuth { redirect_port: Some(port), ..own(&server) }, "", "", &open, &cancel).unwrap();
    assert_eq!(page_args(&seen)["redirect_uri"], format!("http://127.0.0.1:{port}/"));
    // One somebody else has is refused, not swapped for another the provider wouldn't send to.
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = taken.local_addr().unwrap().port();
    let error = sign_in_custom(&OAuth { redirect_port: Some(busy), ..own(&server) }, "", "", &open, &cancel).unwrap_err();
    assert!(matches!(error, NetError::Config(_)), "{error:?}");
}

#[test]
fn settings_that_cant_sign_in_are_refused_before_the_page_opens() {
    let server = FakeHttp::answering(good("r"));
    let opened = Arc::new(Mutex::new(false));
    let flag = opened.clone();
    let open = move |_: &str| *flag.lock().unwrap() = true;
    let cancel = AtomicBool::new(false);
    for broken in [
        OAuth { client_id: String::new(), ..own(&server) },
        OAuth { token: String::new(), ..own(&server) },
        OAuth { authorize: "http://id.example.org/auth".into(), ..own(&server) },
        OAuth { provider: OAuthProvider::Google, ..own(&server) },
    ] {
        assert!(matches!(sign_in_custom(&broken, "", "", &open, &cancel), Err(NetError::Config(_))), "{broken:?}");
    }
    assert!(matches!(sign_in_custom(&own(&server), "a secret with spaces", "", &open, &cancel), Err(NetError::Config(_))));
    assert!(!*opened.lock().unwrap());
    assert_eq!(server.asked(), 0);
    // Nor is it ever a Microsoft Graph sign-in.
    assert!(matches!(sign_in_for(&own(&server), "", &open, &cancel, Resource::Graph), Err(NetError::Config(_))));
    assert!(matches!(refresh_for(&own(&server), "r", Resource::Graph), Err(NetError::Config(_))));
}

#[test]
fn refreshing_keeps_the_newest_refresh_token() {
    let server = FakeHttp::start(|n, _| if n == 0 { good("rotated") } else { good("") });
    let first = refresh_custom(&own(&server), SECRET, "original").unwrap();
    assert_eq!(first.refresh, "rotated");
    let trade = form(&server, 0);
    assert_eq!((trade["grant_type"].as_str(), trade["refresh_token"].as_str()), ("refresh_token", "original"));
    assert_eq!(authorization(&server, 0), Some(basic("mailcraft-desktop", SECRET)));
    assert!(!trade.contains_key("scope"));
    // An answer without a new one keeps the one given.
    assert_eq!(refresh_custom(&own(&server), "", "rotated").unwrap().refresh, "rotated");
    // The generic path takes it too, as a public client.
    assert!(refresh(&own(&server), "rotated").is_ok());
}

#[test]
fn a_refused_refresh_says_sign_in_again_and_a_provider_that_cant_say_doesnt() {
    let refused = FakeHttp::answering(json_answer("400 Bad Request", r#"{"error":"invalid_grant","error_description":"<script>x</script>"}"#));
    let error = refresh_custom(&own(&refused), "", "old").unwrap_err();
    assert!(error.is_refused_sign_in(), "{error:?}");
    assert!(!format!("{error}").contains("script"));
    let down = FakeHttp::answering(json_answer("503 Service Unavailable", "<html>down</html>"));
    let error = refresh_custom(&own(&down), "", "old").unwrap_err();
    assert!(!error.is_refused_sign_in(), "{error:?}");
    let busy = FakeHttp::answering(json_answer("400 Bad Request", r#"{"error":"temporarily_unavailable"}"#));
    assert!(!refresh_custom(&own(&busy), "", "old").unwrap_err().is_refused_sign_in());
    // A provider that can't be reached isn't a refusal either.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let gone = OAuth { token: format!("http://127.0.0.1:{port}/token"), ..own(&down) };
    assert!(matches!(refresh_custom(&gone, "", "old"), Err(NetError::Connect { .. })));
    // No refresh token: nothing to renew, and nothing asked.
    let quiet = FakeHttp::answering(good("r"));
    assert!(refresh_custom(&own(&quiet), "", "").unwrap_err().is_refused_sign_in());
    assert_eq!(quiet.asked(), 0);
}

#[test]
fn an_answer_without_a_usable_token_is_refused() {
    for body in [r#"{"token_type":"mac","access_token":"a"}"#, r#"{"token_type":"Bearer"}"#, "not json", r#"{"access_token":"a b\r\nc"}"#] {
        let server = FakeHttp::answering(json_answer("200 OK", body));
        assert!(refresh_custom(&own(&server), "", "old").is_err(), "{body}");
    }
}

#[test]
fn revoking_gives_the_refresh_token_back_where_the_provider_says() {
    let server = FakeHttp::answering(json_answer("200 OK", ""));
    revoke_custom(&own(&server), SECRET, "the-refresh").unwrap();
    let sent = form(&server, 0);
    assert!(server.request(0).starts_with("POST /revoke "));
    assert_eq!((sent["token"].as_str(), sent["token_type_hint"].as_str()), ("the-refresh", "refresh_token"));
    // RFC 7009 §2.1: the client proves who it is as at the token address.
    assert_eq!(authorization(&server, 0), Some(basic("mailcraft-desktop", SECRET)));
    assert!(!sent.contains_key("client_secret") && !sent.contains_key("client_id"));
    // Nowhere to give it back: nothing is sent.
    let quiet = FakeHttp::answering(json_answer("200 OK", ""));
    revoke_custom(&OAuth { revoke: String::new(), ..own(&quiet) }, "", "the-refresh").unwrap();
    assert_eq!(quiet.asked(), 0);
    // A refusal is an error that names no token.
    let refused = FakeHttp::answering(json_answer("400 Bad Request", r#"{"error":"invalid_client","error_description":"the-refresh"}"#));
    let error = revoke_custom(&own(&refused), "", "the-refresh").unwrap_err();
    assert!(!format!("{error} {error:?}").contains("the-refresh"));
}

#[test]
fn the_endpoints_of_an_own_provider_are_its_settings() {
    let server = FakeHttp::answering(good("r"));
    let ends = Endpoints::of(&own(&server));
    assert_eq!((ends.authorize, ends.token), (server.url("/auth"), server.url("/token")));
    assert_eq!(ends.scope, "openid email offline_access mail");
    assert!(ends.consent.is_empty() && ends.fewer.is_empty() && ends.lesser.is_empty());
}

/// The `Authorization` header of request `n`, when it has one.
fn authorization(server: &FakeHttp, n: usize) -> Option<String> {
    let request = server.request(n);
    let head = request.split_once("\r\n\r\n").map_or(request.as_str(), |(head, _)| head).to_string();
    head.lines()
        .find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("authorization")).map(|(_, v)| v.trim().to_string()))
}

/// RFC 6749 §2.3.1 by hand: each form-encoded, joined by a colon, in base64.
fn basic(id: &str, secret: &str) -> String {
    format!("Basic {}", mailcraft_charset::base64::STANDARD.encode(format!("{}:{}", http::encode(id), http::encode(secret)).as_bytes()))
}

#[test]
fn a_secret_goes_in_the_header_and_the_form_names_no_client() {
    let server = FakeHttp::answering(good("r"));
    let (open, _) = browser(true);
    sign_in_custom(&own(&server), SECRET, "", &open, &AtomicBool::new(false)).unwrap();
    let trade = form(&server, 0);
    // §4.1.3: client_id only from a client that doesn't authenticate; never both ways.
    assert!(!trade.contains_key("client_id") && !trade.contains_key("client_secret"), "{trade:?}");
    assert_eq!(authorization(&server, 0).unwrap(), "Basic bWFpbGNyYWZ0LWRlc2t0b3A6bm90LWEtcmVhbC1jbGllbnQtc2VjcmV0");
}

#[test]
fn the_header_form_encodes_the_id_and_secret_before_base64() {
    let server = FakeHttp::answering(good("r"));
    let oauth = OAuth { client_id: "desk:top".into(), ..own(&server) };
    refresh_custom(&oauth, "a:b+c%d/e", "old").unwrap();
    let sent = authorization(&server, 0).unwrap();
    let decoded = mailcraft_charset::base64::STANDARD.decode(sent.strip_prefix("Basic ").unwrap()).unwrap();
    // The colon in each is escaped, so the one between them is the only one there is.
    assert_eq!(String::from_utf8(decoded).unwrap(), "desk%3Atop:a%3Ab%2Bc%25d%2Fe");
}

#[test]
fn a_provider_that_takes_only_the_form_gets_the_secret_there() {
    let server = FakeHttp::start(|_, _| good("r"));
    let oauth = OAuth { client_auth: ClientAuth::Post, ..own(&server) };
    refresh_custom(&oauth, SECRET, "old").unwrap();
    let trade = form(&server, 0);
    assert_eq!((trade["client_id"].as_str(), trade["client_secret"].as_str()), ("mailcraft-desktop", SECRET));
    assert_eq!(authorization(&server, 0), None);
    revoke_custom(&oauth, SECRET, "old").unwrap();
    assert_eq!(form(&server, 1)["client_secret"], SECRET);
    assert_eq!(authorization(&server, 1), None);
}

#[test]
fn a_public_client_names_itself_in_the_form_and_sends_no_header() {
    let server = FakeHttp::start(|_, _| good("r"));
    for oauth in [own(&server), OAuth { client_auth: ClientAuth::Post, ..own(&server) }] {
        let n = server.asked();
        refresh_custom(&oauth, "", "old").unwrap();
        let trade = form(&server, n);
        assert_eq!(trade.get("client_id").map(String::as_str), Some("mailcraft-desktop"));
        assert!(!trade.contains_key("client_secret") && authorization(&server, n).is_none());
    }
}

#[test]
fn the_header_never_comes_back_in_an_error() {
    let header = basic("mailcraft-desktop", SECRET);
    let echo = format!(r#"{{"error":"invalid_client","error_description":"got {}"}}"#, header);
    let server = FakeHttp::answering(json_answer("401 Unauthorized", &echo));
    let error = refresh_custom(&own(&server), SECRET, "old").unwrap_err();
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains(header.trim_start_matches("Basic ")) && !shown.contains(SECRET), "{shown}");
    assert!(error.is_refused_sign_in());
}

#[test]
fn an_answer_naming_the_issuer_is_taken_and_one_naming_another_never_traded() {
    // RFC 9207 §2.4.
    let server = FakeHttp::answering(good("r"));
    let issuer = own(&server).issuer;
    let (open, _) = browser_adding(true, &format!("&iss={}", http::encode(&issuer)));
    assert!(sign_in_custom(&own(&server), "", "", &open, &AtomicBool::new(false)).is_ok());
    let other = FakeHttp::answering(good("r"));
    let (open, _) = browser_adding(true, "&iss=https%3A%2F%2Fevil.example%2Frealms%2Fmail");
    let error = sign_in_custom(&own(&other), "", "", &open, &AtomicBool::new(false)).unwrap_err();
    assert!(format!("{error}").contains("another identity provider"), "{error}");
    assert_eq!(other.asked(), 0, "a code from another provider was traded");
}

#[test]
fn an_answer_without_the_name_a_provider_promised_is_refused() {
    let server = FakeHttp::answering(good("r"));
    let promised = OAuth { issuer_in_response: true, ..own(&server) };
    let (open, _) = browser(true);
    let error = sign_in_custom(&promised, "", "", &open, &AtomicBool::new(false)).unwrap_err();
    assert!(format!("{error}").contains("without your identity provider's name"), "{error}");
    assert_eq!(server.asked(), 0);
    // A provider that never said it names itself: its answer without a name is taken.
    let (open, _) = browser(true);
    assert!(sign_in_custom(&own(&server), "", "", &open, &AtomicBool::new(false)).is_ok());
}

#[test]
fn a_five_minute_token_is_used_as_it_comes_not_renewed_again_at_once() {
    // Keycloak's own setting: access tokens live five minutes. Renewed tokens that counted as
    // stale at once made every Send/Receive renew again and again (#1831).
    let body =
        |life: &str| json_answer("200 OK", &format!(r#"{{"token_type":"Bearer","expires_in":{life},"access_token":"a","refresh_token":"r"}}"#));
    let server = FakeHttp::start(move |n, _| body(if n == 0 { "300" } else { "0" }));
    let now = mailcraft_time::now();
    let tokens = refresh_custom(&own(&server), "", "old").unwrap();
    assert!(tokens.issued_at >= now && tokens.expires_at >= tokens.issued_at + 300);
    assert!(tokens.fresh_for(mailcraft_time::now(), 600), "a token just renewed counts as stale");
    // One said to live no time at all lives a minute: used once, not renewed in a loop.
    let brief = refresh_custom(&own(&server), "", "old").unwrap();
    assert!(brief.expires_at - brief.issued_at >= 60 && brief.fresh_for(mailcraft_time::now(), 600));
}

/// A no from the provider's page is said in fixed words by its code: its own words, which may
/// name the owner or the organisation, reach neither the error nor anything traded (#1844 review).
#[test]
fn a_no_from_the_page_is_said_in_fixed_words_never_the_providers() {
    let server = FakeHttp::answering(good("r"));
    let (open, _) = browser_adding(true, "&error=access_denied&error_description=Private%20text%20about%20ada%40example.org");
    let error = sign_in_custom(&own(&server), "", "", &open, &AtomicBool::new(false)).unwrap_err();
    let said = format!("{error} {error:?}");
    assert!(said.contains("access was refused on the sign-in page"), "{said}");
    assert!(!said.contains("Private") && !said.contains("ada@example.org") && !said.contains("ada%40"), "{said}");
    assert_eq!(server.asked(), 0, "nothing was traded");
    // A code nobody knows: the general words, not the code itself.
    let (open, _) = browser_adding(true, "&error=Private-code-ada&error_description=more");
    let error = sign_in_custom(&own(&server), "", "", &open, &AtomicBool::new(false)).unwrap_err();
    assert!(!format!("{error} {error:?}").contains("Private"), "{error}");
}

/// The owner's own no on the provider's page is still the owner's: Declined, in fixed words
/// (#1844 review).
#[test]
fn the_owners_no_on_the_page_is_declined() {
    for code in ["access_denied", "consent_required"] {
        let server = FakeHttp::answering(good("r"));
        let (open, _) = browser_adding(true, &format!("&error={code}&error_description=Private"));
        let end = sign_in_custom_explained(&own(&server), "", "", &open, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(end, SignInEnd::Declined), "{code}: {end:?}");
    }
    let server = FakeHttp::answering(good("r"));
    let (open, _) = browser_adding(true, "&error=server_error");
    assert!(!matches!(sign_in_custom_explained(&own(&server), "", "", &open, &AtomicBool::new(false)), Err(SignInEnd::Declined)));
}

/// Outside a test nothing is sent to an own identity provider without encryption, not even to
/// this computer; https addresses are taken either way (#1844 review).
#[test]
fn an_own_providers_plain_http_address_is_spoken_to_only_in_a_test() {
    for address in ["http://127.0.0.1:8080/token", "http://localhost/token", "http://[::1]/token"] {
        assert!(matches!(custom::provider_url_when(address, false), Err(NetError::Config(_))), "{address}");
        assert!(custom::provider_url_when(address, true).is_ok(), "{address}");
    }
    assert!(custom::provider_url_when("https://id.example.org/token", false).is_ok());
    // Plain http elsewhere is never spoken to, test or not.
    assert!(custom::provider_url_when("http://id.example.org/token", true).is_err());
}
