// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The sign-in end to end, on loopback: a fake token endpoint, and an `open` that plays the
//! owner's browser. Nothing here leaves this machine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use proptest::prelude::*;

use super::*;
use crate::fake::{FakeHttp, json_answer};

/// How long the tests wait for their own "browser": long enough for a slow machine, short
/// enough that a broken test ends.
const PATIENCE: Duration = Duration::from_secs(30);

const TOKENS: &str = r#"{"token_type":"Bearer","scope":"mail.read mail.send","expires_in":3599,"ext_expires_in":3599,"access_token":"acc-ess.tok_en","refresh_token":"re-fresh.tok_en"}"#;

fn oauth() -> OAuth {
    OAuth { provider: OAuthProvider::Microsoft, client_id: "client-1".into(), tenant: String::new(), ..Default::default() }
}

fn response(status: &str, body: &str) -> Vec<u8> {
    json_answer(status, body)
}

fn ok(body: &str) -> Vec<u8> {
    json_answer("200 OK", body)
}

/// The names and values of a query or a form.
fn fields(query: &str) -> HashMap<String, String> {
    http::pairs(query).into_iter().collect()
}

/// Endpoints on a fake server: its `/token` is spoken to; its `/authorize` is only ever shown to the "browser".
fn endpoints_of(server: &FakeHttp) -> Endpoints {
    Endpoints {
        authorize: server.url("/authorize"),
        token: server.url("/token"),
        scope: "mail.read mail.send offline_access".into(),
        consent: String::new(),
        lesser: String::new(),
    }
}

/// The same, with a sign-in page that is also asked for what Graph's tokens will be for, as the
/// provider's own is.
fn endpoints_with_consent(server: &FakeHttp) -> Endpoints {
    Endpoints { consent: "graph.send graph.calendars".into(), ..endpoints_of(server) }
}

/// The same, for Graph's tokens.
fn graph_endpoints_of(server: &FakeHttp) -> Endpoints {
    Endpoints { scope: "graph.send graph.calendars offline_access".into(), ..endpoints_of(server) }
}

const GRAPH_SCOPES: &str = "https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read";
/// What Graph was asked for before the mailbox's settings were: what an older sign-in agreed to.
const GRAPH_SCOPES_BEFORE: &str =
    "https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read";

#[test]
fn a_sign_in_that_lacks_the_newest_permission_keeps_everything_it_had() {
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"graph-access","refresh_token":"re-fresh-2"}"#;
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'.","error_codes":[65001],"suberror":"consent_required"}"#;
    let over = r#"{"error":"invalid_grant","error_description":"AADSTS70000: The user could not be authenticated as the grant is expired. The user must sign in again."}"#;
    let all = |server: &FakeHttp| Endpoints { scope: format!("{GRAPH_SCOPES} offline_access"), ..endpoints_of(server) };
    let plain = |server: &FakeHttp| Endpoints { scope: format!("{GRAPH_SCOPES_BEFORE} offline_access"), ..endpoints_of(server) };
    let ask = |server: &FakeHttp| refresh_graph_at(&all(server), &plain(server), &endpoints_of(server), &oauth(), "re-fresh");
    // What the provider's own endpoints ask for: tokens for what was always asked for, and the
    // settings of the mailbox as one thing more for the owner to agree to on the page.
    let wanted = Endpoints::for_resource(&oauth(), Resource::Graph);
    assert!(
        wanted.scope.starts_with(&format!("{GRAPH_SCOPES_BEFORE} offline_access")) && GRAPH_SCOPES.starts_with(GRAPH_SCOPES_BEFORE),
        "{wanted:?}"
    );
    assert_eq!(format!("{GRAPH_SCOPES_BEFORE} {}", wanted.consent), GRAPH_SCOPES);

    // The owner agreed to all of it: one question.
    let server = FakeHttp::answering(ok(granted));
    let got = ask(&server).unwrap();
    assert_eq!(
        (got.tokens.access.as_str(), got.tokens.refresh.as_str(), got.extras_missing, server.asked()),
        ("graph-access", "re-fresh-2", false, 1)
    );
    assert!(form_of(&server, 0)["scope"].contains("MailboxSettings.Read"));

    // The owner agreed before the mailbox's settings were asked for: asked for what was agreed
    // to, the tokens come, and all that is missing is the extra. Nothing that worked stops.
    let server = FakeHttp::start(move |n, _| if n == 0 { response("400 Bad Request", unasked) } else { ok(granted) });
    let got = ask(&server).unwrap();
    assert_eq!((got.tokens.access.as_str(), got.extras_missing, server.asked()), ("graph-access", true, 2));
    let (first, second) = (form_of(&server, 0), form_of(&server, 1));
    assert!(first["scope"].contains("MailboxSettings.Read") && !second["scope"].contains("MailboxSettings.Read"), "{second:?}");
    assert!(second["scope"].contains("Calendars.ReadWrite") && second["refresh_token"] == "re-fresh");
    // The same when the no comes in the words for a sign-in that is over, as it may for a
    // personal account.
    let server = FakeHttp::start(move |n, _| if n == 0 { response("400 Bad Request", over) } else { ok(granted) });
    assert_eq!(ask(&server).map(|got| got.extras_missing), Ok(true));

    // Nobody agreed to Graph at all: the owner is to be asked, whichever words the no comes in.
    // (Said plainly, it is believed; said as "the sign-in is over", mail is asked whether it is.)
    let server = FakeHttp::answering(response("400 Bad Request", unasked));
    let refused = ask(&server).unwrap_err();
    assert!(refused.is_consent_needed() && !refused.is_refused_sign_in(), "{refused:?}");
    assert_eq!(server.asked(), 2);
    let server = FakeHttp::start(move |n, _| if n < 2 { response("400 Bad Request", over) } else { ok(granted) });
    let refused = ask(&server).unwrap_err();
    assert!(refused.is_consent_needed() && !refused.is_refused_sign_in(), "{refused:?}");
    assert_eq!(server.asked(), 3);
    assert!(form_of(&server, 2)["scope"].starts_with("mail.read"), "the third question is for mail");
    assert!(!format!("{refused} {refused:?}").contains("re-fresh"));

    // The sign-in really is over: for mail too. That is what is said, once.
    let server = FakeHttp::answering(response("400 Bad Request", over));
    let refused = ask(&server).unwrap_err();
    assert!(refused.is_refused_sign_in() && !refused.is_consent_needed(), "{refused:?}");
    assert_eq!(server.asked(), 3);
    // The provider can't say just now: nothing is concluded, and nothing more is asked.
    let busy = r#"{"error":"temporarily_unavailable","error_description":"Try again."}"#;
    let server = FakeHttp::answering(response("503 Service Unavailable", busy));
    let refused = ask(&server).unwrap_err();
    assert!(!refused.is_refused_sign_in() && !refused.is_consent_needed(), "{refused:?}");
    assert_eq!(server.asked(), 1);
    let server = FakeHttp::start(move |n, _| if n == 0 { response("400 Bad Request", unasked) } else { response("503 Service Unavailable", busy) });
    let refused = ask(&server).unwrap_err();
    assert!(!refused.is_refused_sign_in() && !refused.is_consent_needed(), "{refused:?}");
}

#[test]
fn graph_is_asked_for_a_calendar_it_may_write_to() {
    // Events made, moved and deleted here go back to the server: reading alone is not enough,
    // and reading alone is not asked for beside it (the wider permission covers it).
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        let endpoints = Endpoints::for_resource(&oauth(), resource);
        let asked = format!("{} {}", endpoints.scope, endpoints.consent);
        let scopes: Vec<&str> = asked.split(' ').collect();
        assert!(scopes.contains(&"https://graph.microsoft.com/Calendars.ReadWrite"), "{asked}");
        assert!(!scopes.contains(&"https://graph.microsoft.com/Calendars.Read"), "{asked}");
        // The contacts are read only, as before.
        assert!(scopes.contains(&"https://graph.microsoft.com/Contacts.Read") && !asked.contains("Contacts.ReadWrite"), "{asked}");
    }
}

#[test]
fn a_sign_in_from_before_the_calendar_was_written_to_needs_the_owners_consent_again() {
    // The owner agreed to reading the calendar, once. The refresh token of that sign-in is asked
    // for a token that may write to it, which nobody agreed to: the provider says so, mail is
    // none the worse, and the owner is to be asked on the provider's page.
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'. Send an interactive authorization request for this user and resource.","error_codes":[65001],"suberror":"consent_required"}"#;
    let server = FakeHttp::answering(response("400 Bad Request", unasked));
    let wanted = Endpoints::for_resource(&oauth(), Resource::Graph).scope;
    let endpoints = Endpoints { scope: wanted.clone(), ..endpoints_of(&server) };
    let refused = refresh_for_at(&endpoints, &oauth(), "re-fresh", Resource::Graph).unwrap_err();
    assert!(refused.is_consent_needed() && !refused.is_refused_sign_in(), "{refused:?}");
    // What was asked for is the calendar to write to.
    let form = form_of(&server, 0);
    assert_eq!(form["scope"], wanted);
    assert!(form["scope"].contains("https://graph.microsoft.com/Calendars.ReadWrite "), "{}", form["scope"]);
    // The same refresh token still gets mail its tokens: there it is a sign-in like any other.
    let mail = FakeHttp::answering(ok(r#"{"token_type":"Bearer","expires_in":3600,"access_token":"mail-access"}"#));
    let tokens = refresh_for_at(&endpoints_of(&mail), &oauth(), "re-fresh", Resource::Mail).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("mail-access", "re-fresh"));
}

/// The form the server was sent in request `n`.
fn form_of(server: &FakeHttp, n: usize) -> HashMap<String, String> {
    fields(server.request(n).split_once("\r\n\r\n").unwrap().1)
}

/// What the "browser" was sent to and what it was shown.
#[derive(Default)]
struct Visit {
    url: String,
    pages: Vec<String>,
}

type Browser = Arc<Mutex<Visit>>;

/// Send one request to the port the sign-in waits on, by the name a browser would use, and read
/// the answer. `request` is the method and what is asked for, or, when it has lines, everything.
fn visit(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("localhost", port)).unwrap();
    stream.set_read_timeout(Some(PATIENCE)).unwrap();
    let whole = if request.contains('\n') {
        request.to_string()
    } else {
        format!("{request} HTTP/1.1\r\nHost: localhost:{port}\r\nAccept: text/html\r\n\r\n")
    };
    // The other side may hang up on a request it has seen enough of.
    let _ = stream.write_all(whole.as_bytes());
    let mut page = Vec::new();
    let _ = stream.read_to_end(&mut page);
    String::from_utf8_lossy(&page).into_owned()
}

/// The port in `http://localhost:{port}`.
fn port_of(redirect_uri: &str) -> u16 {
    redirect_uri.strip_prefix("http://localhost:").unwrap().parse().unwrap()
}

/// An `open` that plays the owner's browser. As a browser is a program of its own, it works on
/// a thread of its own: it takes the address of the sign-in page apart and makes each of
/// `requests` (`GET /?code=…&state={state}`, with the real state filled in) of the address the
/// page would send it back to.
fn browser_asking(requests: &[&str]) -> (impl Fn(&str) + use<>, Browser) {
    let seen = Browser::default();
    let (shared, requests) = (seen.clone(), requests.iter().map(|r| r.to_string()).collect::<Vec<_>>());
    let open = move |url: &str| {
        shared.lock().unwrap().url = url.to_string();
        let asked = fields(url.split_once('?').unwrap().1);
        let (port, state) = (port_of(&asked["redirect_uri"]), asked["state"].clone());
        let (shared, requests) = (shared.clone(), requests.clone());
        std::thread::spawn(move || {
            for request in requests {
                let page = visit(port, &request.replace("{state}", &state));
                shared.lock().unwrap().pages.push(page);
            }
        });
    };
    (open, seen)
}

/// What the page the browser was sent to was asked.
fn asked_of(browser: &Browser) -> HashMap<String, String> {
    let url = browser.lock().unwrap().url.clone();
    fields(url.split_once('?').unwrap().1)
}

/// Every address a browser was sent to, in order.
type Visits = Arc<Mutex<Vec<String>>>;

/// An `open` that plays a browser which may be sent somewhere more than once: the `n`th time
/// (from 0) it makes the `n`th of `requests`, with the state of the page it was sent to.
fn browser_answering(requests: &[&str]) -> (impl Fn(&str) + use<>, Visits) {
    let visits = Visits::default();
    let (shared, requests) = (visits.clone(), requests.iter().map(|r| r.to_string()).collect::<Vec<_>>());
    let open = move |url: &str| {
        let n = {
            let mut visits = shared.lock().unwrap();
            visits.push(url.to_string());
            visits.len() - 1
        };
        let asked = fields(url.split_once('?').unwrap().1);
        let (port, state) = (port_of(&asked["redirect_uri"]), asked["state"].clone());
        let request = requests.get(n).cloned().unwrap_or_else(|| "GET /nothing-more".into());
        std::thread::spawn(move || visit(port, &request.replace("{state}", &state)));
    };
    (open, visits)
}

/// What the `n`th page a browser was sent to was asked.
fn visit_asked(visits: &Visits, n: usize) -> HashMap<String, String> {
    let url = visits.lock().unwrap()[n].clone();
    fields(url.split_once('?').unwrap().1)
}

/// How many pages a browser was sent to.
fn visited(visits: &Visits) -> usize {
    visits.lock().unwrap().len()
}

/// The first `n` pages the browser was shown, once it has them.
fn pages(browser: &Browser, n: usize) -> Vec<String> {
    let started = Instant::now();
    loop {
        let pages = browser.lock().unwrap().pages.clone();
        if pages.len() >= n {
            return pages;
        }
        assert!(started.elapsed() < PATIENCE, "the browser was shown {} of {n} pages", pages.len());
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn sign_in_to(endpoints: &Endpoints, hint: &str, open: &dyn Fn(&str)) -> Result<Tokens, NetError> {
    sign_in_within(endpoints, &oauth(), hint, open, &AtomicBool::new(false), PATIENCE)
}

fn auth(result: Result<Tokens, NetError>) -> String {
    match result {
        Err(NetError::Proto(ProtoError::Auth(why))) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn protocol(result: Result<Tokens, NetError>) -> String {
    match result {
        Err(NetError::Proto(ProtoError::Protocol(why))) => why,
        other => panic!("expected an answer that makes no sense, got {other:?}"),
    }
}

#[test]
fn microsoft_endpoints() {
    let of = |tenant: &str| Endpoints::of(&OAuth { tenant: tenant.into(), ..oauth() });
    let common = of("");
    assert_eq!(common.authorize, "https://login.microsoftonline.com/common/oauth2/v2.0/authorize");
    assert_eq!(common.token, "https://login.microsoftonline.com/common/oauth2/v2.0/token");
    assert_eq!(
        common.scope,
        "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email"
    );
    assert_eq!(of(" consumers ").token, "https://login.microsoftonline.com/consumers/oauth2/v2.0/token");
    assert_eq!(of("contoso.onmicrosoft.com").authorize, "https://login.microsoftonline.com/contoso.onmicrosoft.com/oauth2/v2.0/authorize");
    assert_eq!(
        of("72f988bf-86f1-41af-91ab-2d7cd011db47").token,
        "https://login.microsoftonline.com/72f988bf-86f1-41af-91ab-2d7cd011db47/oauth2/v2.0/token"
    );
    // Whatever the settings say, the addresses are ones that can be spoken to.
    for tenant in ["../../evil", "a/b", "evil.example.org/x?", "a b", "é", "-", "a..b"] {
        assert_eq!(of(tenant), common, "{tenant:?}");
    }
    assert!(Url::parse(&common.authorize).is_ok() && Url::parse(&common.token).is_ok());

    // The page of a sign-in to mail is also asked for what Graph's tokens will be for.
    assert_eq!(common.consent, GRAPH_SCOPES);
    assert_eq!(Endpoints::for_resource(&oauth(), Resource::Mail), common);
    // Graph's own: the same addresses, its own scopes, and the one optional extra for the owner
    // to agree to on the page (a page that won't have it is asked again without it).
    let graph = Endpoints::for_resource(&OAuth { tenant: " consumers ".into(), ..oauth() }, Resource::Graph);
    assert_eq!(graph.authorize, "https://login.microsoftonline.com/consumers/oauth2/v2.0/authorize");
    assert_eq!(graph.token, "https://login.microsoftonline.com/consumers/oauth2/v2.0/token");
    assert_eq!(graph.scope, format!("{GRAPH_SCOPES_BEFORE} offline_access openid profile email"));
    assert_eq!(graph.consent, "https://graph.microsoft.com/MailboxSettings.Read");
    // A request for tokens names one resource: neither asks for a scope of the other's.
    assert!(!common.scope.contains("graph.microsoft.com") && !graph.scope.contains("outlook.office.com"));
    // Reading and changing mail through Graph is asked for only for an account kept in step that
    // way (R3): never by a sign-in to mail, nor by Graph's own.
    let mail = Endpoints::for_resource(&oauth(), Resource::GraphMail);
    assert_eq!(mail.scope, format!("{GRAPH_SCOPES_BEFORE} https://graph.microsoft.com/Mail.ReadWrite offline_access openid profile email"));
    assert_eq!((mail.authorize.as_str(), mail.consent.as_str()), (common.authorize.as_str(), graph.consent.as_str()));
    for other in [&common, &graph] {
        assert!(!other.scope.contains("Mail.ReadWrite") && !other.consent.contains("Mail.ReadWrite"));
    }
}

#[test]
fn the_challenge_is_the_hash_of_the_verifier() {
    // The example of RFC 7636, appendix B.
    assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    // A verifier is 43 characters that need no escaping, and never the same twice.
    let (one, other) = (random(32).unwrap(), random(32).unwrap());
    assert_eq!(one.len(), 43);
    assert!(one.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'), "{one}");
    assert_ne!(one, other);
}

#[test]
fn the_port_is_open_on_loopback_only() {
    let redirect = Redirect::listen().unwrap();
    assert!(!redirect.listeners.is_empty() && redirect.port != 0);
    for listener in &redirect.listeners {
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback(), "{address}");
        assert_eq!(address.port(), redirect.port);
    }
}

#[test]
fn signing_in_brings_back_the_tokens() {
    let server = FakeHttp::answering(ok(TOKENS));
    let (open, browser) = browser_asking(&["GET /favicon.ico", "GET /?code=the%2Fcode-123&state={state}&session_state=abc"]);
    let before = mailcraft_time::now();
    let tokens = sign_in_to(&endpoints_of(&server), " ada+mail@example.org ", &open).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("acc-ess.tok_en", "re-fresh.tok_en"));
    assert!((before + 3599..=mailcraft_time::now() + 3599).contains(&tokens.expires_at));
    assert!(tokens.fresh(before) && !tokens.fresh(before + 3600));

    // The page the owner was sent to, and what it was asked.
    let url = browser.lock().unwrap().url.clone();
    let (address, query) = url.split_once('?').unwrap();
    assert_eq!(address, endpoints_of(&server).authorize);
    let asked = asked_of(&browser);
    let was = |name: &str| asked.get(name).map_or("(missing)", String::as_str);
    assert_eq!(was("client_id"), "client-1");
    assert_eq!((was("response_type"), was("response_mode"), was("prompt")), ("code", "query", "select_account"));
    assert_eq!(was("scope"), "mail.read mail.send offline_access");
    assert_eq!(was("login_hint"), "ada+mail@example.org");
    assert_eq!(was("code_challenge_method"), "S256");
    assert_eq!(asked.len(), 10, "{asked:?}");
    let (redirect_uri, state) = (was("redirect_uri"), was("state"));
    // Exactly as registered: a path or a trailing slash would not match.
    assert_eq!(redirect_uri, format!("http://localhost:{}", port_of(redirect_uri)));
    assert!(state.len() >= 32, "{state}");
    // Written so that nothing in a value can be taken for the address's own punctuation.
    for escaped in ["login_hint=ada%2Bmail%40example.org", "scope=mail.read%20mail.send%20offline_access", "redirect_uri=http%3A%2F%2Flocalhost%3A"] {
        assert!(query.contains(escaped), "{escaped}: {query}");
    }
    assert!(query.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~%=&".contains(&b)), "{query}");

    // What the code was traded in with.
    assert_eq!(server.asked(), 1);
    let request = server.request(0);
    assert!(request.starts_with("POST /token HTTP/1.1\r\n"), "{request}");
    for header in
        [format!("Host: 127.0.0.1:{}\r\n", server.port), "Content-Type: application/x-www-form-urlencoded\r\n".into(), "Connection: close\r\n".into()]
    {
        assert!(request.contains(&header), "{header:?}: {request}");
    }
    let form = form_of(&server, 0);
    let sent = |name: &str| form.get(name).map_or("(missing)", String::as_str);
    assert_eq!((sent("grant_type"), sent("client_id"), sent("code")), ("authorization_code", "client-1", "the/code-123"));
    assert_eq!((sent("redirect_uri"), sent("scope")), (redirect_uri, "mail.read mail.send offline_access"));
    assert_eq!(form.len(), 6, "{form:?}");
    // The verifier is the one whose hash the page was shown, and only its hash went through the browser.
    let verifier = sent("code_verifier");
    assert!((43..=128).contains(&verifier.len()), "{verifier}");
    assert_eq!(challenge(verifier), was("code_challenge"));
    assert!(!url.contains(verifier));

    // What the browser was shown: nothing for the favicon, and a page that stands on its own.
    let pages = pages(&browser, 2);
    assert!(pages[0].starts_with("HTTP/1.1 404 Not Found\r\n"), "{}", pages[0]);
    let done = &pages[1];
    assert!(done.starts_with("HTTP/1.1 200 OK\r\n") && done.contains("Content-Type: text/html; charset=utf-8\r\n"), "{done}");
    assert!(done.contains("You can close this tab and return to MailCraft."), "{done}");
    let (head, body) = done.split_once("\r\n\r\n").unwrap();
    assert!(head.contains(&format!("Content-Length: {}\r\n", body.len())), "{head}");
    let lower = body.to_lowercase();
    for outside in ["<script", "src=", "href=", "url(", "@import", "http:", "https:", "<img", "<link", "<iframe", "<form"] {
        assert!(!lower.contains(outside), "{outside}: {body}");
    }
    // Nothing of the request is repeated in it.
    assert!(!done.contains("the/code-123") && !done.contains("the%2Fcode") && !done.contains(state));
}

#[test]
fn strangers_at_the_port_are_turned_away_and_the_wait_goes_on() {
    let server = FakeHttp::answering(ok(TOKENS));
    let oversized = format!("GET /?state={{state}}&code=x&junk={}", "x".repeat(3 * MAX_REQUEST));
    let (open, browser) = browser_asking(&[
        // Without the state, or with a guess at it: neither a code nor a refusal is believed.
        "GET /?code=stolen&state=guess",
        "GET /?code=stolen",
        "GET /?error=access_denied&state=guess",
        "GET /",
        // Other addresses, other ways of asking.
        "GET /callback?code=stolen&state={state}",
        "POST /?code=stolen&state={state}",
        "junk",
        &oversized,
        "GET /?code=the-code&state={state}",
    ]);
    // A connection that says nothing, as browsers open to have one ready, holds nothing up.
    let silent = Arc::new(Mutex::new(None));
    let kept = silent.clone();
    let open = move |url: &str| {
        let port = port_of(&fields(url.split_once('?').unwrap().1)["redirect_uri"]);
        *kept.lock().unwrap() = Some(TcpStream::connect(("localhost", port)).unwrap());
        open(url);
    };
    let tokens = sign_in_to(&endpoints_of(&server), "", &open).unwrap();
    assert_eq!(tokens.access, "acc-ess.tok_en");
    assert!(silent.lock().unwrap().is_some());
    // Only the real code was traded in.
    assert_eq!(server.asked(), 1);
    assert_eq!(form_of(&server, 0)["code"], "the-code");
    // No hint, none sent.
    assert!(!asked_of(&browser).contains_key("login_hint"));

    let pages = pages(&browser, 9);
    let status = |n: usize| pages[n].lines().next().unwrap_or("").to_string();
    for (n, page) in pages.iter().enumerate().take(4) {
        assert_eq!(status(n), "HTTP/1.1 400 Bad Request", "{n}");
        assert!(page.contains("Not the sign-in MailCraft is waiting for"), "{n}");
    }
    assert_eq!(status(4), "HTTP/1.1 404 Not Found");
    assert_eq!(status(5), "HTTP/1.1 405 Method Not Allowed");
    assert_eq!(status(6), "HTTP/1.1 405 Method Not Allowed");
    // The one that was hung up on may not have got its answer.
    assert!(pages[7].is_empty() || status(7) == "HTTP/1.1 431 Request Header Fields Too Large", "{}", status(7));
    assert_eq!(status(8), "HTTP/1.1 200 OK");
}

#[test]
fn a_browser_with_more_headers_than_are_read_still_signs_in() {
    let server = FakeHttp::answering(ok(TOKENS));
    // Every cookie the browser holds for `localhost` comes along, whoever set it.
    let laden = format!("GET /?code=the-code&state={{state}} HTTP/1.1\r\nHost: localhost\r\nCookie: other={}\r\n\r\n", "x".repeat(4 * MAX_REQUEST));
    let (open, browser) = browser_asking(&[&laden]);
    let tokens = sign_in_to(&endpoints_of(&server), "", &open).unwrap();
    assert_eq!(tokens.access, "acc-ess.tok_en");
    assert_eq!(form_of(&server, 0)["code"], "the-code");
    // And it is shown its page, though not all it sent was read.
    let page = &pages(&browser, 1)[0];
    assert!(page.starts_with("HTTP/1.1 200 OK\r\n") && page.contains("You can close this tab"), "{page}");

    // Nothing is believed of a first line that never ends, however it starts.
    let endless = format!("GET /?code=the-code&state={{state}}&junk={}", "x".repeat(2 * MAX_REQUEST));
    let (open, _) = browser_asking(&[&endless]);
    let result = sign_in_within(&endpoints_of(&server), &oauth(), "", &open, &AtomicBool::new(false), Duration::from_millis(600));
    assert_eq!(result, Err(NetError::SignInTimedOut));
    assert_eq!(server.asked(), 1);
}

#[test]
fn a_refusal_on_the_page_ends_the_sign_in() {
    let server = FakeHttp::answering(ok(TOKENS));
    // In the provider's words, of which the first line is for people.
    let (open, browser) = browser_asking(&[
        "GET /?error=access_denied&error_description=AADSTS65004%3A+User+declined+to+consent+to+access+the+app.%0D%0ATrace+ID%3A+1234%0D%0ATimestamp%3A+2026-10-11&state={state}",
    ]);
    assert_eq!(auth(sign_in_to(&endpoints_of(&server), "", &open)), "AADSTS65004: User declined to consent to access the app.");
    let page = &pages(&browser, 1)[0];
    assert!(page.starts_with("HTTP/1.1 200 OK\r\n") && page.contains("The sign-in didn't go through"), "{page}");
    assert!(!page.contains("AADSTS65004") && !page.contains("declined"));

    // In one word.
    let (open, _) = browser_asking(&["GET /?error=access_denied&state={state}"]);
    let error = sign_in_to(&endpoints_of(&server), "", &open).unwrap_err();
    assert_eq!(error.to_string(), "sign-in failed: access was refused on the sign-in page");
    let (open, _) = browser_asking(&["GET /?state={state}&error=interaction_required%0D%0A&code=ignored"]);
    assert_eq!(auth(sign_in_to(&endpoints_of(&server), "", &open)), "the sign-in server said \"interaction_required\"");
    // "Not now" is not "no".
    let (open, _) = browser_asking(&["GET /?error=temporarily_unavailable&state={state}"]);
    let busy = sign_in_to(&endpoints_of(&server), "", &open);
    assert_eq!(busy, Err(NetError::Proto(ProtoError::Rejected("the sign-in server said \"temporarily_unavailable\"".into()))));
    // The right state, and nothing to go on.
    let (open, browser) = browser_asking(&["GET /?state={state}&session_state=abc"]);
    assert_eq!(protocol(sign_in_to(&endpoints_of(&server), "", &open)), "the sign-in page came back without a code");
    assert!(pages(&browser, 1)[0].contains("The sign-in didn't go through"));
    // Nothing was ever traded in.
    assert_eq!(server.asked(), 0);
}

#[test]
fn a_sign_in_can_be_called_off() {
    let server = FakeHttp::answering(ok(TOKENS));
    // Before it starts: the owner isn't sent anywhere.
    let (open, browser) = browser_asking(&["GET /?code=the-code&state={state}"]);
    let cancel = AtomicBool::new(true);
    assert_eq!(sign_in_within(&endpoints_of(&server), &oauth(), "", &open, &cancel, PATIENCE), Err(NetError::SignInCancelled));
    assert_eq!(sign_in(&oauth(), "", &open, &cancel), Err(NetError::SignInCancelled));
    assert_eq!(browser.lock().unwrap().url, "");

    // While the owner is on the page.
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let open = move |_: &str| {
        let flag = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::Relaxed);
        });
    };
    let started = Instant::now();
    assert_eq!(sign_in_within(&endpoints_of(&server), &oauth(), "", &open, &cancel, PATIENCE), Err(NetError::SignInCancelled));
    assert!((Duration::from_millis(100)..Duration::from_secs(10)).contains(&started.elapsed()), "{:?}", started.elapsed());
    assert_eq!(server.asked(), 0);
}

#[test]
fn a_sign_in_nobody_finishes_times_out() {
    let server = FakeHttp::answering(ok(TOKENS));
    let opened = Mutex::new(String::new());
    let open = |url: &str| *opened.lock().unwrap() = url.to_string();
    let started = Instant::now();
    let result = sign_in_within(&endpoints_of(&server), &oauth(), "", &open, &AtomicBool::new(false), Duration::from_millis(300));
    assert_eq!(result, Err(NetError::SignInTimedOut));
    assert!((Duration::from_millis(300)..Duration::from_secs(10)).contains(&started.elapsed()), "{:?}", started.elapsed());
    // Told apart from a sign-in that was called off, in words too.
    assert_ne!(NetError::SignInTimedOut.to_string(), NetError::SignInCancelled.to_string());
    assert!(NetError::SignInTimedOut.to_string().contains("in time") && NetError::SignInCancelled.to_string().contains("cancelled"));
    // The port is closed again: a browser that comes back late finds nobody.
    let port = port_of(&fields(opened.lock().unwrap().split_once('?').unwrap().1)["redirect_uri"]);
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    assert_eq!(server.asked(), 0);
}

#[test]
fn the_token_endpoint_saying_no() {
    let refusal = r#"{"error":"invalid_grant","error_description":"AADSTS70000: The provided value for the 'code' parameter is not valid.\r\nTrace ID: 1234\r\nCorrelation ID: 5678\r\nTimestamp: 2026-10-11 10:00:00Z","error_codes":[70000],"error_uri":"https://login.example.org/error?code=70000"}"#;
    let server = FakeHttp::answering(response("400 Bad Request", refusal));
    let (open, browser) = browser_asking(&["GET /?code=the-code&state={state}"]);
    let error = sign_in_to(&endpoints_of(&server), "", &open).unwrap_err();
    assert_eq!(error.to_string(), "sign-in failed: AADSTS70000: The provided value for the 'code' parameter is not valid.");
    // The browser had its page before the code was traded in.
    assert!(pages(&browser, 1)[0].contains("You can close this tab"));
    // The same from a server that calls it a success.
    let server = FakeHttp::answering(ok(r#"{"error":"invalid_client"}"#));
    assert_eq!(auth(refresh_at(&endpoints_of(&server), &oauth(), "re-fresh")), "the sign-in server said \"invalid_client\"");
    let server =
        FakeHttp::answering(response("503 Service Unavailable", r#"{"error":"temporarily_unavailable","error_description":"  \r\n Busy.\n"}"#));
    assert_eq!(refresh_at(&endpoints_of(&server), &oauth(), "re-fresh"), Err(NetError::Proto(ProtoError::Rejected("Busy.".into()))));
}

#[test]
fn refreshing_brings_new_tokens() {
    let server = FakeHttp::start(|n, _| match n {
        0 => ok(r#"{"token_type":"bearer","expires_in":"7200","access_token":"new-access","refresh_token":"new-refresh"}"#),
        // No new refresh token: the one that was sent stays good.
        1 => ok(r#"{"token_type":"Bearer","expires_in":3600,"access_token":"newer-access"}"#),
        _ => response(
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"AADSTS70008: The refresh token has expired due to inactivity."}"#,
        ),
    });
    let before = mailcraft_time::now();
    let tokens = refresh_at(&endpoints_of(&server), &oauth(), "old-refresh").unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("new-access", "new-refresh"));
    assert!((before + 7200..=mailcraft_time::now() + 7200).contains(&tokens.expires_at));
    let request = server.request(0);
    assert!(request.starts_with("POST /token HTTP/1.1\r\n"), "{request}");
    let form = form_of(&server, 0);
    assert_eq!(form.len(), 4, "{form:?}");
    assert_eq!(
        (form["grant_type"].as_str(), form["client_id"].as_str(), form["refresh_token"].as_str()),
        ("refresh_token", "client-1", "old-refresh")
    );
    assert_eq!(form["scope"], "mail.read mail.send offline_access");

    let tokens = refresh_at(&endpoints_of(&server), &oauth(), &tokens.refresh).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("newer-access", "new-refresh"));
    assert_eq!(form_of(&server, 1)["refresh_token"], "new-refresh");

    // One the provider no longer takes: the owner has to sign in again.
    assert_eq!(auth(refresh_at(&endpoints_of(&server), &oauth(), &tokens.refresh)), "AADSTS70008: The refresh token has expired due to inactivity.");
    // Without one there is nothing to ask.
    assert_eq!(auth(refresh_at(&endpoints_of(&server), &oauth(), "")), "there is no earlier sign-in to renew");
    assert_eq!(auth(refresh(&oauth(), "")), "there is no earlier sign-in to renew");
    assert_eq!(auth(refresh_at(&endpoints_of(&server), &oauth(), "two\r\nlines")), "there is no earlier sign-in to renew");
    assert_eq!(server.asked(), 3);
}

#[test]
fn the_page_is_asked_for_more_than_the_tokens_are_for() {
    let server = FakeHttp::answering(ok(TOKENS));
    let (open, browser) = browser_asking(&["GET /?code=the-code&state={state}"]);
    let tokens = sign_in_to(&endpoints_with_consent(&server), "ada@example.org", &open).unwrap();
    assert_eq!(tokens.access, "acc-ess.tok_en");
    // On the page the owner agrees to both…
    let asked = asked_of(&browser);
    assert_eq!(asked["scope"], "mail.read mail.send offline_access graph.send graph.calendars");
    assert_eq!(asked.len(), 10, "{asked:?}");
    // …and the code is traded in for the mail servers' tokens alone: a request for tokens names one resource.
    assert_eq!(server.asked(), 1);
    let form = form_of(&server, 0);
    assert_eq!(
        (form["grant_type"].as_str(), form["code"].as_str(), form["scope"].as_str()),
        ("authorization_code", "the-code", "mail.read mail.send offline_access")
    );
    assert_eq!(form.len(), 6, "{form:?}");
    assert!(!server.request(0).contains("graph"), "{}", server.request(0));

    // As the provider's own page is asked: mail's scopes, then Graph's.
    let real = Endpoints::of(&oauth());
    let url = authorize_url(&real, &real.consent, "client-1", "http://localhost:49152", "the-state", "the-challenge", "");
    assert!(url.starts_with("https://login.microsoftonline.com/common/oauth2/v2.0/authorize?"), "{url}");
    assert_eq!(
        fields(url.split_once('?').unwrap().1)["scope"],
        format!(
            "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email {GRAPH_SCOPES}"
        )
    );
}

#[test]
fn graph_can_be_signed_in_to_on_its_own() {
    // What the page is asked for is what the tokens are for: Graph's scopes, and nothing of mail's.
    let server =
        FakeHttp::answering(ok(r#"{"token_type":"Bearer","expires_in":3600,"access_token":"graph-access","refresh_token":"newer-refresh"}"#));
    let graph = Endpoints::for_resource(&oauth(), Resource::Graph);
    let endpoints = Endpoints { scope: graph.scope.clone(), consent: graph.consent.clone(), ..endpoints_of(&server) };
    let (open, visits) = browser_answering(&["GET /?code=graph-code&state={state}"]);
    let tokens = sign_in_to(&endpoints, "ada@example.org", &open).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("graph-access", "newer-refresh"));
    // The tokens are for what Graph was always asked for; the page is asked for one thing more
    // (the mailbox's settings, for its categories), which the refresh token then covers too.
    let expected = format!("{GRAPH_SCOPES_BEFORE} offline_access openid profile email");
    assert_eq!(visit_asked(&visits, 0)["scope"], format!("{expected} https://graph.microsoft.com/MailboxSettings.Read"));
    assert_eq!(visit_asked(&visits, 0)["login_hint"], "ada@example.org");
    let form = form_of(&server, 0);
    assert_eq!((form["code"].as_str(), form["scope"].as_str()), ("graph-code", expected.as_str()));
    assert!(!server.request(0).contains("outlook.office.com"));

    // A page that won't have the one thing more is asked again without it: connecting the
    // calendar and contacts works as it did before the extra was asked for.
    let (open, visits) =
        browser_answering(&["GET /?error=invalid_scope&error_description=AADSTS70011%3A+No+such+scope.&state={state}", "GET /?code=x&state={state}"]);
    let tokens = sign_in_to(&endpoints, "", &open).unwrap();
    assert_eq!(tokens.access, "graph-access");
    assert_eq!((visited(&visits), server.asked()), (2, 2));
    assert_eq!(visit_asked(&visits, 1)["scope"], expected);
    // With nothing more to agree to there is nothing to leave out: a refusal of the scopes is the answer.
    let bare = Endpoints { consent: String::new(), ..endpoints.clone() };
    let (open, visits) =
        browser_answering(&["GET /?error=invalid_scope&error_description=AADSTS70011%3A+No+such+scope.&state={state}", "GET /?code=x&state={state}"]);
    assert_eq!(auth(sign_in_to(&bare, "", &open)), "AADSTS70011: No such scope.");
    assert_eq!((visited(&visits), server.asked()), (1, 2));
    // Like any sign-in, it can be called off before the owner is sent anywhere.
    let cancelled = sign_in_for(&oauth(), "ada@example.org", &open, &AtomicBool::new(true), Resource::Graph);
    assert_eq!((cancelled, visited(&visits)), (Err(NetError::SignInCancelled), 1));
}

#[test]
fn a_page_that_refuses_the_scopes_is_asked_again_for_mail_alone() {
    for refusal in [
        "error=invalid_scope",
        "error=invalid_scope&error_description=AADSTS70011%3A+The+provided+value+for+the+input+parameter+%27scope%27+is+not+valid.",
        "error=invalid_request&error_description=AADSTS28000%3A+Provided+value+for+the+input+parameter+scope+is+not+valid+because+it+contains+more+than+one+resource.",
        "error=invalid_client&error_description=AADSTS650053%3A+The+application+%27MailCraft%27+asked+for+scope+%27Mail.Send%27+that+doesn%27t+exist+on+the+resource.",
        "error=invalid_resource&error_description=The+resource+is+not+known+here.",
        "error=access_denied&error_description=AADSTS650057%3A+Invalid+resource.%0D%0ATrace+ID%3A+1234",
    ] {
        let server = FakeHttp::answering(ok(TOKENS));
        let (open, visits) = browser_answering(&[&format!("GET /?{refusal}&state={{state}}"), "GET /?code=second-code&state={state}"]);
        let tokens = sign_in_to(&endpoints_with_consent(&server), "ada@example.org", &open).unwrap();
        assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("acc-ess.tok_en", "re-fresh.tok_en"), "{refusal}");
        // The owner was sent to the page twice: with everything, then for mail alone.
        assert_eq!(visited(&visits), 2, "{refusal}");
        let (first, second) = (visit_asked(&visits, 0), visit_asked(&visits, 1));
        assert_eq!(first["scope"], "mail.read mail.send offline_access graph.send graph.calendars", "{refusal}");
        assert_eq!(second["scope"], "mail.read mail.send offline_access", "{refusal}");
        assert_eq!((second["login_hint"].as_str(), second["prompt"].as_str(), second.len()), ("ada@example.org", "select_account", first.len()));
        // The second go is a sign-in of its own: nothing of the first is good for it.
        assert_ne!(first["state"], second["state"]);
        assert_ne!(first["code_challenge"], second["code_challenge"]);
        // Only its code was traded in, for mail's tokens, with its own verifier.
        assert_eq!(server.asked(), 1, "{refusal}");
        let form = form_of(&server, 0);
        assert_eq!((form["code"].as_str(), form["scope"].as_str()), ("second-code", "mail.read mail.send offline_access"));
        assert_eq!(form["redirect_uri"], second["redirect_uri"]);
        assert_eq!(challenge(&form["code_verifier"]), second["code_challenge"]);
    }
}

#[test]
fn a_code_refused_for_its_scopes_is_asked_for_again_for_mail_alone() {
    // The page agreed to everything; it is the token endpoint that won't have it.
    let server = FakeHttp::start(|n, _| match n {
        0 => response(
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"AADSTS28000: Provided value for the input parameter scope is not valid because it contains more than one resource.","error_codes":[28000]}"#,
        ),
        _ => ok(TOKENS),
    });
    let (open, visits) = browser_answering(&["GET /?code=first-code&state={state}", "GET /?code=second-code&state={state}"]);
    let tokens = sign_in_to(&endpoints_with_consent(&server), "", &open).unwrap();
    assert_eq!(tokens.access, "acc-ess.tok_en");
    assert_eq!((visited(&visits), server.asked()), (2, 2));
    assert_eq!(visit_asked(&visits, 1)["scope"], "mail.read mail.send offline_access");
    assert_eq!((form_of(&server, 0)["code"].as_str(), form_of(&server, 1)["code"].as_str()), ("first-code", "second-code"));

    // Any other no from there is the answer, as it always was.
    let no = r#"{"error":"invalid_grant","error_description":"AADSTS70000: The provided value for the 'code' parameter is not valid."}"#;
    let server = FakeHttp::answering(response("400 Bad Request", no));
    let (open, visits) = browser_answering(&["GET /?code=first-code&state={state}", "GET /?code=second-code&state={state}"]);
    assert_eq!(
        auth(sign_in_to(&endpoints_with_consent(&server), "", &open)),
        "AADSTS70000: The provided value for the 'code' parameter is not valid."
    );
    assert_eq!((visited(&visits), server.asked()), (1, 1));
}

#[test]
fn only_a_refusal_of_the_scopes_is_asked_about_twice() {
    let server = FakeHttp::answering(ok(TOKENS));
    let again = "GET /?code=second-code&state={state}";
    // The owner said no, the server is in trouble, the page came back with nothing: once is enough.
    for refusal in [
        "error=access_denied&error_description=AADSTS65004%3A+User+declined+to+consent+to+access+the+app.",
        "error=access_denied&error_subcode=cancel",
        "error=consent_required",
        "error=server_error",
        "error=temporarily_unavailable",
        "session_state=abc",
    ] {
        let (open, visits) = browser_answering(&[&format!("GET /?{refusal}&state={{state}}"), again]);
        assert!(sign_in_to(&endpoints_with_consent(&server), "", &open).is_err(), "{refusal}");
        assert_eq!(visited(&visits), 1, "{refusal}");
    }
    // The second go is the last one: what comes of it is what is told.
    let (open, visits) =
        browser_answering(&["GET /?error=invalid_scope&state={state}", "GET /?error=invalid_scope&error_description=Still+no.&state={state}", again]);
    assert_eq!(auth(sign_in_to(&endpoints_with_consent(&server), "", &open)), "Still no.");
    assert_eq!(visited(&visits), 2);
    let (open, visits) = browser_answering(&["GET /?error=invalid_scope&state={state}", "GET /?error=access_denied&state={state}"]);
    assert_eq!(auth(sign_in_to(&endpoints_with_consent(&server), "", &open)), "access was refused on the sign-in page");
    assert_eq!(visited(&visits), 2);
    // A sign-in that is called off isn't started over, however much was asked for.
    let (cancel, opened) = (AtomicBool::new(false), Mutex::new(0));
    let open = |_: &str| {
        *opened.lock().unwrap() += 1;
        cancel.store(true, Ordering::Relaxed);
    };
    let called_off = sign_in_within(&endpoints_with_consent(&server), &oauth(), "", &open, &cancel, PATIENCE);
    assert_eq!((called_off, *opened.lock().unwrap()), (Err(NetError::SignInCancelled), 1));
    // Nor is one that nobody finishes.
    let opened = Mutex::new(0);
    let open = |_: &str| *opened.lock().unwrap() += 1;
    let unfinished = sign_in_within(&endpoints_with_consent(&server), &oauth(), "", &open, &AtomicBool::new(false), Duration::from_millis(200));
    assert_eq!((unfinished, *opened.lock().unwrap()), (Err(NetError::SignInTimedOut), 1));
    // None of it ever got as far as a code.
    assert_eq!(server.asked(), 0);
}

const FAVORITES_SCOPES: &str = "https://graph.microsoft.com/MailboxConfigItem.Read https://graph.microsoft.com/Mail.ReadBasic";

#[test]
fn the_favorites_permissions_are_asked_for_by_themselves_and_never_with_a_sign_in() {
    // Reading Outlook's Favorites takes two permissions more, of which the reference says one
    // takes an administrator in an organisation. An ordinary sign-in must not hang on that:
    // neither its page nor its tokens ask for them, for mail or for the calendar and contacts.
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        let endpoints = Endpoints::for_resource(&oauth(), resource);
        let asked = format!("{} {} {}", endpoints.scope, endpoints.consent, endpoints.lesser);
        assert!(!asked.contains("MailboxConfigItem") && !asked.contains("Mail.ReadBasic"), "{asked}");
        assert_eq!(endpoints.lesser, "");
    }
    // The owner asks for them on a page of their own: what Graph was asked for before, and
    // these two, in the one request (page and tokens alike), with nothing to fall back to.
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"graph-access","refresh_token":"newer-refresh"}"#;
    let server = FakeHttp::answering(ok(granted));
    let wanted = format!("{GRAPH_SCOPES} {FAVORITES_SCOPES} offline_access openid profile email");
    let endpoints = Endpoints { scope: wanted.clone(), ..endpoints_of(&server) };
    let ask = |open: &dyn Fn(&str)| sign_in_for_favorites_within(&endpoints, &oauth(), "ada@example.org", open, &AtomicBool::new(false), PATIENCE);
    let (open, visits) = browser_answering(&["GET /?code=the-code&state={state}"]);
    let tokens = ask(&open).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("graph-access", "newer-refresh"));
    assert_eq!((visit_asked(&visits, 0)["scope"].as_str(), form_of(&server, 0)["scope"].as_str()), (wanted.as_str(), wanted.as_str()));
    assert_eq!(visit_asked(&visits, 0)["login_hint"], "ada@example.org");
    assert!(!server.request(0).contains("outlook.office.com"));

    // The organisation keeps the permission to its administrators: said as what it is, once,
    // and the owner is not sent to a second page.
    for refusal in [
        "error=access_denied&error_description=AADSTS90094%3A+The+grant+requires+admin+permission.",
        "error=access_denied&error_subcode=cancel&error_description=AADSTS90095%3A+Approval+is+required.%0D%0ATrace+ID%3A+12",
        "error=invalid_client&error_description=AADSTS900941%3A+An+administrator+has+to+agree.",
    ] {
        let (open, visits) = browser_answering(&[&format!("GET /?{refusal}&state={{state}}"), "GET /?code=x&state={state}"]);
        assert_eq!(ask(&open), Err(FavoritesRefusal::Approval), "{refusal}");
        assert_eq!(visited(&visits), 1, "{refusal}");
    }
    // The organisation has shut the door for another reason, which no approval of these
    // permissions would open: its rules for signing in, an application it switched off or
    // keeps from this person, a service it doesn't have. That is another thing to be told.
    for refusal in [
        "error=access_denied&error_description=AADSTS53003%3A+Access+has+been+blocked+by+Conditional+Access+policies.",
        "error=unauthorized_client&error_description=AADSTS7000112%3A+Application+is+disabled.",
        "error=access_denied&error_description=AADSTS50105%3A+The+signed+in+user+is+not+assigned+to+a+role+for+the+application.",
        "error=access_denied&error_description=AADSTS650052%3A+The+app+needs+access+to+a+service+that+your+organization+has+not+enabled.",
        // (Shut, and an administrator would have to agree as well: shut is what it is.)
        "error=access_denied&error_description=AADSTS90094%3A+Admin+consent.+AADSTS53003%3A+Blocked.",
    ] {
        let (open, visits) = browser_answering(&[&format!("GET /?{refusal}&state={{state}}"), "GET /?code=x&state={state}"]);
        assert_eq!((ask(&open), visited(&visits)), (Err(FavoritesRefusal::Organisation), 1), "{refusal}");
    }
    // This kind of account has no such permission.
    for refusal in [
        "error=invalid_scope&error_description=AADSTS70011%3A+No+such+scope.",
        "error=invalid_request&error_description=AADSTS500207%3A+The+account+type+can%27t+be+used+for+the+resource.",
    ] {
        let (open, visits) = browser_answering(&[&format!("GET /?{refusal}&state={{state}}"), "GET /?code=x&state={state}"]);
        assert_eq!((ask(&open), visited(&visits)), (Err(FavoritesRefusal::Account), 1), "{refusal}");
    }
    // The owner said no, or the page came to nothing: that, and nothing about anybody else.
    let (open, _) = browser_answering(&["GET /?error=access_denied&error_description=AADSTS65004%3A+User+declined+to+consent.&state={state}"]);
    assert!(matches!(ask(&open), Err(FavoritesRefusal::Other(NetError::Proto(ProtoError::Auth(_))))));
    let cancelled = sign_in_for_favorites(&oauth(), "", &|_| {}, &AtomicBool::new(true));
    assert_eq!(cancelled, Err(FavoritesRefusal::Other(NetError::SignInCancelled)));
    // Only the first of all those ever got a code to trade in.
    assert_eq!(server.asked(), 1);
}

#[test]
fn a_sign_in_that_does_not_cover_the_favorites_keeps_everything_it_had() {
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"graph-access","refresh_token":"re-fresh-2"}"#;
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'.","error_codes":[65001],"suberror":"consent_required"}"#;
    let over = r#"{"error":"invalid_grant","error_description":"AADSTS70000: The user could not be authenticated as the grant is expired. The user must sign in again."}"#;
    let most = |server: &FakeHttp| Endpoints { scope: format!("{GRAPH_SCOPES} {FAVORITES_SCOPES} offline_access"), ..endpoints_of(server) };
    let all = |server: &FakeHttp| Endpoints { scope: format!("{GRAPH_SCOPES} offline_access"), ..endpoints_of(server) };
    let plain = |server: &FakeHttp| Endpoints { scope: format!("{GRAPH_SCOPES_BEFORE} offline_access"), ..endpoints_of(server) };
    let ask =
        |server: &FakeHttp| refresh_graph_with_favorites_at(&most(server), &all(server), &plain(server), &endpoints_of(server), &oauth(), "re-fresh");
    // The owner agreed to all of it: one question, and the tokens are good for the Favorites.
    let server = FakeHttp::answering(ok(granted));
    let got = ask(&server).unwrap();
    assert_eq!((got.tokens.access.as_str(), got.extras_missing, got.covers_favorites, server.asked()), ("graph-access", false, true, 1));
    assert!(form_of(&server, 0)["scope"].contains("MailboxConfigItem.Read") && form_of(&server, 0)["scope"].contains("Mail.ReadBasic"));

    // The owner was never asked for the Favorites: the tokens come exactly as they came before
    // (the mailbox's settings among them), and all that is missing is that.
    for no in [unasked, over] {
        let server = FakeHttp::start(move |n, _| if n == 0 { response("400 Bad Request", no) } else { ok(granted) });
        let got = ask(&server).unwrap();
        assert_eq!((got.tokens.access.as_str(), got.extras_missing, got.covers_favorites, server.asked()), ("graph-access", false, false, 2));
        let second = form_of(&server, 1);
        assert!(!second["scope"].contains("MailboxConfigItem") && !second["scope"].contains("Mail.ReadBasic"), "{second:?}");
        assert!(second["scope"].contains("MailboxSettings.Read") && second["scope"].contains("Calendars.ReadWrite"));
    }
    // A sign-in from before the settings were asked for either: as it was, too.
    let server = FakeHttp::start(move |n, _| if n < 2 { response("400 Bad Request", unasked) } else { ok(granted) });
    let got = ask(&server).unwrap();
    assert_eq!((got.extras_missing, got.covers_favorites, server.asked()), (true, false, 3));
    // Nobody agreed to Graph at all, and a sign-in that is over: told apart as they always were.
    let server = FakeHttp::answering(response("400 Bad Request", unasked));
    assert!(ask(&server).unwrap_err().is_consent_needed());
    let server = FakeHttp::answering(response("400 Bad Request", over));
    let refused = ask(&server).unwrap_err();
    assert!(refused.is_refused_sign_in() && !format!("{refused} {refused:?}").contains("re-fresh"));
    // The provider can't say just now: nothing is concluded, and nothing more is asked.
    let busy = r#"{"error":"temporarily_unavailable","error_description":"Try again."}"#;
    let server = FakeHttp::answering(response("503 Service Unavailable", busy));
    assert!(ask(&server).is_err());
    assert_eq!(server.asked(), 1);
    // Asked for without the Favorites, the tokens are not said to cover them.
    let server = FakeHttp::answering(ok(granted));
    let got = refresh_graph_at(&all(&server), &plain(&server), &endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert!(!got.covers_favorites && !form_of(&server, 0)["scope"].contains("MailboxConfigItem"));
}

#[test]
fn a_page_that_will_not_have_everything_is_asked_for_what_was_asked_before() {
    // Were more put on the page of a sign-in than was before (`Endpoints::lesser` says what
    // was): a page that refuses it, for what it asks or because it is not the owner's to give,
    // is asked once more for what it was always asked for, and only then for mail alone.
    let server = FakeHttp::answering(ok(TOKENS));
    let wide = |server: &FakeHttp| Endpoints { consent: "graph.send graph.more".into(), lesser: "graph.send".into(), ..endpoints_of(server) };
    for refusal in [
        "error=invalid_scope&error_description=AADSTS70011%3A+No+such+scope.",
        "error=access_denied&error_description=AADSTS90094%3A+The+grant+requires+admin+permission.",
        "error=access_denied&error_description=AADSTS65004%3A+User+declined+to+consent+to+access+the+app.",
    ] {
        let (open, visits) = browser_answering(&[&format!("GET /?{refusal}&state={{state}}"), "GET /?code=second-code&state={state}"]);
        let tokens = sign_in_to(&wide(&server), "ada@example.org", &open).unwrap();
        assert_eq!(tokens.access, "acc-ess.tok_en", "{refusal}");
        assert_eq!(visited(&visits), 2, "{refusal}");
        assert_eq!(visit_asked(&visits, 0)["scope"], "mail.read mail.send offline_access graph.send graph.more");
        assert_eq!(visit_asked(&visits, 1)["scope"], "mail.read mail.send offline_access graph.send");
    }
    // Refused for its scopes again: for mail alone, as before. Three pages at most.
    let again = "GET /?error=invalid_scope&state={state}";
    let (open, visits) = browser_answering(&[again, again, "GET /?code=third-code&state={state}"]);
    assert_eq!(sign_in_to(&wide(&server), "", &open).unwrap().access, "acc-ess.tok_en");
    assert_eq!((visited(&visits), visit_asked(&visits, 2)["scope"].as_str()), (3, "mail.read mail.send offline_access"));
    let (open, visits) = browser_answering(&[again, again, again, "GET /?code=fourth-code&state={state}"]);
    assert!(sign_in_to(&wide(&server), "", &open).is_err());
    assert_eq!(visited(&visits), 3);
    // A sign-in that is called off or never finished is not started over.
    let opened = Mutex::new(0);
    let open = |_: &str| *opened.lock().unwrap() += 1;
    let unfinished = sign_in_within(&wide(&server), &oauth(), "", &open, &AtomicBool::new(false), Duration::from_millis(200));
    assert_eq!((unfinished, *opened.lock().unwrap()), (Err(NetError::SignInTimedOut), 1));
}

/// What a refresh for `resource` comes to when the token endpoint answers `answer`.
fn refreshed_for(resource: Resource, answer: Vec<u8>) -> Result<Tokens, NetError> {
    let server = FakeHttp::answering(answer);
    let endpoints = if resource == Resource::Graph { graph_endpoints_of(&server) } else { endpoints_of(&server) };
    refresh_for_at(&endpoints, &oauth(), "re-fresh", resource)
}

fn consent_needed(result: Result<Tokens, NetError>) -> String {
    match result {
        Err(error @ NetError::ConsentNeeded(_)) => {
            // Not a sign-in that is over, and told apart from one in words too.
            assert!(error.is_consent_needed() && !error.is_refused_sign_in(), "{error:?}");
            assert!(error.to_string().starts_with("permission is still to be given: "), "{error}");
            match error {
                NetError::ConsentNeeded(why) => why,
                _ => String::new(),
            }
        }
        other => panic!("expected to be told that the owner has to agree first, got {other:?}"),
    }
}

#[test]
fn the_refresh_token_gets_graph_its_own_tokens() {
    let server = FakeHttp::start(|n, _| match n {
        0 => ok(r#"{"token_type":"Bearer","expires_in":3600,"access_token":"graph-access","refresh_token":"newer-refresh"}"#),
        _ => ok(r#"{"token_type":"Bearer","expires_in":3600,"access_token":"mail-access"}"#),
    });
    let before = mailcraft_time::now();
    let tokens = refresh_for_at(&graph_endpoints_of(&server), &oauth(), "re-fresh", Resource::Graph).unwrap();
    // The access token is Graph's; the refresh token is the one to keep for both.
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("graph-access", "newer-refresh"));
    assert!((before + 3600..=mailcraft_time::now() + 3600).contains(&tokens.expires_at));
    let form = form_of(&server, 0);
    assert_eq!(form.len(), 4, "{form:?}");
    assert_eq!(
        (form["grant_type"].as_str(), form["client_id"].as_str(), form["refresh_token"].as_str(), form["scope"].as_str()),
        ("refresh_token", "client-1", "re-fresh", "graph.send graph.calendars offline_access")
    );
    // And it still gets mail's, asked for mail's scopes.
    let tokens = refresh_for_at(&endpoints_of(&server), &oauth(), &tokens.refresh, Resource::Mail).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str()), ("mail-access", "newer-refresh"));
    assert_eq!(
        (form_of(&server, 1)["refresh_token"].as_str(), form_of(&server, 1)["scope"].as_str()),
        ("newer-refresh", "mail.read mail.send offline_access")
    );
    // Without a refresh token there is nothing to ask with, for either.
    assert_eq!(auth(refresh_for(&oauth(), "", Resource::Graph)), "there is no earlier sign-in to renew");
    assert_eq!(auth(refresh_for(&oauth(), "two\r\nlines", Resource::Mail)), "there is no earlier sign-in to renew");
    assert_eq!(server.asked(), 2);
}

#[test]
fn a_refresh_for_graph_tells_consent_that_is_needed_from_a_sign_in_that_is_over() {
    let bad = |body: &str| response("400 Bad Request", body);
    // Nobody agreed to Graph: the refresh token is good, and mail goes on working.
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'. Send an interactive authorization request for this user and resource.\r\nTrace ID: 1234\r\nCorrelation ID: 5678\r\nTimestamp: 2026-10-12 08:00:00Z","error_codes":[65001],"suberror":"consent_required","error_uri":"https://login.example.org/error?code=65001"}"#;
    assert_eq!(
        consent_needed(refreshed_for(Resource::Graph, bad(unasked))),
        "AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'. Send an interactive authorization request for this user and resource."
    );
    // Said in any one of the ways it is said.
    for (said, shown) in [
        (r#"{"error":"invalid_grant","error_description":"AADSTS65001: Not consented."}"#, "AADSTS65001: Not consented."),
        (r#"{"error":"invalid_grant","error_description":"Consent is needed.","error_codes":[65001]}"#, "Consent is needed."),
        (r#"{"error":"invalid_grant","error_description":"Consent is needed.","suberror":"consent_required"}"#, "Consent is needed."),
        (r#"{"error":"invalid_grant","error_codes":[70000,65001]}"#, "the sign-in doesn't cover Microsoft Graph"),
        (r#"{"error":"consent_required"}"#, "the sign-in server said \"consent_required\""),
        (
            r#"{"error":"interaction_required","error_description":"AADSTS50076: You must use multi-factor authentication to access the resource."}"#,
            "AADSTS50076: You must use multi-factor authentication to access the resource.",
        ),
        (
            r#"{"error":"invalid_grant","error_description":"AADSTS90094: An administrator has to agree to this."}"#,
            "AADSTS90094: An administrator has to agree to this.",
        ),
        // The provider won't hand Graph's scopes to this application at all: the same, as the
        // sign-in itself is no worse for it.
        (
            r#"{"error":"invalid_scope","error_description":"AADSTS70011: The provided value for the input parameter 'scope' is not valid."}"#,
            "AADSTS70011: The provided value for the input parameter 'scope' is not valid.",
        ),
        (r#"{"error":"invalid_client","error_description":"AADSTS650057: Invalid resource."}"#, "AADSTS650057: Invalid resource."),
    ] {
        assert_eq!(consent_needed(refreshed_for(Resource::Graph, bad(said))), shown, "{said}");
        // A server that calls it a success is not believed.
        assert_eq!(consent_needed(refreshed_for(Resource::Graph, ok(said))), shown, "{said}");
        // For mail, signing in again is what puts any refusal right: there it reads as it always did.
        let for_mail = refreshed_for(Resource::Mail, bad(said)).unwrap_err();
        assert!(for_mail.is_refused_sign_in() && !for_mail.is_consent_needed(), "{said}: {for_mail:?}");
        let server = FakeHttp::answering(bad(said));
        assert_eq!(refresh_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap_err(), for_mail, "{said}");
    }

    // The refresh token is no longer good for anything: the owner signs in again.
    for over in [
        r#"{"error":"invalid_grant","error_description":"AADSTS70008: The refresh token has expired due to inactivity.","error_codes":[70008]}"#,
        r#"{"error":"invalid_grant","error_description":"AADSTS50173: The provided grant has expired due to it being revoked.","suberror":"bad_token"}"#,
        r#"{"error":"invalid_grant","error_description":"AADSTS650010: Read whole, this is not the number of a lack of consent."}"#,
        r#"{"error":"invalid_grant"}"#,
        r#"{"error":"invalid_client","error_description":"AADSTS700016: The application was not found in the directory."}"#,
        r#"{"error":"unauthorized_client"}"#,
    ] {
        let error = refreshed_for(Resource::Graph, bad(over)).unwrap_err();
        assert!(error.is_refused_sign_in() && !error.is_consent_needed(), "{over}: {error:?}");
        assert_eq!(refreshed_for(Resource::Mail, bad(over)).unwrap_err(), error, "{over}");
    }
    assert_eq!(auth(refreshed_for(Resource::Graph, bad(r#"{"error":"invalid_grant"}"#))), "the earlier sign-in is no longer good");

    // Neither: the provider can't say just now, or didn't answer as one. What was kept is as good as it was.
    for trouble in [
        response("503 Service Unavailable", r#"{"error":"temporarily_unavailable","error_description":"Busy."}"#),
        response("500 Internal Server Error", r#"{"error":"server_error"}"#),
        response("500 Internal Server Error", "<html>oops</html>"),
        ok("not json"),
        ok(r#"{"token_type":"Bearer"}"#),
        Vec::new(),
    ] {
        let error = refreshed_for(Resource::Graph, trouble.clone()).unwrap_err();
        assert!(!error.is_refused_sign_in() && !error.is_consent_needed(), "{error:?}");
        assert_eq!(refreshed_for(Resource::Mail, trouble).unwrap_err(), error);
    }
}

#[test]
fn no_secret_shows_when_consent_is_needed() {
    // A careless server again: it repeats the form it was sent in its refusal.
    let echo = |_: usize, request: &str| {
        let body = request.split_once("\r\n\r\n").unwrap().1;
        let description = format!("AADSTS65001: Not consented. {} was sent: {body}", "no good ".repeat(16));
        response("400 Bad Request", &format!(r#"{{"error":"invalid_grant","error_description":"{description}","error_codes":[65001]}}"#))
    };
    let server = FakeHttp::start(echo);
    let refresh_token = "re-fresh.0123456789-abcdefghijklmnopqrstuvwxyz";
    let error = refresh_for_at(&graph_endpoints_of(&server), &oauth(), refresh_token, Resource::Graph).unwrap_err();
    let shown = format!("{error} {error:?}");
    assert!(error.is_consent_needed() && shown.contains("AADSTS65001") && shown.contains("***"), "{shown}");
    assert!((0..refresh_token.len() - 8).all(|at| !shown.contains(&refresh_token[at..at + 8])), "{shown}");
}

#[test]
fn answers_in_chunks_too_long_or_not_http() {
    let refreshed = |answer: Vec<u8>| refresh_at(&endpoints_of(&FakeHttp::answering(answer)), &oauth(), "re-fresh");
    let (half, rest) = TOKENS.split_at(40);
    let chunked = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{half}\r\n{:X};note=1\r\n{rest}\r\n0\r\n\r\n",
        half.len(),
        rest.len()
    );
    assert_eq!(refreshed(chunked.into_bytes()).unwrap().access, "acc-ess.tok_en");
    // Without a length, up to the end of the connection.
    assert_eq!(refreshed(format!("HTTP/1.0 200 OK\r\n\r\n{TOKENS}").into_bytes()).unwrap().refresh, "re-fresh.tok_en");

    let padded = format!(r#"{{"access_token":"acc-ess","padding":"{}"}}"#, "x".repeat(http::MAX_BODY));
    assert!(protocol(refreshed(ok(&padded))).contains("longer than 64 KB"));
    let endless = format!("HTTP/1.1 200 OK\r\n\r\n{padded}");
    assert!(protocol(refreshed(endless.into_bytes())).contains("longer than 64 KB"));

    for garbage in [
        "SSH-2.0-OpenSSH_9.9\r\n".to_string(),
        "* OK IMAP4rev1 ready\r\n".to_string(),
        "\u{0}\u{1}\u{2}\u{ff}\r\n\r\n".to_string(),
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nnot a size\r\n".to_string(),
        format!("HTTP/1.1 200 OK\r\nContent-Length: nine\r\n\r\n{TOKENS}"),
    ] {
        let why = protocol(refreshed(garbage.clone().into_bytes()));
        assert!(!why.is_empty() && why.len() < 100, "{garbage:?}: {why}");
    }
    // An answer that isn't what a token endpoint says.
    assert_eq!(protocol(refreshed(response("500 Internal Server Error", "<html>oops</html>"))), "the sign-in server answered with status 500");
    assert_eq!(protocol(refreshed(response("302 Found", ""))), "the sign-in server answered with status 302");
    assert_eq!(protocol(refreshed(ok("<html>hello</html>"))), "the sign-in server's answer isn't the JSON it should be");
    assert_eq!(protocol(refreshed(ok("[1, 2]"))), "the sign-in server's answer isn't the JSON it should be");
    assert_eq!(protocol(refreshed(ok(&TOKENS[..60]))), "the sign-in server's answer isn't the JSON it should be");
    assert_eq!(protocol(refreshed(ok(r#"{"token_type":"Bearer","expires_in":3600}"#))), "the sign-in server's answer has no access token");
    assert_eq!(protocol(refreshed(ok(r#"{"access_token":42}"#))), "the sign-in server's answer has no access token");
    assert_eq!(
        protocol(refreshed(ok(r#"{"token_type":"mac","access_token":"acc-ess"}"#))),
        "the sign-in server sent a kind of token that mail servers don't take"
    );
    for unusable in [r#"{"access_token":"acc\r\ness"}"#, r#"{"access_token":"acc\u0001ess"}"#, r#"{"access_token":"a","refresh_token":"re\nfresh"}"#]
    {
        assert_eq!(protocol(refreshed(ok(unusable))), "the sign-in server sent a token that can't be used", "{unusable}");
    }
    // A server that hangs up, at once or half-way.
    for cut in ["", "HTTP/1.1 200 OK\r\nContent-Length: 500\r\n\r\n{\"access_token\":"] {
        let result = refreshed(cut.as_bytes().to_vec());
        assert_eq!(result, Err(NetError::Proto(ProtoError::Io("the server closed the connection".into()))), "{cut:?}");
    }
}

#[test]
fn a_server_that_is_not_there_is_a_connection_error() {
    // A port that was just free: nothing listens there.
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let endpoints = Endpoints {
        authorize: format!("http://127.0.0.1:{port}/a"),
        token: format!("http://127.0.0.1:{port}/t"),
        scope: "mail".into(),
        ..Default::default()
    };
    assert!(matches!(refresh_at(&endpoints, &oauth(), "re-fresh"), Err(NetError::Connect { .. })));
    // For Graph's tokens as for mail's: neither a refusal nor a lack of consent.
    let unreachable = refresh_for_at(&endpoints, &oauth(), "re-fresh", Resource::Graph).unwrap_err();
    assert!(matches!(unreachable, NetError::Connect { .. }) && !unreachable.is_refused_sign_in() && !unreachable.is_consent_needed());
    // One that doesn't speak TLS where TLS is asked for.
    let server = FakeHttp::answering(ok(TOKENS));
    let endpoints = Endpoints { token: format!("https://localhost:{}/token", server.port), ..endpoints_of(&server) };
    assert!(matches!(refresh_at(&endpoints, &oauth(), "re-fresh"), Err(NetError::Tls { .. })));
}

#[test]
fn settings_that_cannot_work_are_refused_before_anything_happens() {
    let server = FakeHttp::answering(ok(TOKENS));
    let (open, browser) = browser_asking(&["GET /?code=the-code&state={state}"]);
    let cancel = AtomicBool::new(false);
    let config = |result: Result<Tokens, NetError>| match result {
        Err(NetError::Config(why)) => why,
        other => panic!("expected the settings to be refused, got {other:?}"),
    };

    // No application id: the host fills its own in before it calls.
    for id in ["", "  "] {
        let nameless = OAuth { client_id: id.into(), ..oauth() };
        let why = config(sign_in(&nameless, "ada@example.org", &open, &cancel));
        assert_eq!(why, "an application (client) id is needed to sign in with Microsoft");
        assert_eq!(config(refresh(&nameless, "re-fresh")), why);
        assert_eq!(config(sign_in_at(&endpoints_of(&server), &nameless, "", &open, &cancel)), why);
        assert_eq!(config(refresh_at(&endpoints_of(&server), &nameless, "re-fresh")), why);
    }
    let odd = OAuth { client_id: "two words\r\n here".into(), ..oauth() };
    assert!(config(refresh_at(&endpoints_of(&server), &odd, "re-fresh")).contains("can't be an application (client) id"));

    // A tenant that would lead somewhere else.
    for tenant in ["../../evil", "common/../x", "a b", "evil.example.org/x?", "ten#ant", "é", ".", "-a", "a..b", "a\r\nb", &"a".repeat(300)] {
        let astray = OAuth { tenant: tenant.into(), ..oauth() };
        let why = config(sign_in(&astray, "", &open, &cancel));
        assert!(why.contains("can't be a Microsoft tenant") && !why.contains('\r') && why.len() < 200, "{tenant:?}: {why}");
        assert_eq!(config(refresh(&astray, "re-fresh")), why);
        // Even when the endpoints come from elsewhere.
        assert_eq!(config(sign_in_at(&endpoints_of(&server), &astray, "", &open, &cancel)), why);
        assert_eq!(config(refresh_at(&endpoints_of(&server), &astray, "re-fresh")), why);
    }

    // Endpoints that aren't encrypted, on another machine.
    for (authorize, token) in [
        ("http://login.example.org/authorize", "https://login.example.org/token"),
        ("https://login.example.org/authorize", "http://login.example.org/token"),
        ("https://login.example.org/authorize", "http://192.0.2.7/token"),
        ("https://login.example.org/authorize", "login.example.org/token"),
        ("javascript:alert(1)", "https://login.example.org/token"),
        ("", ""),
    ] {
        let endpoints =
            Endpoints { authorize: authorize.into(), token: token.into(), scope: "mail".into(), consent: "more".into(), lesser: String::new() };
        let why = config(sign_in_at(&endpoints, &oauth(), "", &open, &cancel));
        assert!(why.starts_with("the server's address "), "{why}");
        if !Url::parse(token).is_ok_and(|url| url.secure) {
            assert_eq!(config(refresh_at(&endpoints, &oauth(), "re-fresh")), why);
            assert_eq!(config(refresh_for_at(&endpoints, &oauth(), "re-fresh", Resource::Graph)), why);
        }
    }
    // For Graph's tokens the settings are checked just the same.
    let nameless = OAuth { client_id: String::new(), ..oauth() };
    assert!(config(sign_in_for(&nameless, "", &open, &cancel, Resource::Graph)).contains("application (client) id"));
    assert!(config(refresh_for(&nameless, "re-fresh", Resource::Graph)).contains("application (client) id"));
    let astray = OAuth { tenant: "../../evil".into(), ..oauth() };
    assert!(config(refresh_for(&astray, "re-fresh", Resource::Graph)).contains("can't be a Microsoft tenant"));
    assert!(config(sign_in_for(&astray, "", &open, &cancel, Resource::Graph)).contains("can't be a Microsoft tenant"));

    // Nobody was sent anywhere, and nobody was asked anything.
    assert_eq!(browser.lock().unwrap().url, "");
    assert_eq!(server.asked(), 0);
}

#[test]
fn no_secret_shows_in_an_error() {
    // A careless server: it repeats the form it was sent, far enough into a long answer that
    // the cut for the error message falls inside it.
    let echo = |_: usize, request: &str| {
        let body = request.split_once("\r\n\r\n").unwrap().1;
        let description = format!("{} was sent: {body}", "no good ".repeat(18));
        response("400 Bad Request", &format!(r#"{{"error":"invalid_grant with {body}","error_description":"{description}"}}"#))
    };
    let server = FakeHttp::start(echo);
    let (open, _) = browser_asking(&["GET /?code=the-secret-code-0123456789-abcdefghijklmnopqrstuvwxyz&state={state}"]);
    let error = sign_in_to(&endpoints_of(&server), "", &open).unwrap_err();
    let shown = format!("{error} {error:?}");
    assert!(matches!(error, NetError::Proto(ProtoError::Auth(_))), "{shown}");
    let form = form_of(&server, 0);
    let (code, verifier) = (form["code"].as_str(), form["code_verifier"].as_str());
    assert!(server.request(0).contains(code) && verifier.len() == 43);
    assert!(shown.contains("***"), "{shown}");
    for secret in [code, verifier] {
        // Not whole, and not a recognisable piece of it.
        assert!((0..secret.len() - 8).all(|at| !shown.contains(&secret[at..at + 8])), "{secret}: {shown}");
    }

    let refresh_token = "re-fresh.0123456789-abcdefghijklmnopqrstuvwxyz";
    let error = refresh_at(&endpoints_of(&server), &oauth(), refresh_token).unwrap_err();
    let shown = format!("{error} {error:?}");
    assert!(matches!(error, NetError::Proto(ProtoError::Auth(_))), "{shown}");
    assert!(shown.contains("***"), "{shown}");
    assert!((0..refresh_token.len() - 8).all(|at| !shown.contains(&refresh_token[at..at + 8])), "{shown}");

    // The tokens that did arrive don't show in what is printed of them.
    let server = FakeHttp::answering(ok(TOKENS));
    let tokens = refresh_at(&endpoints_of(&server), &oauth(), refresh_token).unwrap();
    let shown = format!("{tokens:?}");
    assert!(!shown.contains("acc-ess") && !shown.contains("re-fresh"), "{shown}");
}

#[test]
fn how_long_a_token_lasts() {
    let lasting = |expires_in: &str| {
        let body = format!(r#"{{"access_token":"acc-ess"{expires_in}}}"#);
        tokens_from(&Answer { status: 200, body: body.into_bytes() }, "kept", 1_000, &[]).unwrap()
    };
    assert_eq!(
        lasting(r#","expires_in":3599"#),
        Tokens { access: "acc-ess".into(), refresh: "kept".into(), expires_at: 4_599, ..Default::default() }
    );
    assert_eq!(lasting(r#","expires_in":" 3600 ""#).expires_at, 4_600);
    assert_eq!(lasting(r#","expires_in":90.9"#).expires_at, 1_090);
    // Not said, or not said in a way that can be read: an hour, as is usual.
    for unsaid in ["", r#","expires_in":null"#, r#","expires_in":"soon""#, r#","expires_in":[3600]"#] {
        assert_eq!(lasting(unsaid).expires_at, 1_000 + DEFAULT_LIFETIME, "{unsaid}");
    }
    // Already over, and longer than any token lasts.
    for over in [r#","expires_in":0"#, r#","expires_in":-5"#, r#","expires_in":"-99999999999999""#, r#","expires_in":-1e300"#] {
        assert_eq!(lasting(over).expires_at, 1_000, "{over}");
    }
    for forever in [r#","expires_in":99999999999"#, r#","expires_in":1e300"#, r#","expires_in":18446744073709551615"#] {
        assert_eq!(lasting(forever).expires_at, 1_000 + MAX_LIFETIME, "{forever}");
    }
    // A clock at the end of time doesn't wrap around.
    let answer = Answer { status: 200, body: br#"{"access_token":"acc-ess","expires_in":3600}"#.to_vec() };
    assert_eq!(tokens_from(&answer, "", i64::MAX, &[]).unwrap().expires_at, i64::MAX);
}

/// An identity token with these claims: a header, the claims and a signature, each in base64.
fn id_token(claims: &str) -> String {
    format!("{}.{}.c2lnbmF0dXJl", URL_SAFE_NO_PAD.encode(r#"{"typ":"JWT","alg":"RS256"}"#), URL_SAFE_NO_PAD.encode(claims))
}

/// The name and the address read from an identity token.
fn who(id_token: &str) -> (String, String) {
    let tokens = identity(id_token);
    // Nothing else is touched.
    assert_eq!(Tokens { name: String::new(), email: String::new(), ..tokens.clone() }, Tokens::default());
    (tokens.name, tokens.email)
}

fn named(name: &str, email: &str) -> (String, String) {
    (name.to_string(), email.to_string())
}

#[test]
fn who_signed_in_is_read_from_the_identity_token() {
    let ada = id_token(
        r#"{"aud":"client-1","iss":"https://login.example.org/v2.0","name":"Ada Lovelace","email":"ada@example.org","preferred_username":"a.lovelace@example.org","sub":"123"}"#,
    );
    assert_eq!(who(&ada), named("Ada Lovelace", "ada@example.org"));
    // Without `email`, the name the owner signs in with, when that is an address.
    assert_eq!(
        who(&id_token(r#"{"name":"Ada Lovelace","preferred_username":" a.lovelace@example.org "}"#)),
        named("Ada Lovelace", "a.lovelace@example.org")
    );
    assert_eq!(who(&id_token(r#"{"name":"Ada Lovelace","email":"","preferred_username":"+44 20 7946 0000"}"#)), named("Ada Lovelace", ""));
    assert_eq!(who(&id_token(r#"{"email":"not an address","preferred_username":"ada@example.org"}"#)), named("", "ada@example.org"));
    assert_eq!(who(&id_token(r#"{"sub":"123"}"#)), named("", ""));
    // Padded, as base64 elsewhere is.
    let padded = format!("e30=.{}=.", mailcraft_charset::base64::URL_SAFE.encode(r#"{"name":"Ada"}"#).trim_end_matches('='));
    assert_eq!(who(&padded), named("Ada", ""));

    // With the tokens of a sign-in, and of a refresh only when it brings one.
    let answer = format!(r#"{{"token_type":"Bearer","expires_in":3600,"access_token":"acc-ess","refresh_token":"re-fresh","id_token":"{ada}"}}"#);
    let server = FakeHttp::start(move |n, _| if n < 2 { ok(&answer) } else { ok(TOKENS) });
    let (open, _) = browser_asking(&["GET /?code=the-code&state={state}"]);
    let tokens = sign_in_to(&endpoints_of(&server), "ada@example.org", &open).unwrap();
    assert_eq!((tokens.name.as_str(), tokens.email.as_str(), tokens.access.as_str()), ("Ada Lovelace", "ada@example.org", "acc-ess"));
    let tokens = refresh_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert_eq!((tokens.name.as_str(), tokens.email.as_str()), ("Ada Lovelace", "ada@example.org"));
    let tokens = refresh_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert_eq!((tokens.name.as_str(), tokens.email.as_str(), tokens.access.as_str()), ("", "", "acc-ess.tok_en"));
}

#[test]
fn a_hostile_identity_token_leaves_the_name_empty() {
    for junk in [
        String::new(),
        ".".into(),
        "..".into(),
        "...".into(),
        "not a token".into(),
        "e30.!!!not base64!!!.c2ln".into(),
        "e30.e30".into(),
        format!("{}.extra", id_token(r#"{"name":"Ada"}"#)),
        // Base64 of something else than a JSON object.
        format!("e30.{}.c2ln", URL_SAFE_NO_PAD.encode("Ada Lovelace")),
        format!("e30.{}.c2ln", URL_SAFE_NO_PAD.encode([0xFF, 0xFE, 0x00])),
        id_token("[\"name\",\"Ada\"]"),
        id_token("\"name\""),
        id_token(r#"{"name":"Ada""#),
        // Values that aren't text.
        id_token(r#"{"name":42,"email":["ada@example.org"],"preferred_username":{"value":"ada@example.org"}}"#),
        id_token(r#"{"name":null,"email":true}"#),
        id_token(&format!(r#"{{"name":{}1{}}}"#, "[".repeat(1_000), "]".repeat(1_000))),
        // A token larger than any is.
        id_token(&format!(r#"{{"name":"Ada","padding":"{}"}}"#, "x".repeat(MAX_TOKEN))),
    ] {
        assert_eq!(who(&junk), named("", ""), "{junk:.80}");
    }
    // Text that is there is cut to size and to one line, and an address is one or it is left out.
    let long = who(&id_token(&format!(r#"{{"name":"{}","email":"{}@example.org"}}"#, "é".repeat(1_000), "a".repeat(300))));
    assert_eq!((long.0.chars().count(), long.1.as_str()), (MAX_IDENTITY, ""));
    let odd = id_token(r#"{"name":"  Ada\r\n\tLovelace\u0000\u001b[31m \u202egro.elpmaxe  ","email":"ada@example.org\r\nBcc: eve@example.org"}"#);
    assert_eq!(who(&odd), named("Ada Lovelace [31m gro.elpmaxe", ""));
    for not_an_address in ["ada", "@example.org", "ada@", "a@b@c", "ada lovelace@example.org", "ada@exam\u{202e}ple.org", "ada@example.org\u{0}"] {
        let claims = serde_json::json!({ "email": not_an_address, "preferred_username": not_an_address }).to_string();
        assert_eq!(who(&id_token(&claims)), named("", ""), "{not_an_address:?}");
    }
    // A bad identity token doesn't cost the sign-in its tokens.
    let answer = Answer { status: 200, body: br#"{"access_token":"acc-ess","id_token":"a.b.c"}"#.to_vec() };
    let tokens = tokens_from(&answer, "kept", 1_000, &[]).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str(), tokens.name.as_str(), tokens.email.as_str()), ("acc-ess", "kept", "", ""));
    let answer = Answer { status: 200, body: br#"{"access_token":"acc-ess","id_token":42}"#.to_vec() };
    assert!(tokens_from(&answer, "", 1_000, &[]).is_ok());
}

#[test]
fn requests_are_judged_by_their_first_line() {
    let judged = |request: &str| {
        let (page, outcome) = judge(request.as_bytes(), "the-state");
        (page.status, outcome)
    };
    assert_eq!(judged("GET /?code=abc&state=the-state HTTP/1.1\r\nHost: localhost\r\n\r\n"), ("200 OK", Some(Ok("abc".into()))));
    assert_eq!(judged("GET /?state=the-state&code=a%20b+c HTTP/1.1\n\n"), ("200 OK", Some(Ok("a b c".into()))));
    // Headers are not where a code comes from.
    assert_eq!(judged("GET / HTTP/1.1\r\nX-Query: ?code=abc&state=the-state\r\n\r\n"), ("400 Bad Request", None));
    for (request, status) in [
        ("GET /?code=abc&state=the-stat HTTP/1.1\r\n\r\n", "400 Bad Request"),
        ("GET /?code=abc&state=the-state-and-more HTTP/1.1\r\n\r\n", "400 Bad Request"),
        ("GET /?code=abc&state=THE-STATE HTTP/1.1\r\n\r\n", "400 Bad Request"),
        ("GET /?code=abc&state= HTTP/1.1\r\n\r\n", "400 Bad Request"),
        ("GET /?code=abc HTTP/1.1\r\n\r\n", "400 Bad Request"),
        ("GET //?code=abc&state=the-state HTTP/1.1\r\n\r\n", "404 Not Found"),
        ("GET http://localhost/?code=abc&state=the-state HTTP/1.1\r\n\r\n", "404 Not Found"),
        ("GET /favicon.ico HTTP/1.1\r\n\r\n", "404 Not Found"),
        ("GET  HTTP/1.1\r\n\r\n", "404 Not Found"),
        ("get /?code=abc&state=the-state HTTP/1.1\r\n\r\n", "405 Method Not Allowed"),
        ("HEAD /?code=abc&state=the-state HTTP/1.1\r\n\r\n", "405 Method Not Allowed"),
        ("OPTIONS * HTTP/1.1\r\n\r\n", "405 Method Not Allowed"),
        ("\r\n\r\n", "405 Method Not Allowed"),
        ("", "405 Method Not Allowed"),
    ] {
        assert_eq!(judged(request), (status, None), "{request:?}");
    }
    // A sign-in that waits for no state at all believes nobody.
    assert!(judge(b"GET /?code=abc&state= HTTP/1.1\r\n\r\n", "").1.is_none());
    assert!(judge(b"GET /?code=abc HTTP/1.1\r\n\r\n", "").1.is_none());
    // A code that couldn't be sent on is as good as none.
    let (status, outcome) = judged("GET /?code=a%0D%0Ab&state=the-state HTTP/1.1\r\n\r\n");
    let no_code = |outcome: &Option<Result<String, Refused>>| {
        matches!(outcome, Some(Err(Refused { error: NetError::Proto(ProtoError::Protocol(_)), about: About::Other, .. })))
    };
    assert!(status == "200 OK" && no_code(&outcome), "{outcome:?}");
    let long = format!("GET /?code={}&state=the-state HTTP/1.1\r\n\r\n", "c".repeat(MAX_TOKEN + 1));
    assert!(no_code(&judged(&long).1));
    // A refusal comes with what it is about.
    let refused = |query: &str| match judged(&format!("GET /?state=the-state&{query} HTTP/1.1\r\n\r\n")) {
        ("200 OK", Some(Err(Refused { error: NetError::Proto(_), about, .. }))) => about,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(refused("error=invalid_scope"), About::Scope);
    assert_eq!(refused("error=invalid_request&error_description=AADSTS28000%3A+More+than+one+resource."), About::Scope);
    assert_eq!(refused("error=access_denied&error_description=AADSTS65004%3A+User+declined+to+consent."), About::Consent);
    assert_eq!(refused("error=access_denied&error_subcode=cancel"), About::Other);
    assert_eq!(refused("error=server_error"), About::Other);
}

#[test]
fn what_a_no_is_about() {
    // The error code says it, or one of Microsoft's own numbers does: listed, or in the description.
    for (error, suberror, description, codes, expected) in [
        ("invalid_scope", "", "", &[][..], About::Scope),
        (" Invalid_Resource ", "", "", &[][..], About::Scope),
        ("invalid_request", "", "AADSTS70011: The provided value for the input parameter 'scope' is not valid.", &[][..], About::Scope),
        ("invalid_grant", "", "", &[28_000][..], About::Scope),
        ("invalid_client", "", "AADSTS650053: The application 'MailCraft' asked for scope 'Mail.Send' that doesn't exist.", &[][..], About::Scope),
        ("invalid_request", "", "Twice: AADSTS90002 and then AADSTS650057: Invalid resource.", &[][..], About::Scope),
        ("consent_required", "", "", &[][..], About::Consent),
        ("interaction_required", "", "AADSTS50076: You must use multi-factor authentication.", &[50_076][..], About::Consent),
        ("invalid_grant", "Consent_Required", "", &[][..], About::Consent),
        ("invalid_grant", "", "AADSTS65001: The user or administrator has not consented.", &[][..], About::Consent),
        ("invalid_grant", "", "", &[70_000, 65_001][..], About::Consent),
        ("access_denied", "", "AADSTS90094: An administrator has to agree.", &[][..], About::Consent),
        // A refresh token that is no longer good, a wrong code, a server in trouble: none of the above.
        ("invalid_grant", "", "AADSTS70008: The refresh token has expired due to inactivity.", &[70_008][..], About::Other),
        ("invalid_grant", "bad_token", "AADSTS50173: The provided grant has expired due to it being revoked.", &[50_173][..], About::Other),
        ("invalid_client", "", "", &[][..], About::Other),
        ("temporarily_unavailable", "", "", &[][..], About::Other),
        ("", "", "", &[][..], About::Other),
        // A number is read whole, and only where it is one of Microsoft's.
        ("invalid_grant", "", "AADSTS650010: Something else.", &[][..], About::Other),
        ("invalid_grant", "", "AADSTS700110: Something else. AADSTS: none. AADSTS", &[][..], About::Other),
        ("invalid_grant", "", "Error 65001, or aadsts65001, or STS65001.", &[][..], About::Other),
        ("invalid_grant", "", "AADSTS99999999999999999999999999: no number at all.", &[650_010][..], About::Other),
    ] {
        assert_eq!(about(error, suberror, description, codes), expected, "{error} {suberror} {description} {codes:?}");
    }
    // What was asked for comes before who has to agree: without the one, the other can't be asked.
    assert_eq!(about("consent_required", "", "AADSTS70011: no such scope.", &[]), About::Scope);
    assert_eq!(aadsts("AADSTS65001: x. AADSTS7: y. AADSTSx AADSTS").collect::<Vec<_>>(), [65_001, 7]);
    assert_eq!(aadsts(&"AADSTS1 ".repeat(100)).count(), MAX_CODES);
}

#[test]
fn pages_are_whole_answers() {
    for page in [&DONE, &FAILED, &NOT_OURS, &NOT_FOUND, &NOT_ALLOWED, &TOO_LARGE, &APPROVED, &NOT_APPROVED] {
        let response = String::from_utf8(page.response()).unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with(&format!("HTTP/1.1 {}\r\n", page.status)), "{head}");
        assert!(head.contains(&format!("\r\nContent-Length: {}\r\n", body.len())) && head.ends_with("Connection: close"), "{head}");
        assert!(head.contains("Content-Security-Policy: default-src 'none'") && head.contains("Cache-Control: no-store"), "{head}");
        assert!(body.starts_with("<!doctype html>") && body.contains(page.title) && body.contains(page.text), "{body}");
        assert!(body.contains("MailCraft") && body.len() < 1_000, "{body}");
    }
}

proptest! {
    #[test]
    fn arbitrary_requests_never_panic_or_pass(request in proptest::collection::vec(any::<u8>(), 0..400)) {
        prop_assert!(judge(&request, "the-state").1.is_none());
        let mut prefixed = b"GET /?state=the-state&".to_vec();
        prefixed.extend_from_slice(&request);
        let _ = judge(&prefixed, "the-state");
    }

    #[test]
    fn request_shaped_noise_is_only_believed_with_the_state(
        method in "(GET|POST|get|)",
        path in "(/|/x|//|)",
        query in proptest::collection::vec(("(code|state|error|error_description|x)", "(the-state|abc|%0D%0A|\\PC{0,6}|)"), 0..6),
    ) {
        let query: Vec<String> = query.iter().map(|(name, value)| format!("{name}={}", http::encode(value))).collect();
        let request = format!("{method} {path}?{} HTTP/1.1\r\n\r\n", query.join("&"));
        let first_state = query.iter().find_map(|pair| pair.strip_prefix("state="));
        if let (_, Some(_)) = judge(request.as_bytes(), "the-state") {
            prop_assert_eq!((method.as_str(), path.as_str(), first_state), ("GET", "/", Some("the-state")));
        }
    }

    #[test]
    fn arbitrary_token_answers_never_panic(status in 0u16..700, body in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = tokens_from(&Answer { status, body }, "kept", 1_000, &["kept"]);
    }

    #[test]
    fn token_shaped_answers_give_usable_tokens_or_none(
        status in prop_oneof![Just(200u16), Just(400u16)],
        members in proptest::collection::vec(
            (
                "(access_token|refresh_token|expires_in|token_type|error|error_description|other)",
                prop_oneof![
                    Just("\"Bearer\"".to_string()),
                    Just("null".to_string()),
                    "-?[0-9]{1,25}(\\.[0-9]{1,3})?",
                    "\"[a-zA-Z0-9._ -]{0,12}\"",
                    Just("\"line\\r\\nbreak\"".to_string()),
                    Just("[1,{\"a\":2}]".to_string()),
                ],
            ),
            0..6,
        ),
    ) {
        let members: Vec<String> = members.iter().map(|(name, value)| format!("\"{name}\":{value}")).collect();
        let answer = Answer { status, body: format!("{{{}}}", members.join(",")).into_bytes() };
        match tokens_from(&answer, "kept", 1_000, &[]) {
            Ok(tokens) => {
                prop_assert_eq!(status, 200);
                prop_assert!(!tokens.access.is_empty() && usable(&tokens.access) && usable(&tokens.refresh));
                prop_assert!((1_000..=1_000 + MAX_LIFETIME).contains(&tokens.expires_at));
            }
            Err(refused) => {
                let text = refused.error.to_string();
                prop_assert!(!text.contains('\r') && !text.contains('\n') && text.chars().count() < 400, "{}", text);
            }
        }
    }

    #[test]
    fn arbitrary_identity_tokens_give_a_clean_name_or_none(token in "[A-Za-z0-9_=.-]{0,200}", text in any::<String>()) {
        let claims = serde_json::json!({ "name": text, "email": text, "preferred_username": format!("{text}@example.org") }).to_string();
        for token in [token, text.clone(), id_token(&text), id_token(&claims)] {
            let (name, email) = who(&token);
            prop_assert!(name.chars().count() <= MAX_IDENTITY && !name.chars().any(unprintable), "{:?}", name);
            prop_assert!(email.len() <= MAX_IDENTITY && !email.chars().any(|c| unprintable(c) || c.is_whitespace()), "{:?}", email);
            prop_assert!(email.is_empty() || email.contains('@'));
        }
    }

    #[test]
    fn any_hint_and_scope_make_an_address_that_reads_back(hint in "\\PC{0,40}", scope in "[ -~]{0,40}", state in "[A-Za-z0-9_-]{32}") {
        let endpoints = Endpoints { authorize: "https://login.example.org/authorize".into(), scope: scope.clone(), ..Default::default() };
        let url = authorize_url(&endpoints, "", "client-1", "http://localhost:49152/", &state, "the-challenge", &hint);
        let (address, query) = url.split_once('?').unwrap();
        prop_assert_eq!(address, "https://login.example.org/authorize");
        prop_assert!(query.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~%=&".contains(&b)), "{}", query);
        let asked = fields(query);
        prop_assert_eq!(&asked["scope"], &scope);
        prop_assert_eq!(&asked["state"], &state);
        prop_assert_eq!(&asked["redirect_uri"], "http://localhost:49152/");
        prop_assert_eq!(asked.get("login_hint").map(String::as_str), Some(hint.trim()).filter(|hint| !hint.is_empty()));
        // What else the page is asked for follows the tokens' own scopes, and nothing else changes.
        let more = authorize_url(&endpoints, " graph.send  ", "client-1", "http://localhost:49152/", &state, "the-challenge", &hint);
        let asked_more = fields(more.split_once('?').unwrap().1);
        prop_assert_eq!(&asked_more["scope"], &format!("{} graph.send", scope.trim()));
        prop_assert_eq!(asked_more.len(), asked.len());
    }

    #[test]
    fn what_any_no_is_about_never_panics(
        error in "\\PC{0,20}",
        suberror in "\\PC{0,20}",
        description in "(AADSTS|aadsts|A|[0-9]{1,30}|\\PC){0,30}",
        codes in proptest::collection::vec(any::<u64>(), 0..40),
    ) {
        let _ = about(&error, &suberror, &description, &codes);
        prop_assert!(aadsts(&description).count() <= MAX_CODES);
    }

    #[test]
    fn a_number_names_what_a_no_is_about_only_when_read_whole(
        n in prop_oneof![0u64..1_000_000, proptest::sample::select(SCOPE_CODES.to_vec()), proptest::sample::select(CONSENT_CODES.to_vec())],
        tail in "[ :.a-z]{0,6}",
        longer in 0u64..10,
    ) {
        let expected = |n: u64| if SCOPE_CODES.contains(&n) { About::Scope } else if CONSENT_CODES.contains(&n) { About::Consent } else { About::Other };
        let description = format!("The server said AADSTS{n}{tail}");
        prop_assert_eq!(aadsts(&description).collect::<Vec<_>>(), vec![n]);
        prop_assert_eq!(about("invalid_grant", "", &description, &[]), expected(n));
        prop_assert_eq!(about("invalid_grant", "", "", &[n]), expected(n));
        // With a digit more it is another number.
        let other = n * 10 + longer;
        prop_assert_eq!(about("invalid_grant", "", &format!("AADSTS{other}{tail}"), &[]), expected(other));
    }
}

#[test]
fn the_link_for_an_administrator_names_the_application_and_the_permissions_and_nothing_else() {
    let settings = OAuth { client_id: "0f6c8d2a-1b3e-4c5d-9e7f-a1b2c3d4e5f6".into(), ..oauth() };
    let parts = |link: &str| -> (String, HashMap<String, String>) {
        let (address, query) = link.split_once('?').unwrap();
        (address.to_string(), fields(query))
    };
    // Everything the program asks an owner for, so that one approval covers what is asked
    // later: the mail servers, Microsoft Graph with the Favorites' two, and who signs in.
    let link = admin_consent_url(&settings, "").unwrap();
    let (address, asked) = parts(&link);
    assert_eq!(address, "https://login.microsoftonline.com/organizations/v2.0/adminconsent");
    assert_eq!(asked.len(), 3, "{asked:?}");
    assert_eq!(asked["client_id"], "0f6c8d2a-1b3e-4c5d-9e7f-a1b2c3d4e5f6");
    assert_eq!(asked["scope"], ADMIN_SCOPES);
    assert_eq!(asked["redirect_uri"], "http://localhost");
    // Nothing of outlook.office365.com is asked for: the mail servers are outlook.office.com.
    assert!(!ADMIN_SCOPES.contains("office365"));
    // The organisation's own directory when it is known, by its id or by a domain of its own.
    for tenant in ["3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b", "lumen.example", " Lumen.Example "] {
        let (address, _) = parts(&admin_consent_url(&settings, tenant).unwrap());
        assert_eq!(address, format!("https://login.microsoftonline.com/{}/v2.0/adminconsent", tenant.trim()));
    }
    // No administrator is to be found in "any directory" or among personal accounts: the
    // administrator's own sign-in says which organisation it is then.
    for tenant in ["common", "consumers", "organizations", mailcraft_model::Directory::PERSONAL_TENANT, ""] {
        let (address, _) = parts(&admin_consent_url(&settings, tenant).unwrap());
        assert_eq!(address, "https://login.microsoftonline.com/organizations/v2.0/adminconsent", "{tenant}");
    }
    // What can't be part of an address isn't put into one.
    for tenant in ["lumen.example/evil", "a b", "x?y=z", "../common", "lumen.example#frag"] {
        assert!(matches!(admin_consent_url(&settings, tenant), Err(NetError::Config(_))), "{tenant}");
    }
    assert!(matches!(admin_consent_url(&OAuth { client_id: String::new(), ..oauth() }, ""), Err(NetError::Config(_))));
    // Nothing of a sign-in is in it: no state, no hint of who asked, no token.
    for never in ["state", "login_hint", "code", "token", "@"] {
        assert!(!link.contains(never), "{never}: {link}");
    }
}

#[test]
fn the_identity_token_says_which_directory_signed_in() {
    use mailcraft_model::Directory;
    let work = identity(&id_token(r#"{"name":"Ada Lovelace","email":"ada@lumen.example","tid":"3A1C5E7B-9d2f-4a6c-8e0b-1d3f5a7c9e2b"}"#));
    assert_eq!((work.tenant.as_str(), Directory::of_tenant(&work.tenant)), ("3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b", Directory::Organisation));
    let personal = identity(&id_token(r#"{"email":"ada@outlook.example","tid":"9188040d-6c67-4c5b-b112-36a304b66dad"}"#));
    assert_eq!(Directory::of_tenant(&personal.tenant), Directory::Personal);
    // Anything that is not shaped like a directory's id says nothing.
    for tid in [
        r#""""#,
        r#""common""#,
        "12",
        "null",
        r#"["3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b"]"#,
        r#""3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b/../x""#,
        r#""3a1c5e7b 9d2f 4a6c 8e0b 1d3f5a7c9e2b""#,
    ] {
        let tokens = identity(&id_token(&format!(r#"{{"name":"Ada","tid":{tid}}}"#)));
        assert_eq!((tokens.tenant.as_str(), Directory::of_tenant(&tokens.tenant)), ("", Directory::Unknown), "{tid}");
    }
    // The settings of an account say it too, when they name one.
    assert_eq!(
        ["", "common", "consumers", "organizations", "lumen.example"].map(Directory::of_tenant),
        [Directory::Unknown, Directory::Unknown, Directory::Personal, Directory::Organisation, Directory::Organisation]
    );
}

// ---- an administrator's approval ----

/// What an administrator is asked to approve: everything the program asks an owner for, and what
/// later shared-mailbox slices will ask for.
const ADMIN_SCOPES: &str = "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/MailboxConfigItem.Read https://graph.microsoft.com/Mail.ReadBasic https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared https://graph.microsoft.com/Tasks.ReadWrite offline_access openid profile email";
/// The same, service by service.
const ADMIN_SCOPES_MAIL: &str =
    "https://outlook.office.com/IMAP.AccessAsUser.All https://outlook.office.com/SMTP.Send offline_access openid profile email";
const ADMIN_SCOPES_GRAPH: &str = "https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Calendars.ReadWrite https://graph.microsoft.com/Contacts.Read https://graph.microsoft.com/MailboxSettings.Read https://graph.microsoft.com/Mail.ReadWrite https://graph.microsoft.com/MailboxConfigItem.Read https://graph.microsoft.com/Mail.ReadBasic https://graph.microsoft.com/Mail.ReadWrite.Shared https://graph.microsoft.com/Mail.Send.Shared https://graph.microsoft.com/Calendars.ReadWrite.Shared https://graph.microsoft.com/Tasks.ReadWrite offline_access openid profile email";
/// A directory's id and an application's, both invented.
const DIRECTORY: &str = "3a1c5e7b-9d2f-4a6c-8e0b-1d3f5a7c9e2b";
const APPLICATION: &str = "0f6c8d2a-1b3e-4c5d-9e7f-a1b2c3d4e5f6";

fn approval(tenant: &str, open: &dyn Fn(&str)) -> Result<(), AdminRefusal> {
    let settings = OAuth { client_id: APPLICATION.into(), ..oauth() };
    admin_consent_within(&settings, tenant, open, &AtomicBool::new(false), PATIENCE)
}

#[test]
fn one_approval_covers_everything_the_program_asks_an_owner_for() {
    let approved: Vec<String> = admin_scope().split(' ').map(str::to_string).collect();
    assert_eq!(approved.join(" "), ADMIN_SCOPES);
    // What the page of a sign-in asks for, to mail and to Graph, and what the tokens are for: a
    // page that asked for one permission more would send the owner to find an administrator again.
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        let endpoints = Endpoints::for_resource(&oauth(), resource);
        for asked in format!("{} {} {}", endpoints.scope, endpoints.consent, endpoints.lesser).split_whitespace() {
            assert!(approved.iter().any(|scope| scope == asked), "{resource:?}: {asked}");
        }
    }
    // And what the Favorites take through Graph, which an owner is asked for later, and what a
    // shared mailbox takes, read and sent from (#70).
    for asked in
        format!("{GRAPH_SCOPES} {FAVORITES_SCOPES} {} {}", shared_scope(), shared_send_scope()).split(' ').chain(shared_calendar_scope().split(' '))
    {
        assert!(approved.iter().any(|scope| scope == asked), "{asked}");
    }
    // No permission twice.
    let mut seen: Vec<&str> = approved.iter().map(String::as_str).collect();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), approved.len());

    // Service by service, for a provider that won't have them on one page: each with who signs in.
    assert_eq!(apart(ADMIN_SCOPES), [ADMIN_SCOPES_MAIL, ADMIN_SCOPES_GRAPH]);
    assert_eq!(apart("offline_access openid"), Vec::<String>::new());
    assert_eq!(apart(""), Vec::<String>::new());
    assert_eq!(apart("a/x  b/y a/z who"), ["a/x a/z who", "b/y who"]);
}

#[test]
fn an_administrators_yes_comes_back_to_the_loopback_port() {
    let (open, visits) = browser_answering(&[&format!(
        "GET /?admin_consent=True&tenant={DIRECTORY}&scope=https%3A%2F%2Fgraph.microsoft.com%2FMail.Send&state={{state}}"
    )]);
    assert_eq!(approval("lumen.example", &open), Ok(()));
    // The page the administrator was sent to: Microsoft's, for the organisation's own directory,
    // asked for the application, the permissions, the way back and the state, and nothing else.
    let url = visits.lock().unwrap()[0].clone();
    let (address, query) = url.split_once('?').unwrap();
    assert_eq!(address, "https://login.microsoftonline.com/lumen.example/v2.0/adminconsent");
    let asked = visit_asked(&visits, 0);
    assert_eq!(asked.len(), 4, "{asked:?}");
    assert_eq!((asked["client_id"].as_str(), asked["scope"].as_str()), (APPLICATION, ADMIN_SCOPES));
    // As registered, on a port of this machine: no path, no trailing slash.
    assert_eq!(asked["redirect_uri"], format!("http://localhost:{}", port_of(&asked["redirect_uri"])));
    assert!(asked["state"].len() >= 32, "{}", asked["state"]);
    assert!(query.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~%=&".contains(&b)), "{query}");
    assert_eq!(visited(&visits), 1);
    // No directory that is known: the administrator's own sign-in says which organisation it is.
    let (open, visits) = browser_answering(&["GET /?admin_consent=true&state={state}"]);
    assert_eq!(approval("", &open), Ok(()));
    assert!(visits.lock().unwrap()[0].starts_with("https://login.microsoftonline.com/organizations/v2.0/adminconsent?"));

    // What the browser is shown: a page that stands on its own and repeats nothing it was sent.
    let (open, browser) = browser_asking(&[&format!("GET /?admin_consent=True&tenant={DIRECTORY}&state={{state}}")]);
    assert_eq!(approval("", &open), Ok(()));
    let page = &pages(&browser, 1)[0];
    assert!(page.starts_with("HTTP/1.1 200 OK\r\n") && page.contains("MailCraft is approved for your organisation"), "{page}");
    assert!(!page.contains(DIRECTORY) && !page.contains(&asked_of(&browser)["state"]));
}

#[test]
fn an_administrators_no_says_whose_it_is() {
    let refused = |answer: &str| {
        let (open, browser) = browser_asking(&[&format!("GET /?{answer}&state={{state}}")]);
        let outcome = approval("", &open);
        let page = pages(&browser, 1).remove(0);
        assert!(page.starts_with("HTTP/1.1 200 OK\r\n") && page.contains("The approval didn't go through"), "{page}");
        // Microsoft's words are for the window to show, never for the browser.
        assert!(!page.contains("AADSTS"), "{page}");
        outcome.unwrap_err()
    };
    // As Microsoft's documentation has a refusal: `admin_consent=True` beside the error.
    let declined = refused(
        "admin_consent=True&error=consent_required&error_description=AADSTS65004%3a+The+resource+owner+or+authorization+server+denied+the+request.%0d%0aTrace+ID%3a+0000aaaa%0d%0aTimestamp%3a+2026-10-13",
    );
    let words = "AADSTS65004: The resource owner or authorization server denied the request.";
    assert_eq!(declined, AdminRefusal::Declined(words.into()));
    assert_eq!(refused("error=access_denied"), AdminRefusal::Declined("access was refused on the sign-in page".into()));
    assert!(matches!(refused("error=access_denied"), AdminRefusal::Declined(_)));
    assert!(matches!(refused("error=access_denied&error_subcode=cancel&admin_consent=True"), AdminRefusal::Declined(_)));
    // Whoever signed in there is not an administrator: Microsoft's page told them to find one.
    for number in [90_093, 90_094, 90_095, 900_941] {
        let answer = format!("admin_consent=True&error=access_denied&error_description=AADSTS{number}%3A+An+administrator+has+to+approve.");
        assert_eq!(refused(&answer), AdminRefusal::NotAdministrator(format!("AADSTS{number}: An administrator has to approve.")), "{number}");
    }
    // Shut for another reason: said in Microsoft's words, as it is.
    let blocked = refused("error=access_denied&error_description=AADSTS53003%3A+Blocked+by+Conditional+Access.");
    assert_eq!(blocked, AdminRefusal::Other(NetError::Proto(ProtoError::Auth("AADSTS53003: Blocked by Conditional Access.".into()))));
    let later = refused("error=temporarily_unavailable");
    assert!(matches!(later, AdminRefusal::Other(NetError::Proto(ProtoError::Rejected(_)))), "{later:?}");
    // The right state and no answer at all, or one that is not a yes.
    for nothing in ["tenant=x", "admin_consent=False", "admin_consent=", "admin_consent=Truely", "code=abc"] {
        let said = refused(nothing);
        let broken = NetError::Proto(ProtoError::Protocol("the approval page came back without an answer".into()));
        assert_eq!(said, AdminRefusal::Other(broken), "{nothing}");
    }
}

#[test]
fn strangers_and_junk_neither_approve_nor_refuse() {
    let (open, browser) = browser_asking(&[
        // Without the state, or with a guess at it: neither a yes nor a no is believed.
        "GET /?admin_consent=True",
        "GET /?admin_consent=True&state=guess",
        "GET /?error=access_denied&state=guess",
        "GET /favicon.ico",
        "GET /callback?admin_consent=True&state={state}",
        "POST /?admin_consent=True&state={state}",
        "junk",
        "GET /?admin_consent=True&state={state}",
    ]);
    assert_eq!(approval("", &open), Ok(()));
    let pages = pages(&browser, 8);
    let status = |n: usize| pages[n].lines().next().unwrap_or("").to_string();
    for n in 0..3 {
        assert_eq!(status(n), "HTTP/1.1 400 Bad Request", "{n}");
    }
    assert_eq!((status(3), status(4)), ("HTTP/1.1 404 Not Found".to_string(), "HTTP/1.1 404 Not Found".to_string()));
    assert_eq!((status(5), status(6)), ("HTTP/1.1 405 Method Not Allowed".to_string(), "HTTP/1.1 405 Method Not Allowed".to_string()));
    assert_eq!(status(7), "HTTP/1.1 200 OK");

    // Judged by the first line alone, as a sign-in's answer is.
    let judged = |request: &str| {
        let (page, outcome) = judge_consent(request.as_bytes(), "the-state");
        (page.status, outcome.map(|outcome| outcome.map_err(AdminRefusal::from)))
    };
    assert_eq!(judged("GET /?admin_consent=True&state=the-state HTTP/1.1\r\n\r\n"), ("200 OK", Some(Ok(()))));
    assert_eq!(judged("GET /?state=the-state&admin_consent=TRUE HTTP/1.1\n\n"), ("200 OK", Some(Ok(()))));
    assert_eq!(judged("GET / HTTP/1.1\r\nX-Query: ?admin_consent=True&state=the-state\r\n\r\n"), ("400 Bad Request", None));
    assert_eq!(judged("GET /?admin_consent=True&state=the-stat HTTP/1.1\r\n\r\n"), ("400 Bad Request", None));
    assert!(judge_consent(b"GET /?admin_consent=True&state= HTTP/1.1\r\n\r\n", "").1.is_none());
    // A no beside a yes is a no.
    assert!(matches!(
        judged("GET /?admin_consent=True&error=consent_required&state=the-state HTTP/1.1\r\n\r\n").1,
        Some(Err(AdminRefusal::Declined(_)))
    ));
}

#[test]
fn an_approval_nobody_gives_times_out_or_is_called_off() {
    let settings = OAuth { client_id: APPLICATION.into(), ..oauth() };
    // Called off before it starts: nobody is sent anywhere.
    let opened = Mutex::new(Vec::<String>::new());
    let open = |url: &str| opened.lock().unwrap().push(url.to_string());
    assert_eq!(admin_consent(&settings, "", &open, &AtomicBool::new(true)), Err(AdminRefusal::Cancelled));
    assert!(opened.lock().unwrap().is_empty());
    // Nobody comes back: said as that, and the port is closed again.
    let started = Instant::now();
    let late = admin_consent_within(&settings, "", &open, &AtomicBool::new(false), Duration::from_millis(300));
    assert_eq!(late, Err(AdminRefusal::TimedOut));
    assert!((Duration::from_millis(300)..Duration::from_secs(10)).contains(&started.elapsed()), "{:?}", started.elapsed());
    let port = port_of(&fields(opened.lock().unwrap()[0].split_once('?').unwrap().1)["redirect_uri"]);
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
    // Called off while the page is open.
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let open = move |_: &str| {
        let flag = flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::Relaxed);
        });
    };
    assert_eq!(admin_consent_within(&settings, "", &open, &cancel, PATIENCE), Err(AdminRefusal::Cancelled));
    // Settings that can't work are refused before anybody is sent anywhere.
    let nobody = |url: &str| assert!(url.is_empty(), "nobody is to be sent anywhere");
    let free = AtomicBool::new(false);
    for tenant in ["lumen.example/evil", "a b", "x?y=z", "../common"] {
        assert!(matches!(admin_consent(&settings, tenant, &nobody, &free), Err(AdminRefusal::Other(NetError::Config(_)))), "{tenant}");
    }
    let no_id = OAuth { client_id: String::new(), ..oauth() };
    assert!(matches!(admin_consent(&no_id, "", &nobody, &free), Err(AdminRefusal::Other(NetError::Config(_)))));
}

#[test]
fn a_page_that_will_not_have_several_services_is_asked_service_by_service() {
    let together = "GET /?error=invalid_request&error_description=AADSTS28000%3A+Provided+value+for+the+input+parameter+scope+is+not+valid+because+it+contains+more+than+one+resource.&state={state}";
    let yes = "GET /?admin_consent=True&state={state}";
    // Refused for what it asked for together: a page for the mail servers, then one for Graph.
    let (open, visits) = browser_answering(&[together, yes, yes]);
    assert_eq!(approval(DIRECTORY, &open), Ok(()));
    assert_eq!(visited(&visits), 3);
    let asked: Vec<String> = (0..3).map(|n| visit_asked(&visits, n)["scope"].clone()).collect();
    assert_eq!(asked, [ADMIN_SCOPES, ADMIN_SCOPES_MAIL, ADMIN_SCOPES_GRAPH]);
    // Each page has a state of its own, and all are for the same directory and application.
    let states: Vec<String> = (0..3).map(|n| visit_asked(&visits, n)["state"].clone()).collect();
    assert!(states[0] != states[1] && states[1] != states[2]);
    for url in visits.lock().unwrap().iter() {
        assert!(url.starts_with(&format!("https://login.microsoftonline.com/{DIRECTORY}/v2.0/adminconsent?client_id={APPLICATION}&")), "{url}");
    }
    // A no to one of them is the answer: the rest is not asked.
    let no = "GET /?admin_consent=True&error=consent_required&error_description=AADSTS65004%3A+Denied.&state={state}";
    let (open, visits) = browser_answering(&[together, yes, no]);
    assert!(matches!(approval("", &open), Err(AdminRefusal::Declined(_))));
    assert_eq!(visited(&visits), 3);
    let (open, visits) = browser_answering(&[together, no, yes]);
    assert!(matches!(approval("", &open), Err(AdminRefusal::Declined(_))));
    assert_eq!(visited(&visits), 2);
    // Any other no is not asked about twice.
    let (open, visits) = browser_answering(&[no, yes]);
    assert!(matches!(approval("", &open), Err(AdminRefusal::Declined(_))));
    assert_eq!(visited(&visits), 1);
}

/// How a sign-in at `endpoints` ended, with the "browser" `open`.
fn ended(endpoints: &Endpoints, open: &dyn Fn(&str)) -> SignInEnd {
    let outcome = sign_in_refused(endpoints, &oauth(), "ada@example.org", open, &AtomicBool::new(false), PATIENCE);
    SignInEnd::from(outcome.unwrap_err())
}

#[test]
fn how_a_sign_in_ended_is_told_apart() {
    let server = FakeHttp::answering(ok(TOKENS));
    let endpoints = endpoints_with_consent(&server);
    let came_back = |answer: &str| {
        let (open, visits) = browser_answering(&[&format!("GET /?{answer}&state={{state}}")]);
        let end = ended(&endpoints, &open);
        // Nothing but a refusal of what was asked for is asked about twice.
        assert_eq!(visited(&visits), 1, "{answer}");
        end
    };
    // An administrator has to approve first: Microsoft's numbers for it, whatever the code.
    for number in [90_093, 90_094, 90_095, 900_941] {
        for code in ["access_denied", "invalid_client", "interaction_required"] {
            let answer = format!("error={code}&error_description=AADSTS{number}%3A+Admin+consent+is+required.%0D%0ATrace+ID%3A+12");
            assert_eq!(came_back(&answer), SignInEnd::Approval, "{number} {code}");
        }
    }
    // A no that was given on the page; also what leaving the page that asks for an
    // administrator comes back as, so the two are one here.
    assert_eq!(came_back("error=access_denied&error_description=AADSTS65004%3A+User+declined+to+consent+to+access+the+app."), SignInEnd::Declined);
    assert_eq!(came_back("error=access_denied&error_subcode=cancel"), SignInEnd::Declined);
    assert_eq!(came_back("error=access_denied"), SignInEnd::Declined);
    assert_eq!(came_back("error=consent_required"), SignInEnd::Declined);
    // The organisation's door is shut for another reason: its own words, and no approval is offered.
    for number in [50_105, 53_003, 7_000_112] {
        let end = came_back(&format!("error=access_denied&error_description=AADSTS{number}%3A+Not+for+you."));
        assert_eq!(end, SignInEnd::Other(NetError::Proto(ProtoError::Auth(format!("AADSTS{number}: Not for you.")))));
    }
    // (Shut, and an administrator's yes would not open it: said as shut.)
    let both = came_back("error=access_denied&error_description=AADSTS53003%3A+Blocked.+AADSTS90094%3A+Admin+consent.");
    assert!(matches!(both, SignInEnd::Other(_)), "{both:?}");
    assert!(matches!(came_back("error=server_error"), SignInEnd::Other(NetError::Proto(ProtoError::Rejected(_)))));
    assert!(matches!(came_back("session_state=abc"), SignInEnd::Other(NetError::Proto(ProtoError::Protocol(_)))));

    // Nothing came back at all: the page was closed, or left.
    let quiet = |_: &str| {};
    let late = sign_in_refused(&endpoints, &oauth(), "", &quiet, &AtomicBool::new(false), Duration::from_millis(200));
    assert_eq!(SignInEnd::from(late.unwrap_err()), SignInEnd::TimedOut);
    let off = sign_in_refused(&endpoints, &oauth(), "", &quiet, &AtomicBool::new(true), PATIENCE);
    assert_eq!(SignInEnd::from(off.unwrap_err()), SignInEnd::Cancelled);
    assert_eq!(sign_in_explained(&endpoints, &oauth(), "", &quiet, &AtomicBool::new(true)), Err(SignInEnd::Cancelled));
    // The token endpoint's no is never a no given on a page.
    let refusing = FakeHttp::answering(response("400 Bad Request", r#"{"error":"access_denied","error_description":"AADSTS65004: Declined."}"#));
    let (open, _) = browser_answering(&["GET /?code=the-code&state={state}"]);
    assert!(matches!(ended(&endpoints_of(&refusing), &open), SignInEnd::Other(NetError::Proto(ProtoError::Auth(_)))));
    // And a sign-in that goes through is as it always was.
    let (open, _) = browser_answering(&["GET /?code=the-code&state={state}"]);
    let tokens = sign_in_refused(&endpoints, &oauth(), "", &open, &AtomicBool::new(false), PATIENCE).unwrap();
    assert_eq!(tokens.access, "acc-ess.tok_en");
    assert_eq!(server.asked(), 1);
}

proptest! {
    #[test]
    fn the_address_for_an_administrator_holds_nothing_but_what_it_names(
        client in "[!-~]{1,60}",
        tenant in prop_oneof![
            Just(String::new()),
            "[a-z0-9]{1,12}(\\.[a-z0-9]{1,8}){0,2}",
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            "(common|consumers|organizations)",
            "\\PC{0,24}",
        ],
    ) {
        use mailcraft_model::Directory;
        let settings = OAuth { client_id: client.clone(), ..oauth() };
        let named = tenant.trim();
        let fits = named.is_empty()
            || (named.len() <= 253
                && named.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
                && named.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
                && named.bytes().next_back().is_some_and(|b| b.is_ascii_alphanumeric())
                && !named.contains(".."));
        match admin_consent_url(&settings, &tenant) {
            Ok(link) => {
                prop_assert!(fits, "{:?}", tenant);
                let (address, query) = link.split_once('?').unwrap();
                // Microsoft's own page, for that directory or for whichever the administrator is of.
                let directory = if Directory::of_tenant(named) == Directory::Organisation { named } else { "organizations" };
                prop_assert_eq!(address, format!("https://login.microsoftonline.com/{directory}/v2.0/adminconsent"));
                // The application, the permissions, the way back: three things, each as it was given.
                prop_assert!(query.bytes().all(|b| b.is_ascii_alphanumeric() || b"-._~%=&".contains(&b)), "{}", query);
                let asked = fields(query);
                prop_assert_eq!(asked.len(), 3);
                prop_assert_eq!(&asked["client_id"], &client);
                prop_assert_eq!(&asked["scope"], &admin_scope());
                prop_assert_eq!(&asked["redirect_uri"], "http://localhost");
            }
            Err(error) => {
                prop_assert!(!fits && matches!(error, NetError::Config(_)), "{:?}: {:?}", tenant, error);
            }
        }
    }

    #[test]
    fn arbitrary_answers_to_an_approval_never_panic_or_pass(request in proptest::collection::vec(any::<u8>(), 0..400)) {
        prop_assert!(judge_consent(&request, "the-state").1.is_none());
        let mut prefixed = b"GET /?state=the-state&".to_vec();
        prefixed.extend_from_slice(&request);
        let _ = judge_consent(&prefixed, "the-state");
    }

    #[test]
    fn approval_shaped_noise_is_a_yes_only_with_the_state_and_no_error(
        method in "(GET|POST|)",
        path in "(/|/x|)",
        query in proptest::collection::vec(("(admin_consent|state|error|error_description|tenant|x)", "(the-state|True|true|False|abc|\\PC{0,6}|)"), 0..6),
    ) {
        let pieces: Vec<String> = query.iter().map(|(name, value)| format!("{name}={}", http::encode(value))).collect();
        let request = format!("{method} {path}?{} HTTP/1.1\r\n\r\n", pieces.join("&"));
        let first = |wanted: &str| query.iter().find(|(name, _)| name == wanted).map(|(_, value)| value.as_str());
        match judge_consent(request.as_bytes(), "the-state") {
            (_, Some(outcome)) => {
                prop_assert_eq!((method.as_str(), path.as_str(), first("state")), ("GET", "/", Some("the-state")));
                if outcome.is_ok() {
                    prop_assert!(first("error").is_none_or(str::is_empty));
                    prop_assert!(first("admin_consent").is_some_and(|said| said.trim().eq_ignore_ascii_case("true")));
                }
            }
            (page, None) => prop_assert!(page.status != "200 OK"),
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The page an administrator is sent to from here: what [`admin_consent_url`] names, the
    /// way back to this machine, and a state nobody could guess.
    #[test]
    fn the_page_an_administrator_is_sent_to_holds_nothing_of_the_owner(tenant in "([a-z0-9]{1,12}\\.example|)") {
        let settings = OAuth { client_id: APPLICATION.into(), ..oauth() };
        let (opened, cancel) = (Mutex::new(String::new()), AtomicBool::new(false));
        // (Nobody comes back: the wait is called off as soon as the page is "open".)
        let open = |url: &str| {
            *opened.lock().unwrap() = url.to_string();
            cancel.store(true, Ordering::Relaxed);
        };
        prop_assert_eq!(admin_consent_within(&settings, &tenant, &open, &cancel, PATIENCE), Err(AdminRefusal::Cancelled));
        let url = opened.lock().unwrap().clone();
        let (address, query) = url.split_once('?').unwrap();
        let link = admin_consent_url(&settings, &tenant).unwrap();
        prop_assert_eq!(address, link.split_once('?').unwrap().0);
        let (asked, passed_on) = (fields(query), fields(link.split_once('?').unwrap().1));
        prop_assert_eq!(asked.len(), 4);
        prop_assert_eq!((&asked["client_id"], &asked["scope"]), (&passed_on["client_id"], &passed_on["scope"]));
        prop_assert_eq!(&asked["redirect_uri"], &format!("http://localhost:{}", port_of(&asked["redirect_uri"])));
        prop_assert!(asked["state"].len() >= 32 && asked["state"].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
    }
}

// ---- shared mailboxes (#70) ----

const SHARED_SCOPE: &str = "https://graph.microsoft.com/Mail.ReadWrite.Shared";

#[test]
fn a_shared_mailboxs_permission_is_never_asked_for_with_a_sign_in() {
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        let endpoints = Endpoints::for_resource(&oauth(), resource);
        let asked = format!("{} {} {}", endpoints.scope, endpoints.consent, endpoints.lesser);
        assert!(!asked.contains("Shared"), "{resource:?}: {asked}");
    }
    assert!(shared_scope().split(' ').any(|scope| scope == SHARED_SCOPE));
    assert!(shared_scope().contains("https://graph.microsoft.com/Mail.ReadWrite ") && shared_scope().contains("offline_access"));
    // Reading one doesn't ask for sending from one: that is asked for at the first send.
    assert!(!shared_scope().contains(SHARED_SEND_SCOPE));
    let send = shared_send_scope();
    assert!(send.split(' ').any(|scope| scope == SHARED_SEND_SCOPE) && send.split(' ').any(|scope| scope == SHARED_SCOPE), "{send}");
    assert!(send.contains("https://graph.microsoft.com/Mail.ReadWrite ") && send.contains("offline_access"));
}

const SHARED_SEND_SCOPE: &str = "https://graph.microsoft.com/Mail.Send.Shared";

#[test]
fn sending_from_a_shared_mailbox_asks_for_its_permission_and_nothing_less() {
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"send-access","refresh_token":"re-fresh-3"}"#;
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'.","error_codes":[65001],"suberror":"consent_required"}"#;
    let server = FakeHttp::answering(ok(granted));
    let got = refresh_graph_shared_send_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert_eq!((got.tokens.access.as_str(), got.tokens.refresh.as_str(), server.asked()), ("send-access", "re-fresh-3", 1));
    assert_eq!(form_of(&server, 0)["scope"], shared_send_scope());
    // Not agreed to yet: the window asks the owner; nothing is had without it.
    let server = FakeHttp::answering(response("400 Bad Request", unasked));
    assert!(refresh_graph_shared_send_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap_err().is_consent_needed());
    assert_eq!(server.asked(), 1);
}

#[test]
fn a_shared_mailboxs_tokens_are_asked_for_with_its_permission_and_nothing_less() {
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"shared-access","refresh_token":"re-fresh-2"}"#;
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'.","error_codes":[65001],"suberror":"consent_required"}"#;
    let server = FakeHttp::answering(ok(granted));
    let got = refresh_graph_shared_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert_eq!((got.tokens.access.as_str(), got.tokens.refresh.as_str(), server.asked()), ("shared-access", "re-fresh-2", 1));
    assert_eq!(form_of(&server, 0)["scope"], shared_scope());
    // Not agreed to yet: the owner is to be asked on the page; nothing is asked without it.
    let server = FakeHttp::answering(response("400 Bad Request", unasked));
    assert!(refresh_graph_shared_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap_err().is_consent_needed());
    assert_eq!(server.asked(), 1);
}

const SHARED_CALENDARS_SCOPE: &str = "https://graph.microsoft.com/Calendars.ReadWrite.Shared";

#[test]
fn a_shared_mailboxs_calendars_are_asked_for_by_themselves_and_never_with_a_sign_in() {
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        let endpoints = Endpoints::for_resource(&oauth(), resource);
        let asked = format!("{} {} {}", endpoints.scope, endpoints.consent, endpoints.lesser);
        assert!(!asked.contains(SHARED_CALENDARS_SCOPE), "{resource:?}: {asked}");
    }
    // Reading mail from a shared mailbox, or sending from one, doesn't ask for its calendars.
    assert!(!shared_scope().contains(SHARED_CALENDARS_SCOPE) && !shared_send_scope().contains(SHARED_CALENDARS_SCOPE));
    let calendars = shared_calendar_scope();
    // `Calendars.ReadWrite.Shared` alone: never the read-only twin.
    assert!(calendars.split(' ').any(|scope| scope == SHARED_CALENDARS_SCOPE) && !calendars.contains("Calendars.Read.Shared"), "{calendars}");
    assert!(
        calendars.split(' ').any(|scope| scope == SHARED_SCOPE) && calendars.contains("offline_access") && !calendars.contains(SHARED_SEND_SCOPE)
    );
    // The administrator's one approval covers it.
    assert!(admin_scope().split(' ').any(|scope| scope == SHARED_CALENDARS_SCOPE));
}

#[test]
fn a_shared_mailboxs_calendars_need_their_permission_and_nothing_less() {
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"calendar-access","refresh_token":"re-fresh-4"}"#;
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'.","error_codes":[65001],"suberror":"consent_required"}"#;
    let server = FakeHttp::answering(ok(granted));
    let got = refresh_graph_shared_calendars_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert_eq!((got.tokens.access.as_str(), got.tokens.refresh.as_str(), server.asked()), ("calendar-access", "re-fresh-4", 1));
    assert_eq!(form_of(&server, 0)["scope"], shared_calendar_scope());
    // Not agreed to yet: the owner is asked on the page, by itself; nothing is had without it.
    let server = FakeHttp::answering(response("400 Bad Request", unasked));
    assert!(refresh_graph_shared_calendars_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap_err().is_consent_needed());
    assert_eq!(server.asked(), 1);
}

#[test]
fn calendars_opened_by_address_are_asked_for_with_their_permission_and_nothing_more() {
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"calendars-access","refresh_token":"re-fresh-2"}"#;
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'.","error_codes":[65001],"suberror":"consent_required"}"#;
    let server = FakeHttp::answering(ok(granted));
    let got = refresh_graph_opened_calendars_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert_eq!((got.tokens.access.as_str(), got.tokens.refresh.as_str(), server.asked()), ("calendars-access", "re-fresh-2", 1));
    let scope = form_of(&server, 0)["scope"].to_string();
    assert!(scope.contains("Calendars.ReadWrite.Shared") && scope.contains("Calendars.ReadWrite "), "{scope}");
    // Never the mail permissions of shared mailboxes, nor the read-only twin.
    assert!(!scope.contains("Mail.ReadWrite") && !scope.contains("Mail.Send.Shared") && !scope.contains("Calendars.Read.Shared"), "{scope}");
    // Not agreed to yet: the owner is to be asked on the page; nothing is asked without it.
    let server = FakeHttp::answering(response("400 Bad Request", unasked));
    assert!(refresh_graph_opened_calendars_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap_err().is_consent_needed());
    assert_eq!(server.asked(), 1);
    // An ordinary sign-in and the others' refreshes don't carry it.
    assert!(!shared_scope().contains("Calendars.ReadWrite.Shared"));
}

const TASKS_SCOPE: &str = "https://graph.microsoft.com/Tasks.ReadWrite";

#[test]
fn tasks_are_never_put_on_a_sign_in_page_so_a_mail_sign_in_never_waits_on_them() {
    // Asked for by itself, after the window has asked the owner, for new accounts and old alike: a
    // tenant that makes an administrator approve it must not stop a plain mail sign-in.
    for resource in [Resource::Mail, Resource::Graph, Resource::GraphMail] {
        let endpoints = Endpoints::for_resource(&oauth(), resource);
        assert!(!endpoints.consent.contains("Tasks") && !endpoints.lesser.contains("Tasks"), "{resource:?}: {}", endpoints.consent);
        assert!(!endpoints.scope.contains("Tasks"), "{resource:?}: {}", endpoints.scope);
    }
    // A refresh for calendar and contacts, as a sign-in from before asks for it, doesn't mention it.
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"graph-access","refresh_token":"re-fresh-2"}"#;
    let server = FakeHttp::answering(ok(granted));
    let all = Endpoints { scope: format!("{GRAPH_SCOPES} offline_access openid profile email"), ..endpoints_of(&server) };
    refresh_graph_at(&all, &all, &endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert!(!form_of(&server, 0)["scope"].contains("Tasks"));
    // The administrator's one approval covers it.
    assert!(admin_scope().split(' ').any(|scope| scope == TASKS_SCOPE));
}

#[test]
fn a_tasks_job_needs_its_permission_and_nothing_less() {
    let granted = r#"{"token_type":"Bearer","expires_in":3600,"access_token":"tasks-access","refresh_token":"re-fresh-5"}"#;
    let unasked = r#"{"error":"invalid_grant","error_description":"AADSTS65001: The user or administrator has not consented to use the application with ID 'client-1' named 'MailCraft'.","error_codes":[65001],"suberror":"consent_required"}"#;
    let server = FakeHttp::answering(ok(granted));
    let got = refresh_graph_tasks_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap();
    assert_eq!((got.tokens.access.as_str(), got.tokens.refresh.as_str(), server.asked()), ("tasks-access", "re-fresh-5", 1));
    let scope = form_of(&server, 0)["scope"].to_string();
    assert!(scope.split(' ').any(|s| s == TASKS_SCOPE) && scope.contains("Calendars.ReadWrite"), "{scope}");
    // Never the permissions that are asked for by themselves for other things.
    assert!(!scope.contains("Mail.ReadWrite") && !scope.contains("Shared"), "{scope}");
    // A sign-in from before: the owner is to be asked on the page; nothing is read without it.
    let server = FakeHttp::answering(response("400 Bad Request", unasked));
    assert!(refresh_graph_tasks_at(&endpoints_of(&server), &oauth(), "re-fresh").unwrap_err().is_consent_needed());
    assert_eq!(server.asked(), 1);
}
