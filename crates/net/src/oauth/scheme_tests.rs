// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Signing in through a custom scheme (what a phone does) end to end: a fake token endpoint, and
//! a hand-back that plays the system bringing the program the address its page came back to.
//! The same through loopback is in `tests.rs`; here the address is hostile. Nothing leaves this
//! machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use proptest::prelude::*;

use super::*;
use crate::fake::{FakeHttp, json_answer};

const SCHEME: &str = "org.example.mail";
const PATH: &str = "/oauth2redirect";
const TOKENS: &str = r#"{"token_type":"Bearer","expires_in":3599,"access_token":"acc-ess.tok_en","refresh_token":"re-fresh.tok_en"}"#;

fn oauth() -> OAuth {
    OAuth { provider: OAuthProvider::Microsoft, client_id: "client-1".into(), tenant: String::new(), ..Default::default() }
}

fn fields(query: &str) -> HashMap<String, String> {
    http::pairs(query).into_iter().collect()
}

fn endpoints_of(server: &FakeHttp) -> Endpoints {
    Endpoints {
        authorize: server.url("/authorize"),
        token: server.url("/token"),
        scope: "mail.read mail.send offline_access".into(),
        consent: String::new(),
        lesser: String::new(),
    }
}

fn token_server() -> FakeHttp {
    FakeHttp::answering(json_answer("200 OK", TOKENS))
}

/// What the page was sent to, as the "browser" saw it.
type Seen = Arc<Mutex<String>>;

fn asked_of(seen: &Seen) -> HashMap<String, String> {
    let url = seen.lock().unwrap().clone();
    fields(url.split_once('?').map_or("", |(_, query)| query))
}

/// A hand-back that gives `template` with `{state}` filled in with the state of the page it was
/// opened for.
fn handback_of(seen: &Seen, template: &str) -> impl Fn(&AtomicBool, Duration) -> Result<String, NetError> + use<> {
    let (seen, template) = (seen.clone(), template.to_string());
    move |_, _| {
        let state = asked_of(&seen).get("state").cloned().unwrap_or_default();
        Ok(template.replace("{state}", &state))
    }
}

/// An `open` that notes the address of the page.
fn noting(seen: &Seen) -> impl Fn(&str) + use<> {
    let seen = seen.clone();
    move |url: &str| *seen.lock().unwrap() = url.to_string()
}

/// A sign-in that comes back to `template`, through the registered scheme. Gives what the
/// sign-in said and the address of the page it opened.
fn sign_in_back(server: &FakeHttp, template: &str) -> (Result<Tokens, NetError>, Seen) {
    let seen = Seen::default();
    let handback = handback_of(&seen, template);
    let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, &handback).unwrap());
    let result = sign_in_via(&endpoints_of(server), &oauth(), "ada@example.org", &noting(&seen), &AtomicBool::new(false), &via);
    (result, seen)
}

fn refused(result: Result<Tokens, NetError>, server: &FakeHttp, what: &str) {
    assert!(result.is_err(), "{what}: {result:?}");
    assert_eq!(server.asked(), 0, "{what}: nothing is traded for an address that isn't right");
}

#[test]
fn a_sign_in_through_a_custom_scheme_brings_tokens() {
    let server = token_server();
    let (result, seen) = sign_in_back(&server, &format!("{SCHEME}:{PATH}?code=the-code&state={{state}}"));
    let tokens = result.unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("acc-ess.tok_en", "re-fresh.tok_en"));
    // The page was told where to come back to, with PKCE and a state, and nothing of loopback.
    let page = asked_of(&seen);
    let uri = format!("{SCHEME}:{PATH}");
    assert_eq!(page["redirect_uri"], uri);
    assert_eq!((page["code_challenge_method"].as_str(), page["response_type"].as_str()), ("S256", "code"));
    assert!(!seen.lock().unwrap().contains("localhost") && page["state"].len() >= 24);
    // The code went to the token endpoint with the verifier whose hash the page was shown, and
    // with the same address.
    assert_eq!(server.asked(), 1);
    let form = fields(server.request(0).split_once("\r\n\r\n").unwrap().1);
    assert_eq!(form["code"], "the-code");
    assert_eq!(form["redirect_uri"], uri);
    assert_eq!(challenge(&form["code_verifier"]), page["code_challenge"]);
    assert_eq!(form["grant_type"], "authorization_code");
}

#[test]
fn what_the_provider_adds_is_taken_and_the_scheme_may_come_in_any_case() {
    for address in [
        format!("{SCHEME}:{PATH}?code=c&state={{state}}&session_state=abc&iss=https%3A%2F%2Flogin.example.org%2F&client_info=xyz"),
        format!("{}:{PATH}?state={{state}}&code=c", SCHEME.to_uppercase()),
        // An empty piece between two others says nothing.
        format!("{SCHEME}:{PATH}?code=c&&state={{state}}"),
    ] {
        let server = token_server();
        let (result, _) = sign_in_back(&server, &address);
        assert!(result.is_ok(), "{address}: {result:?}");
    }
}

#[test]
fn an_address_that_is_not_the_registered_one_is_refused_and_nothing_is_traded() {
    let hostile = [
        // The state.
        format!("{SCHEME}:{PATH}?code=c&state=not-the-state"),
        format!("{SCHEME}:{PATH}?code=c"),
        format!("{SCHEME}:{PATH}?code=c&state="),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}x"),
        format!("{SCHEME}:{PATH}?code=c&state=x{{state}}"),
        format!("{SCHEME}:{PATH}?code=c&STATE={{state}}"),
        // The scheme.
        format!("http:{PATH}?code=c&state={{state}}"),
        format!("https://localhost:1234{PATH}?code=c&state={{state}}"),
        format!("org.example.mail2:{PATH}?code=c&state={{state}}"),
        format!("org.example:{PATH}?code=c&state={{state}}"),
        format!("xorg.example.mail:{PATH}?code=c&state={{state}}"),
        format!("javascript:{PATH}?code=c&state={{state}}"),
        "?code=c&state={state}".to_string(),
        format!(":{PATH}?code=c&state={{state}}"),
        "nonsense".to_string(),
        String::new(),
        // The path.
        format!("{SCHEME}:/other?code=c&state={{state}}"),
        format!("{SCHEME}:{PATH}x?code=c&state={{state}}"),
        format!("{SCHEME}:{PATH}/?code=c&state={{state}}"),
        format!("{SCHEME}:{PATH}/more?code=c&state={{state}}"),
        format!("{SCHEME}:/{PATH}?code=c&state={{state}}"),
        format!("{SCHEME}://{}?code=c&state={{state}}", &PATH[1..]),
        format!("{SCHEME}:///{}?code=c&state={{state}}", &PATH[1..]),
        format!("{SCHEME}://host{PATH}?code=c&state={{state}}"),
        format!("{SCHEME}:{}?code=c&state={{state}}", PATH.to_uppercase()),
        format!("{SCHEME}:?code=c&state={{state}}"),
        format!("{SCHEME}:code=c&state={{state}}"),
        format!("{SCHEME}:{PATH}&code=c&state={{state}}"),
        format!("{SCHEME}:{PATH};x?code=c&state={{state}}"),
        // Extra names, and names said twice.
        format!("{SCHEME}:{PATH}?code=c&state={{state}}&evil=1"),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}&redirect_uri=x"),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}&admin_consent=True"),
        format!("{SCHEME}:{PATH}?code=c&code=d&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}&%63ode=d"),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}&=x"),
        // A fragment, white space, control characters, other letters.
        format!("{SCHEME}:{PATH}?code=c&state={{state}}#more"),
        format!("{SCHEME}:{PATH}#code=c&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=c&state={{state}} "),
        format!(" {SCHEME}:{PATH}?code=c&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}\r\nHost: evil"),
        format!("{SCHEME}:{PATH}?code=c\u{0}&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=\u{e9}&state={{state}}"),
        format!("{SCHEME}\u{200b}:{PATH}?code=c&state={{state}}"),
        // No code, or one that can't be sent on.
        format!("{SCHEME}:{PATH}?state={{state}}"),
        format!("{SCHEME}:{PATH}?code=&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=%00&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=a%0Ab&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=%C3%A9&state={{state}}"),
    ];
    for address in &hostile {
        let server = token_server();
        let (result, _) = sign_in_back(&server, address);
        refused(result, &server, address);
    }
}

#[test]
fn an_address_of_any_size_is_refused_without_harm() {
    let big = "a".repeat(1 << 20);
    let hostile = [
        // Too long as a whole.
        format!("{SCHEME}:{PATH}?code={big}&state={{state}}"),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}&{big}=1"),
        format!("{big}:{PATH}?code=c&state={{state}}"),
        format!("{SCHEME}:{big}"),
        // One value too long, the whole not.
        format!("{SCHEME}:{PATH}?code={}&state={{state}}", "a".repeat(5000)),
        // Too many names.
        format!("{SCHEME}:{PATH}?code=c&state={{state}}{}", "&x=1".repeat(100)),
        format!("{SCHEME}:{PATH}?code=c&state={{state}}{}", "&".repeat(100_000)),
        // The same name more often than anything else.
        format!("{SCHEME}:{PATH}?{}state={{state}}", "code=c&".repeat(100)),
        // Percent signs without end.
        format!("{SCHEME}:{PATH}?code={}&state={{state}}", "%".repeat(3000)),
        format!("{SCHEME}:{PATH}?code={}&state={{state}}", "%ZZ".repeat(1000)),
    ];
    for address in &hostile {
        let server = token_server();
        let (result, _) = sign_in_back(&server, address);
        refused(result, &server, &address.chars().take(60).collect::<String>());
    }
}

#[test]
fn an_error_says_nothing_of_the_address_the_code_or_the_state() {
    let server = token_server();
    let secret = "SECRET-code-12345";
    let (result, seen) = sign_in_back(&server, &format!("{SCHEME}:{PATH}?code={secret}&state=wrong&extra={secret}"));
    let error = result.unwrap_err();
    let text = format!("{error:?} {error}");
    let state = asked_of(&seen)["state"].clone();
    assert!(!text.contains(secret) && !text.contains("wrong") && !text.contains(&state) && !text.contains(SCHEME), "{text}");
    let (result, _) = sign_in_back(&server, &format!("{SCHEME}:{PATH}?code={secret}&state={{state}}&extra={secret}"));
    let error = result.unwrap_err();
    let text = format!("{error:?} {error}");
    assert!(!text.contains(secret), "{text}");
}

#[test]
fn a_token_endpoint_that_says_no_does_not_give_back_the_code_or_the_verifier() {
    let no = json_answer("400 Bad Request", r#"{"error":"invalid_grant","error_description":"The code the-code-9 and verifier were wrong."}"#);
    let server = FakeHttp::answering(no);
    let (result, _) = sign_in_back(&server, &format!("{SCHEME}:{PATH}?code=the-code-9&state={{state}}"));
    let error = result.unwrap_err();
    assert!(error.is_refused_sign_in(), "{error:?}");
    let form = fields(server.request(0).split_once("\r\n\r\n").unwrap().1);
    let text = format!("{error:?} {error}");
    assert!(!text.contains(&form["code_verifier"]), "{text}");
}

#[test]
fn a_no_on_the_providers_page_is_the_owners_no() {
    let server = token_server();
    let (result, _) = sign_in_back(&server, &format!("{SCHEME}:{PATH}?error=access_denied&error_description=The+owner+said+no&state={{state}}"));
    assert!(result.unwrap_err().is_refused_sign_in());
    assert_eq!(server.asked(), 0);
    // A no that doesn't carry the state is not believed: nobody else can call the sign-in off.
    let (result, _) = sign_in_back(&server, &format!("{SCHEME}:{PATH}?error=access_denied&state=other"));
    assert!(!result.unwrap_err().is_refused_sign_in());
    let (result, _) = sign_in_back(&server, &format!("{SCHEME}:{PATH}?error=access_denied"));
    assert!(!result.unwrap_err().is_refused_sign_in());
}

#[test]
fn an_administrators_no_is_told_apart_on_the_way_back_too() {
    let server = token_server();
    let seen = Seen::default();
    let handback = handback_of(
        &seen,
        &format!("{SCHEME}:{PATH}?error=access_denied&error_description=AADSTS90094%3A+Admin+consent+is+required.&state={{state}}"),
    );
    let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, &handback).unwrap());
    let end = sign_in_explained_via(&endpoints_of(&server), &oauth(), "", &noting(&seen), &AtomicBool::new(false), &via);
    assert_eq!(end.unwrap_err(), SignInEnd::Approval);
}

#[test]
fn the_host_ending_the_wait_ends_the_sign_in_as_it_says() {
    let server = token_server();
    let sign_in = |handback: &Handback, cancel: &AtomicBool| {
        let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, handback).unwrap());
        sign_in_via(&endpoints_of(&server), &oauth(), "", &|_| {}, cancel, &via)
    };
    let timed_out = |_: &AtomicBool, _: Duration| Err(NetError::SignInTimedOut);
    assert!(matches!(sign_in(&timed_out, &AtomicBool::new(false)), Err(NetError::SignInTimedOut)));
    let called_off = |_: &AtomicBool, _: Duration| Err(NetError::SignInCancelled);
    assert!(matches!(sign_in(&called_off, &AtomicBool::new(false)), Err(NetError::SignInCancelled)));
    // Called off before the page is opened: the page is not opened and the host is not asked.
    let events = Arc::new(Mutex::new(0));
    let (opened, asked) = (events.clone(), events.clone());
    let handback = move |_: &AtomicBool, _: Duration| -> Result<String, NetError> {
        *asked.lock().unwrap() += 100;
        Err(NetError::SignInTimedOut)
    };
    let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, &handback).unwrap());
    let off = sign_in_via(&endpoints_of(&server), &oauth(), "", &move |_| *opened.lock().unwrap() += 1, &AtomicBool::new(true), &via);
    assert!(matches!(off, Err(NetError::SignInCancelled)), "{off:?}");
    assert_eq!(*events.lock().unwrap(), 0);
    // Called off while the address was on its way: it is not used.
    let cancel = AtomicBool::new(false);
    let late = |flag: &AtomicBool, _: Duration| -> Result<String, NetError> {
        flag.store(true, Ordering::Relaxed);
        Ok(format!("{SCHEME}:{PATH}?code=c&state=whatever"))
    };
    assert!(matches!(sign_in(&late, &cancel), Err(NetError::SignInCancelled)));
    assert_eq!(server.asked(), 0);
}

#[test]
fn the_host_is_told_how_long_to_wait() {
    let server = token_server();
    let told = Arc::new(Mutex::new(None));
    let shared = told.clone();
    let handback = move |_: &AtomicBool, patience: Duration| -> Result<String, NetError> {
        *shared.lock().unwrap() = Some(patience);
        Err(NetError::SignInTimedOut)
    };
    let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, &handback).unwrap());
    let _ = sign_in_via(&endpoints_of(&server), &oauth(), "", &|_| {}, &AtomicBool::new(false), &via);
    assert_eq!(*told.lock().unwrap(), Some(SIGN_IN_TIMEOUT));
}

#[test]
fn only_a_reverse_domain_scheme_and_a_plain_path_are_registered() {
    let handback = |_: &AtomicBool, _: Duration| -> Result<String, NetError> { Err(NetError::SignInTimedOut) };
    for scheme in ["org.example.mail", "a.b", "org.example-mail.app", "com.example.mail+2", "org.example.app1"] {
        assert!(CustomScheme::new(scheme, PATH, &handback).is_ok(), "{scheme}");
    }
    for scheme in [
        "",
        "a",
        "ab",
        "http",
        "https",
        "file",
        "mailto",
        "javascript",
        "tel",
        "cid",
        "data",
        "localhost",
        "mail",
        "Org.Example.Mail",
        "org..example",
        ".org.example",
        "org.example.",
        "org.example-",
        "-org.example",
        "1org.example",
        "org.-example",
        "org-.example",
        "org.+example",
        "org example",
        "org.example:",
        "org.example/",
        "org.example\n",
        "org.\u{e9}xample",
        "org_example.mail",
        "org.example.mail://",
    ] {
        assert!(CustomScheme::new(scheme, PATH, &handback).is_err(), "{scheme:?}");
    }
    assert!(CustomScheme::new(&format!("a.{}", "b".repeat(200)), PATH, &handback).is_err());
    for path in [DEFAULT_REDIRECT_PATH, "/auth", "/a/b", "/a-b_c.d~e"] {
        assert!(CustomScheme::new(SCHEME, path, &handback).is_ok(), "{path}");
    }
    let long = format!("/{}", "a".repeat(200));
    for path in ["", "/", "auth", "//auth", "/a//b", "/a/", "/../a", "/a?b", "/a#b", "/a b", "/a\n", "/\u{e9}", "/a%2Fb", "http://x", long.as_str()] {
        assert!(CustomScheme::new(SCHEME, path, &handback).is_err(), "{path:?}");
    }
    // What it is shown as, and in the logs.
    let scheme = CustomScheme::new(SCHEME, "/back", &handback).unwrap();
    assert_eq!(scheme.redirect_uri(), "org.example.mail:/back");
    assert!(!format!("{scheme:?}").contains("handback"));
}

#[test]
fn a_custom_scheme_is_not_for_google() {
    let server = token_server();
    let handback = |_: &AtomicBool, _: Duration| -> Result<String, NetError> { Err(NetError::SignInTimedOut) };
    let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, &handback).unwrap());
    let google = OAuth { provider: OAuthProvider::Google, ..oauth() };
    let opened = Arc::new(Mutex::new(false));
    let shared = opened.clone();
    let result = sign_in_via(&endpoints_of(&server), &google, "", &move |_| *shared.lock().unwrap() = true, &AtomicBool::new(false), &via);
    assert!(matches!(result, Err(NetError::Config(_))), "{result:?}");
    assert!(!*opened.lock().unwrap());
}

#[test]
fn loopback_through_the_strategy_is_what_it_was() {
    let server = token_server();
    let open = |url: &str| {
        let asked = fields(url.split_once('?').unwrap().1);
        let port: u16 = asked["redirect_uri"].strip_prefix("http://localhost:").unwrap().parse().unwrap();
        let request = format!("GET /?code=loop-code&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n", asked["state"]);
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut stream = std::net::TcpStream::connect(("localhost", port)).unwrap();
            let _ = stream.write_all(request.as_bytes());
            let mut page = Vec::new();
            let _ = stream.read_to_end(&mut page);
        });
    };
    let tokens = sign_in_via(&endpoints_of(&server), &oauth(), "", &open, &AtomicBool::new(false), &RedirectStrategy::Loopback).unwrap();
    assert_eq!(tokens.access, "acc-ess.tok_en");
    let form = fields(server.request(0).split_once("\r\n\r\n").unwrap().1);
    assert!(form["redirect_uri"].starts_with("http://localhost:") && form["code"] == "loop-code");
}

/// An administrator's approval that comes back to `template`.
fn approval_back(template: &str) -> Result<(), AdminRefusal> {
    let seen = Seen::default();
    let handback = handback_of(&seen, template);
    let via = RedirectStrategy::CustomScheme(CustomScheme::new(SCHEME, PATH, &handback).unwrap());
    let settings = OAuth { client_id: "0f6c8d2a-1b3e-4c5d-9e7f-a1b2c3d4e5f6".into(), ..oauth() };
    let result = admin_consent_via(&settings, "", &noting(&seen), &AtomicBool::new(false), &via);
    let page = asked_of(&seen);
    assert_eq!(page["redirect_uri"], format!("{SCHEME}:{PATH}"));
    result
}

#[test]
fn an_administrators_approval_comes_back_through_a_custom_scheme_too() {
    assert_eq!(approval_back(&format!("{SCHEME}:{PATH}?admin_consent=True&tenant=3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b&state={{state}}")), Ok(()));
    for hostile in [
        format!("{SCHEME}:{PATH}?admin_consent=True&state=other"),
        format!("{SCHEME}:{PATH}?admin_consent=True"),
        format!("{SCHEME}:{PATH}?admin_consent=True&state={{state}}&evil=1"),
        format!("{SCHEME}:{PATH}?admin_consent=True&admin_consent=True&state={{state}}"),
        format!("{SCHEME}:{PATH}?admin_consent=True&code=c&state={{state}}"),
        format!("http:{PATH}?admin_consent=True&state={{state}}"),
        format!("{SCHEME}:/other?admin_consent=True&state={{state}}"),
        format!("{SCHEME}:{PATH}?admin_consent=True&state={{state}}#x"),
        format!("{SCHEME}:{PATH}?admin_consent=True&state={{state}}{}", "&".repeat(50_000)),
    ] {
        assert!(matches!(approval_back(&hostile), Err(AdminRefusal::Other(_))), "{hostile}");
    }
    // The yes is the word `True` and nothing else.
    for answer in ["admin_consent=False", "tenant=x", "admin_consent="] {
        let result = approval_back(&format!("{SCHEME}:{PATH}?{answer}&state={{state}}"));
        assert!(matches!(result, Err(AdminRefusal::Other(_))), "{answer}: {result:?}");
    }
    let no =
        approval_back(&format!("{SCHEME}:{PATH}?error=access_denied&error_description=AADSTS90094%3A+Admin+consent+is+required.&state={{state}}"));
    assert!(matches!(no, Err(AdminRefusal::NotAdministrator(_))), "{no:?}");
}

#[test]
fn refresh_is_the_same_whatever_the_way_back() {
    // Nothing of a redirect is in a refresh: no redirect_uri, no code.
    let server = token_server();
    let tokens = refresh_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert_eq!(tokens.access, "acc-ess.tok_en");
    let form = fields(server.request(0).split_once("\r\n\r\n").unwrap().1);
    assert!(!form.contains_key("redirect_uri") && !form.contains_key("code") && form["grant_type"] == "refresh_token");
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever the system hands over, the sign-in ends in tokens or an error, never a panic,
    /// and tokens only for an address that says the state.
    #[test]
    fn any_address_ends_without_a_panic(address in "\\PC{0,300}", with_state in any::<bool>()) {
        let server = token_server();
        let template = if with_state { format!("{SCHEME}:{PATH}?code=c&state={{state}}") } else { address };
        let (result, _) = sign_in_back(&server, &template);
        if result.is_ok() {
            prop_assert!(with_state);
        }
    }

    /// Anything printable after the path, without the real state in it, is an error and nothing is traded.
    #[test]
    fn arbitrary_text_after_the_path_is_never_a_sign_in(tail in "[ -~]{0,200}") {
        let server = token_server();
        let (result, _) = sign_in_back(&server, &format!("{SCHEME}:{PATH}{tail}"));
        prop_assert!(result.is_err() && server.asked() == 0);
    }
}
