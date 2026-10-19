//! The client over scripted answers: `connect` and `call`, and what they make of a hostile server.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use proptest::prelude::*;
use serde_json::json;

use super::*;

const SESSION: &str = "https://jmap.example.org/jmap/session?account=ada%40example.org";
const TOKEN: &str = "tk-3f9a7c";

/// An `Http` that answers what it is told to, in order, and keeps what it was sent.
#[derive(Clone, Default)]
struct Script {
    replies: Rc<RefCell<VecDeque<Result<Response>>>>,
    sent: Rc<RefCell<Vec<Request>>>,
}

impl Script {
    fn new(replies: Vec<Result<Response>>) -> Script {
        Script { replies: Rc::new(RefCell::new(replies.into())), sent: Rc::default() }
    }

    fn sent(&self) -> Vec<Request> {
        self.sent.borrow().clone()
    }
}

impl Http for Script {
    async fn send(&self, req: Request) -> Result<Response> {
        self.sent.borrow_mut().push(req);
        self.replies.borrow_mut().pop_front().unwrap_or_else(|| Err(Error::Io("nothing was scripted".into())))
    }
}

fn reply(status: u16, body: &Value) -> Result<Response> {
    Ok(Response { status, body: serde_json::to_vec(body).unwrap(), retry_after: None })
}

fn status(status: u16, retry_after: Option<u32>) -> Result<Response> {
    Ok(Response { status, body: Vec::new(), retry_after })
}

/// A session as a server of the standard kind has it, with every kind of address.
fn session_doc() -> Value {
    json!({
        "capabilities": {CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": 500}, MAIL: {}, "urn:ietf:params:jmap:submission": {}},
        "accounts": {"A1": {"name": "ada@example.org"}},
        "primaryAccounts": {MAIL: "A1"},
        "apiUrl": "https://jmap.example.org/jmap/api",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?type={type}",
        "uploadUrl": "//jmap.example.org/jmap/upload/{accountId}/",
        "state": "1",
    })
}

fn connect_with(address: &str, doc: &Value) -> (Result<Client<Script>>, Script) {
    let script = Script::new(vec![reply(200, doc)]);
    (block_on(Client::connect(script.clone(), address, &Auth::bearer(TOKEN))), script)
}

fn connect_to(doc: &Value) -> Result<Client<Script>> {
    connect_with(SESSION, doc).0
}

/// A connected client, and the script it speaks over; `replies` follow the session's.
fn client(replies: Vec<Result<Response>>) -> (Client<Script>, Script) {
    let mut all = vec![reply(200, &session_doc())];
    all.extend(replies);
    let script = Script::new(all);
    (block_on(Client::connect(script.clone(), SESSION, &Auth::bearer(TOKEN))).unwrap(), script)
}

fn answers_to(responses: Value) -> Result<Response> {
    reply(200, &json!({"methodResponses": responses, "sessionState": "1"}))
}

fn mutated(change: impl FnOnce(&mut Value)) -> Value {
    let mut doc = session_doc();
    change(&mut doc);
    doc
}

#[test]
fn a_bare_host_gets_the_well_known_path_and_nothing_else_does() {
    for (typed, session) in [
        ("jmap.example.com", "https://jmap.example.com/.well-known/jmap"),
        (" https://api.example.com/ ", "https://api.example.com/.well-known/jmap"),
        ("https://api.example.com/jmap/session", "https://api.example.com/jmap/session"),
        ("https://api.example.com/jmap/", "https://api.example.com/jmap/"),
        ("https://api.example.com?account=a", "https://api.example.com/.well-known/jmap?account=a"),
        ("https://api.example.com/?account=a#f", "https://api.example.com/.well-known/jmap?account=a#f"),
        ("https://api.example.com/s?account=a", "https://api.example.com/s?account=a"),
        ("http://127.0.0.1:8080", "http://127.0.0.1:8080/.well-known/jmap"),
        ("[::1]:8080", "https://[::1]:8080/.well-known/jmap"),
        ("example.org/p?next=https://o/", "https://example.org/p?next=https://o/"),
        ("example.org?next=https://o/", "https://example.org/.well-known/jmap?next=https://o/"),
        ("HTTPS://Example.org", "HTTPS://Example.org/.well-known/jmap"),
    ] {
        assert_eq!(session_url(typed), session, "{typed}");
    }
}

#[test]
fn ids_are_the_url_safe_alphabet_up_to_512_characters() {
    for good in ["a", "E1", "A-z_9", &"x".repeat(MAX_ID)] {
        assert!(valid_id(good), "{good}");
    }
    for bad in ["", "a b", "a/b", "a=", "é", "../x", "a\n", "a%41", "a.b", &"x".repeat(MAX_ID + 1)] {
        assert!(!valid_id(bad), "{bad}");
    }
}

#[test]
fn credentials_are_checked_and_never_shown() {
    assert_eq!(Auth::basic("me", "pw").header().unwrap(), "Basic bWU6cHc=");
    assert_eq!(Auth::basic("me", "p:w").header().unwrap(), "Basic bWU6cDp3", "a password may hold a colon");
    assert_eq!(Auth::basic("jörg", "pässwörd").header().unwrap(), "Basic asO2cmc6cMOkc3N3w7ZyZA==", "sent as UTF-8");
    assert_eq!(Auth::basic("", "pw").header().unwrap(), "Basic OnB3", "a token as the password, say");
    assert_eq!(Auth::basic("me", "").header().unwrap(), "Basic bWU6");
    assert_eq!(Auth::bearer("abc.DEF-1_2~3+/=").header().unwrap(), "Bearer abc.DEF-1_2~3+/=");
    for bad in [
        Auth::basic("a:b", "pw"),
        Auth::basic("me", "p\r\nX: y"),
        Auth::basic("m\0e", "pw"),
        Auth::basic("me", "p".repeat(MAX_CREDENTIALS)),
        Auth::basic("", ""),
        Auth::bearer(""),
        Auth::bearer("a b"),
        Auth::bearer("tok\r\nX: y"),
        Auth::bearer("tökén"),
        Auth::bearer("a".repeat(MAX_CREDENTIALS + 1)),
    ] {
        assert!(matches!(bad.header(), Err(Error::Auth(_))), "{bad:?}");
    }
    let shown = format!("{:?} {:?}", Auth::basic("ada@example.org", "hunter2"), Auth::bearer("s3cr3t-token"));
    for secret in ["ada", "example", "hunter2", "s3cr3t"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    // Nothing is sent with credentials that can't be.
    let script = Script::new(vec![reply(200, &session_doc())]);
    let refused = block_on(Client::connect(script.clone(), SESSION, &Auth::bearer("a b")));
    assert!(matches!(refused, Err(Error::Auth(_))) && script.sent().is_empty());
}

#[test]
fn connect_signs_in_and_resolves_the_session_against_its_own_address() {
    let (client, script) = client(vec![]);
    let sent = script.sent();
    let first = sent.first().unwrap();
    assert_eq!((first.method.as_str(), first.url.as_str(), first.max), ("GET", SESSION, MAX_SESSION));
    assert!(first.headers.contains(&("Authorization".into(), "Bearer tk-3f9a7c".into())));
    assert!(first.headers.contains(&("Accept".into(), "application/json".into())));
    assert_eq!(client.account_id(), "A1");
    assert!(client.has_capability(MAIL) && client.has_capability("urn:ietf:params:jmap:submission") && !client.has_capability("urn:example:x"));
    assert_eq!((client.max_objects_in_get(), client.max_calls_in_request()), (500, 16));
    // A template of the session is filled in with the account, and whatever is put in it is encoded.
    assert_eq!(
        client.download_url("B1", "message/rfc822", "message.eml").unwrap(),
        "https://jmap.example.org/jmap/download/A1/B1/message.eml?type=message%2Frfc822"
    );
    assert_eq!(
        client.download_url("B-1_x", "a b", "n/a?x#y&z={name}").unwrap(),
        "https://jmap.example.org/jmap/download/A1/B-1_x/n%2Fa%3Fx%23y%26z%3D%7Bname%7D?type=a%20b"
    );
    assert_eq!(client.upload_url().unwrap(), "https://jmap.example.org/jmap/upload/A1/");
    for bad in ["", "../x", "a/b", "B1?x", &"b".repeat(MAX_ID + 1)] {
        assert!(matches!(client.download_url(bad, "t", "n"), Err(Error::Protocol(_))), "{bad}");
    }
    // A name or a type is at most 512 bytes.
    let long = "n".repeat(MAX_VARIABLE + 1);
    assert!(matches!(client.download_url("B1", "t", &long), Err(Error::Protocol(_))));
    assert!(matches!(client.download_url("B1", &long, "n"), Err(Error::Protocol(_))));
    assert!(client.download_url("B1", "t", &"n".repeat(MAX_VARIABLE)).is_ok());
}

#[test]
fn a_template_is_filled_in_in_one_pass_and_never_grows_past_its_cap() {
    // A variable nobody gave is empty, and braces in a value are encoded, not expanded again.
    let client = connect_to(&mutated(|d| d["downloadUrl"] = json!("/d/{blobId}/{accept}/{name}?t={type}&x={accountId}{blobId}"))).unwrap();
    assert_eq!(client.download_url("B1", "t", "{blobId}").unwrap(), "https://jmap.example.org/d/B1//%7BblobId%7D?t=t&x=A1B1");
    // Many variables and a long value are refused before the address is built, not after.
    let client = connect_to(&mutated(|d| d["downloadUrl"] = json!(format!("/d/{}", "{name}".repeat(600))))).unwrap();
    assert!(matches!(client.download_url("B1", "t", &"x".repeat(MAX_VARIABLE)), Err(Error::Protocol(_))));
    assert!(client.download_url("B1", "t", "x").is_ok(), "600 short values fit");
    let template = "{a}".repeat(MAX_TEMPLATE / 3);
    assert!(expand(&template, &[("a", &"é".repeat(MAX_VARIABLE / 2))]).is_err());
    // Braces that are not variables are the session's mistake, said when it is read.
    assert_eq!(expand("/d/{a", &[("a", "1")]).unwrap(), "/d/{a");
}

#[test]
fn a_bare_host_is_asked_for_the_well_known_session() {
    let script = Script::new(vec![reply(200, &session_doc())]);
    block_on(Client::connect(script.clone(), "jmap.example.org", &Auth::bearer(TOKEN))).unwrap();
    assert_eq!(script.sent().first().map(|r| r.url.as_str()), Some("https://jmap.example.org/.well-known/jmap"));
}

#[test]
fn connect_refuses_a_session_that_cannot_be_used() {
    let cases = [
        ("no mail", mutated(|d| drop(d["capabilities"].as_object_mut().unwrap().remove(MAIL)))),
        ("no capabilities", mutated(|d| drop(d.as_object_mut().unwrap().remove("capabilities")))),
        ("capabilities a list", mutated(|d| d["capabilities"] = json!([MAIL]))),
        ("no primary accounts", mutated(|d| drop(d.as_object_mut().unwrap().remove("primaryAccounts")))),
        ("no mail account", mutated(|d| d["primaryAccounts"] = json!({"urn:example:x": "A1"}))),
        ("an account that is no id", mutated(|d| d["primaryAccounts"][MAIL] = json!("A 1"))),
        ("an empty account id", mutated(|d| d["primaryAccounts"][MAIL] = json!(""))),
        ("a long account id", mutated(|d| d["primaryAccounts"][MAIL] = json!("a".repeat(MAX_ID + 1)))),
        ("a number for an account", mutated(|d| d["primaryAccounts"][MAIL] = json!(7))),
        ("no apiUrl", mutated(|d| drop(d.as_object_mut().unwrap().remove("apiUrl")))),
        ("no downloadUrl", mutated(|d| drop(d.as_object_mut().unwrap().remove("downloadUrl")))),
        ("no uploadUrl", mutated(|d| drop(d.as_object_mut().unwrap().remove("uploadUrl")))),
        ("a number for an address", mutated(|d| d["apiUrl"] = json!(7))),
        ("a template for the API", mutated(|d| d["apiUrl"] = json!("/api/{accountId}"))),
        ("another scheme", mutated(|d| d["apiUrl"] = json!("ftp://jmap.example.org/api"))),
        ("a user name in an address", mutated(|d| d["apiUrl"] = json!("https://u:p@jmap.example.org/api"))),
        ("a space in an address", mutated(|d| d["uploadUrl"] = json!("/up load/{accountId}"))),
        ("a line break in an address", mutated(|d| d["downloadUrl"] = json!("/d/{blobId}\r\nX-Evil: 1"))),
        ("an address that is too long", mutated(|d| d["downloadUrl"] = json!(format!("/{}", "d".repeat(MAX_TEMPLATE))))),
        ("a template never closed", mutated(|d| d["downloadUrl"] = json!("/d/{blobId"))),
        ("a brace that closes nothing", mutated(|d| d["downloadUrl"] = json!("/d/blobId}"))),
        ("a variable with no name", mutated(|d| d["downloadUrl"] = json!("/d/{}"))),
        ("a variable inside a variable", mutated(|d| d["downloadUrl"] = json!("/d/{a{b}}"))),
        ("a variable with a bad name", mutated(|d| d["downloadUrl"] = json!("/d/{blob-id}"))),
        ("a variable with a long name", mutated(|d| d["downloadUrl"] = json!(format!("/d/{{{}}}", "a".repeat(MAX_VARIABLE_NAME + 1))))),
        ("an address beyond ASCII", mutated(|d| d["downloadUrl"] = json!("/d/é/{blobId}"))),
        ("a list", json!([])),
        ("a string", json!("session")),
        ("null", Value::Null),
    ];
    for (what, doc) in cases {
        let refused = connect_to(&doc);
        assert!(matches!(refused, Err(Error::Protocol(_))), "{what}: {refused:?}");
    }
    // Not even JSON.
    let script = Script::new(vec![Ok(Response { status: 200, body: b"<html>".to_vec(), retry_after: None })]);
    assert!(matches!(block_on(Client::connect(script, SESSION, &Auth::bearer(TOKEN))), Err(Error::Protocol(_))));
}

#[test]
fn connect_keeps_the_credentials_on_the_server_the_session_is_served_from() {
    for key in ["apiUrl", "downloadUrl", "uploadUrl"] {
        // Not down to http, not to another server, another port, or a name that only looks alike.
        for bad in [
            "http://jmap.example.org/x",
            "http://127.0.0.1/x",
            "https://elsewhere.example/x",
            "https://jmap.example.org:8443/x",
            "https://jmap.example.org.evil.example/x",
            "https://evil.example/jmap.example.org/x",
            "//elsewhere.example/x",
            "//jmap.example.org:444/x",
        ] {
            let result = connect_to(&mutated(|d| d[key] = json!(bad)));
            let Err(Error::Tls(why)) = &result else { panic!("{key} {bad}: {result:?}") };
            let words = if bad.starts_with("http:") { "isn't https" } else { "isn't on the server the session is served from" };
            assert!(why.contains(words), "{key} {bad}: {why}");
        }
        // The server itself, whichever way the address names it.
        for good in ["/x", "x", "//jmap.example.org/x", "https://jmap.example.org/x", "https://JMAP.Example.org:443/x"] {
            let client = connect_to(&mutated(|d| d[key] = json!(good))).unwrap_or_else(|e| panic!("{key} {good}: {e}"));
            let address = match key {
                "apiUrl" => &client.api,
                "downloadUrl" => &client.download,
                _ => &client.upload,
            };
            assert!(address.starts_with("https://jmap.example.org/") && address.ends_with("/x"), "{key} {good}: {address}");
        }
    }
    // A plain http session reaches nowhere but this computer, and nothing is sent to find out.
    let script = Script::new(vec![reply(200, &session_doc())]);
    for bad in ["http://jmap.example.org/jmap/session", "http://10.0.0.1/s", "http://127.0.0.1.example.org/s"] {
        let result = block_on(Client::connect(script.clone(), bad, &Auth::bearer(TOKEN)));
        assert!(matches!(result, Err(Error::Tls(_))), "{bad}");
    }
    assert!(script.sent().is_empty());
}

#[test]
fn a_plain_http_session_is_for_this_computer_alone() {
    let doc = mutated(|d| {
        d["apiUrl"] = json!("/jmap/api");
        d["downloadUrl"] = json!("/jmap/d/{blobId}");
        d["uploadUrl"] = json!("http://127.0.0.1:8080/jmap/up/{accountId}");
    });
    let script = Script::new(vec![reply(200, &doc), answers_to(json!([["Mailbox/get", {}, "c0"]]))]);
    let client = block_on(Client::connect(script.clone(), "http://127.0.0.1:8080/jmap/session", &Auth::bearer(TOKEN))).unwrap();
    block_on(client.mail(vec![("Mailbox/get", json!({}))])).unwrap();
    assert_eq!(script.sent().get(1).map(|r| r.url.as_str()), Some("http://127.0.0.1:8080/jmap/api"));
    assert_eq!(client.upload_url().unwrap(), "http://127.0.0.1:8080/jmap/up/A1");
    // It does not hand the credentials on to another plain http address, or up to another server.
    for bad in [
        "http://jmap.example.org/api",
        "http://10.0.0.1/api",
        "http://127.0.0.1.example.org/api",
        "http://127.0.0.1:9/api",
        "https://127.0.0.1:8080/api",
        "https://elsewhere.example/api",
    ] {
        let doc = mutated(|d| d["apiUrl"] = json!(bad));
        let script = Script::new(vec![reply(200, &doc)]);
        let result = block_on(Client::connect(script, "http://localhost:8080/jmap/session", &Auth::bearer(TOKEN)));
        assert!(matches!(result, Err(Error::Tls(_))), "{bad}");
    }
}

#[test]
fn what_is_sent_for_the_session_is_the_address_that_was_checked() {
    for (typed, sent) in [
        ("https://jmap.example.org/jmap/a b\r\nX-Evil: 1", "https://jmap.example.org/jmap/a%20b%0D%0AX-Evil:%201"),
        ("https://jmap.example.org/jmap/s\u{0}x", "https://jmap.example.org/jmap/s%00x"),
        ("https://jmap.example.org/jmap/session#frag", "https://jmap.example.org/jmap/session"),
        ("HTTPS://JMAP.EXAMPLE.ORG:443/s", "https://jmap.example.org/s"),
        ("https://jmap.example.org/jmap/é?q=ü", "https://jmap.example.org/jmap/%C3%A9?q=%C3%BC"),
    ] {
        let script = Script::new(vec![reply(200, &session_doc())]);
        let connected = block_on(Client::connect(script.clone(), typed, &Auth::bearer(TOKEN)));
        assert!(connected.is_ok(), "{typed:?}: {connected:?}");
        let url = script.sent().first().map(|r| r.url.clone()).unwrap();
        assert_eq!(url, sent, "{typed:?}");
        assert!(url.bytes().all(|b| b.is_ascii_graphic()));
    }
    // What is no address is refused before anything is sent.
    let script = Script::new(vec![reply(200, &session_doc())]);
    for typed in [
        "https://jmap.example.org:+443/s",
        "https://jmap.exam ple.org/s",
        "https://user@jmap.example.org/s",
        "https://jmap.example.org\\@evil.example/",
        "https://[evil.example]/s",
    ] {
        assert!(matches!(block_on(Client::connect(script.clone(), typed, &Auth::bearer(TOKEN))), Err(Error::Protocol(_))), "{typed}");
    }
    assert!(script.sent().is_empty());
}

#[test]
fn a_session_address_over_2048_bytes_is_refused_unsent() {
    let script = Script::new(vec![reply(200, &session_doc())]);
    let too_long = format!("https://jmap.example.org/{}", "s".repeat(MAX_SESSION_URL));
    assert!(matches!(block_on(Client::connect(script.clone(), &too_long, &Auth::bearer(TOKEN))), Err(Error::Protocol(_))));
    assert!(script.sent().is_empty());
    let just_fits = format!("https://jmap.example.org/{}", "s".repeat(MAX_SESSION_URL - "https://jmap.example.org/".len()));
    assert_eq!(just_fits.len(), MAX_SESSION_URL);
    assert!(block_on(Client::connect(script.clone(), &just_fits, &Auth::bearer(TOKEN))).is_ok());
}
#[test]
fn connect_clamps_what_the_server_says_it_takes() {
    let limits = |core: Value| connect_to(&mutated(|d| d["capabilities"][CORE] = core)).unwrap();
    for (said, objects, calls) in [
        (json!({"maxObjectsInGet": 100, "maxCallsInRequest": 8}), 100, 8),
        (json!({"maxObjectsInGet": 1000, "maxCallsInRequest": 64}), 1000, 64),
        (json!({"maxObjectsInGet": 10_000_000, "maxCallsInRequest": 100_000}), 1000, 64),
        (json!({"maxObjectsInGet": u64::MAX, "maxCallsInRequest": u64::MAX}), 1000, 64),
        (json!({"maxObjectsInGet": 0, "maxCallsInRequest": 0}), 1, 1),
        (json!({}), 500, 16),
        (json!({"maxObjectsInGet": "many", "maxCallsInRequest": -3}), 500, 16),
        (json!({"maxObjectsInGet": null, "maxCallsInRequest": 2.5}), 500, 16),
        (json!({"maxObjectsInGet": 1e30, "maxCallsInRequest": [1]}), 500, 16),
        (json!("core"), 500, 16),
    ] {
        let client = limits(said.clone());
        assert_eq!((client.max_objects_in_get(), client.max_calls_in_request()), (objects, calls), "{said}");
    }
    // A session without the core capability at all is held to what the standard says every server takes.
    let client = connect_to(&mutated(|d| drop(d["capabilities"].as_object_mut().unwrap().remove(CORE)))).unwrap();
    assert_eq!((client.max_objects_in_get(), client.max_calls_in_request()), (500, 16));
}

#[test]
fn connect_remembers_every_capability_the_session_names() {
    let mut capabilities: serde_json::Map<String, Value> = (0..1000).map(|n| (format!("urn:example:cap{n}"), json!({}))).collect();
    capabilities.insert(MAIL.into(), json!({}));
    let client = connect_to(&mutated(|d| d["capabilities"] = Value::Object(capabilities))).unwrap();
    assert!(client.has_capability(MAIL) && client.has_capability("urn:example:cap999") && !client.has_capability(CORE));
}

#[test]
fn http_failures_become_the_error_the_caller_acts_on() {
    let problem = |status: u16, body: &Value| reply(status, body);
    let cases: Vec<(Result<Response>, Error)> = vec![
        (status(401, None), Error::Auth("the server refused the credentials".into())),
        (status(403, None), Error::Auth("the server refused the credentials".into())),
        (status(429, Some(120)), Error::Busy { status: 429, retry_after: Some(120) }),
        (status(503, None), Error::Busy { status: 503, retry_after: None }),
        (status(503, Some(0)), Error::Busy { status: 503, retry_after: Some(1) }),
        (status(429, Some(u32::MAX)), Error::Busy { status: 429, retry_after: Some(3600) }),
        (status(500, None), Error::Bad("HTTP 500".into())),
        (status(302, None), Error::Bad("HTTP 302".into())),
        (status(0, None), Error::Bad("HTTP 0".into())),
        (problem(404, &json!({"type": "about:blank", "detail": "no such\n\tsession"})), Error::Bad("HTTP 404 no such session".into())),
        (
            problem(400, &json!({"type": "urn:ietf:params:jmap:error:notRequest"})),
            Error::Bad("HTTP 400 urn:ietf:params:jmap:error:notRequest".into()),
        ),
        (problem(400, &json!([1, 2])), Error::Bad("HTTP 400".into())),
        (
            problem(400, &json!({"type": LIMIT_PROBLEM, "limit": "maxCallsInRequest", "status": 400, "detail": "too many"})),
            Error::TooLarge("the request is over the server's limit maxCallsInRequest".into()),
        ),
        (problem(400, &json!({"type": LIMIT_PROBLEM})), Error::TooLarge("the request is over the server's limit".into())),
        (problem(413, &json!({"type": LIMIT_PROBLEM})), Error::Bad(format!("HTTP 413 {LIMIT_PROBLEM}"))),
        (Err(Error::Io("offline".into())), Error::Io("offline".into())),
    ];
    for (reply, expected) in cases {
        // At the session...
        let script = Script::new(vec![reply.clone()]);
        assert_eq!(block_on(Client::connect(script, SESSION, &Auth::bearer(TOKEN))).unwrap_err(), expected);
        // ... and at a request.
        let (client, _) = client(vec![reply]);
        assert_eq!(block_on(client.mail(vec![("Mailbox/get", json!({}))])).unwrap_err(), expected);
    }
    // What a server says in an error is cut to a line of reasonable size.
    let long = reply(400, &json!({"detail": format!("line\n{}", "x".repeat(10_000))})).unwrap();
    let Err(Error::Bad(text)) = check(&long) else { panic!("not an error") };
    assert!(text.chars().count() < 400 && !text.contains('\n'), "{text}");
    // A body that is no problem details at all, or too big to be, is not read for a reason.
    let big = Response { status: 400, body: vec![b'{'; MAX_PROBLEM + 1], retry_after: None };
    assert_eq!(check(&big), Err(Error::Bad("HTTP 400".into())));
}

#[test]
fn an_answer_over_the_size_asked_for_is_refused() {
    let big = |max: usize| Ok(Response { status: 200, body: vec![b' '; max + 1], retry_after: None });
    let script = Script::new(vec![big(MAX_SESSION)]);
    let refused = block_on(Client::connect(script, SESSION, &Auth::bearer(TOKEN)));
    assert!(matches!(refused, Err(Error::TooLarge(_))), "{refused:?}");
    let (client, _) = client(vec![big(MAX_JSON)]);
    let refused = block_on(client.mail(vec![("Mailbox/get", json!({}))]));
    assert!(matches!(refused, Err(Error::TooLarge(_))), "{refused:?}");
    // The size the request itself named is the one held to.
    let script = Script::new(vec![Ok(Response { status: 200, body: vec![b'x'; 11], retry_after: None })]);
    let refused = block_on(send(&script, Request::get("https://jmap.example.org/", 10)));
    assert!(matches!(refused, Err(Error::TooLarge(_))));
    let script = Script::new(vec![Ok(Response { status: 200, body: vec![b'x'; 10], retry_after: None })]);
    assert!(block_on(send(&script, Request::get("https://jmap.example.org/", 10))).is_ok());
}

#[test]
fn call_sends_one_batched_post_and_answers_in_order() {
    let responses = json!([["Email/query", {"ids": ["E2", "E1"]}, "c0"], ["Email/get", {"list": [{"id": "E2"}]}, "c1"]]);
    let (client, script) = client(vec![answers_to(responses)]);
    let calls = vec![
        ("Email/query", json!({"accountId": "A1", "filter": {"inMailbox": "M1"}, "limit": 2})),
        ("Email/get", json!({"accountId": "A1", "#ids": result_of(0, "Email/query", "/ids"), "properties": ["id"]})),
    ];
    let answers = block_on(client.mail(calls)).unwrap();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers.get(0).unwrap(), &json!({"ids": ["E2", "E1"]}));
    assert_eq!(answers.get(1).unwrap(), &json!({"list": [{"id": "E2"}]}));
    assert_eq!(script.sent().len(), 2, "the session, then one request for both calls");
    let post = script.sent().get(1).cloned().unwrap();
    assert_eq!((post.method.as_str(), post.url.as_str(), post.max), ("POST", "https://jmap.example.org/jmap/api", MAX_JSON));
    assert!(post.headers.contains(&("Content-Type".into(), "application/json".into())));
    assert!(post.headers.contains(&("Authorization".into(), "Bearer tk-3f9a7c".into())));
    let body: Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(body["using"], json!([CORE, MAIL]));
    assert_eq!(body["methodCalls"][0][2], "c0");
    assert_eq!(body["methodCalls"][1][0], "Email/get");
    assert_eq!(body["methodCalls"][1][1]["#ids"], json!({"resultOf": "c0", "name": "Email/query", "path": "/ids"}));
    assert_eq!(body["methodCalls"][1][2], "c1");
    assert_eq!(format!("{answers:?}"), "Answers(2 of which 0 failed)");
}

#[test]
fn a_method_error_fails_that_call_alone() {
    let responses = json!([
        ["Mailbox/get", {"list": [{"id": "M1"}]}, "c0"],
        ["error", {"type": "invalidArguments", "description": "bad\nids\u{7}"}, "c1"],
        ["Email/get", {"list": []}, "c2"],
        ["error", {"type": 7}, "c3"]
    ]);
    let (client, _) = client(vec![answers_to(responses)]);
    let calls = ["Mailbox/get", "Email/query", "Email/get", "Email/get"].map(|name| (name, json!({}))).to_vec();
    let answers = block_on(client.mail(calls)).unwrap();
    assert_eq!(answers.get(0).unwrap(), &json!({"list": [{"id": "M1"}]}));
    let refused = Error::Refused { kind: "invalidArguments".into(), description: "bad ids".into() };
    assert_eq!(answers.get(1).unwrap_err(), refused);
    assert_eq!(refused.to_string(), "the server refused: invalidArguments bad ids");
    assert_eq!(answers.get(2).unwrap(), &json!({"list": []}));
    assert_eq!(answers.get(3).unwrap_err(), Error::Refused { kind: String::new(), description: String::new() });
    assert_eq!(format!("{answers:?}"), "Answers(4 of which 2 failed)");
}

#[test]
fn a_missing_or_misnamed_answer_fails_that_call_alone() {
    let responses = json!([
        ["Mailbox/get", {"first": true}, "c0"],
        ["Mailbox/get", {"again": true}, "c0"],
        ["Email/get", {"for": "another call"}, "c1"],
        "not an answer",
        ["Email/get"],
        [null, {}, "c3"]
    ]);
    let (client, _) = client(vec![answers_to(responses)]);
    let calls = ["Mailbox/get", "Email/query", "Email/get", "Email/get"].map(|name| (name, json!({}))).to_vec();
    let answers = block_on(client.mail(calls)).unwrap();
    assert_eq!(answers.get(0).unwrap(), &json!({"first": true}), "the first answer to a call is its answer");
    for (call, name) in [(1, "Email/query"), (2, "Email/get"), (3, "Email/get")] {
        assert_eq!(answers.get(call).unwrap_err(), Error::Protocol(format!("JMAP: no answer to {name}")), "call {call}");
    }
    assert!(matches!(answers.get(4), Err(Error::Protocol(_))), "there is no fifth call");
}

#[test]
fn a_reply_that_is_no_list_of_answers_is_an_error() {
    for body in [json!([]), json!("x"), json!(null), json!({}), json!({"methodResponses": "x"}), json!({"methodResponses": {}}), json!({"type": "x"})]
    {
        let (client, _) = client(vec![reply(200, &body)]);
        let refused = block_on(client.mail(vec![("Mailbox/get", json!({}))]));
        assert!(matches!(refused, Err(Error::Protocol(_))), "{body}: {refused:?}");
    }
    let (client, _) = client(vec![Ok(Response { status: 200, body: b"not json".to_vec(), retry_after: None })]);
    assert!(matches!(block_on(client.mail(vec![("Mailbox/get", json!({}))])), Err(Error::Protocol(_))));
}

#[test]
fn json_nested_to_the_depth_of_a_stack_is_an_error_not_a_crash() {
    let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n)).into_bytes();
    let script = Script::new(vec![Ok(Response { status: 200, body: deep(200_000), retry_after: None })]);
    assert!(matches!(block_on(Client::connect(script, SESSION, &Auth::bearer(TOKEN))), Err(Error::Protocol(_))));
    let (connected, _) = client(vec![Ok(Response { status: 200, body: deep(200_000), retry_after: None })]);
    assert!(matches!(block_on(connected.mail(vec![("Mailbox/get", json!({}))])), Err(Error::Protocol(_))));
    // The same inside an answer: the depth the parser allows reads, one beyond it is an error.
    let within = |n: usize| format!(r#"{{"methodResponses":[["Mailbox/get",{}{},"c0"]]}}"#, "[".repeat(n), "]".repeat(n)).into_bytes();
    let (connected, _) = client(vec![Ok(Response { status: 200, body: within(100), retry_after: None })]);
    assert!(block_on(connected.mail(vec![("Mailbox/get", json!({}))])).unwrap().get(0).is_ok());
    let (connected, _) = client(vec![Ok(Response { status: 200, body: within(200_000), retry_after: None })]);
    assert!(matches!(block_on(connected.mail(vec![("Mailbox/get", json!({}))])), Err(Error::Protocol(_))));
}

#[test]
fn many_answers_are_matched_to_the_calls_in_one_pass() {
    // 200,000 answers nobody asked for, in front of the ones that were: each call still finds its own.
    let mut responses: Vec<Value> = (0..200_000).map(|n| json!(["Foo/bar", {}, format!("x{n}")])).collect();
    responses.extend((0..16).map(|n| json!(["Mailbox/get", {"n": n}, format!("c{n}")])));
    let (client, _) = client(vec![answers_to(Value::Array(responses))]);
    let calls = (0..16).map(|_| ("Mailbox/get", json!({}))).collect();
    let answers = block_on(client.mail(calls)).unwrap();
    for n in 0..16 {
        assert_eq!(answers.get(n).unwrap(), &json!({"n": n}));
    }
}

#[test]
fn only_the_ids_the_client_gave_are_answered() {
    for id in ["c0", "c1", "c12"] {
        assert_eq!(call_index(id), id.strip_prefix('c').and_then(|n| n.parse().ok()), "{id}");
    }
    for id in ["", "c", "c01", "c+1", "c-0", " c1", "c1 ", "C1", "d1", "1", "c1x", "c18446744073709551616"] {
        assert_eq!(call_index(id), None, "{id:?}");
    }
    let responses = json!([
        ["Mailbox/get", {"is": "c01"}, "c01"],
        ["Mailbox/get", {"is": "c+1"}, "c+1"],
        ["Mailbox/get", {"is": " c1"}, " c1"],
        ["Mailbox/get", {"is": "C1"}, "C1"],
        ["Mailbox/get", {"is": "c1"}, "c1"],
        ["Mailbox/get", {"is": "c0"}, "c0"]
    ]);
    let (client, _) = client(vec![answers_to(responses)]);
    let answers = block_on(client.mail(vec![("Mailbox/get", json!({})), ("Mailbox/get", json!({}))])).unwrap();
    assert_eq!((answers.get(0).unwrap(), answers.get(1).unwrap()), (&json!({"is": "c0"}), &json!({"is": "c1"})));
}

#[test]
fn a_request_over_the_calls_the_server_takes_is_not_sent() {
    let doc = mutated(|d| d["capabilities"][CORE]["maxCallsInRequest"] = json!(2));
    let script = Script::new(vec![reply(200, &doc)]);
    let client = block_on(Client::connect(script.clone(), SESSION, &Auth::bearer(TOKEN))).unwrap();
    let three = vec![("Mailbox/get", json!({})), ("Mailbox/get", json!({})), ("Mailbox/get", json!({}))];
    assert!(matches!(block_on(client.mail(three)), Err(Error::TooLarge(_))));
    assert_eq!(script.sent().len(), 1, "only the session was asked for");
    // None is no request either.
    assert!(block_on(client.mail(Vec::new())).unwrap().is_empty());
    assert_eq!(script.sent().len(), 1);
}

#[test]
fn what_a_server_echoes_of_the_credentials_never_reaches_an_error() {
    let auth = Auth::basic("ada", "hunter2");
    let header = auth.header().unwrap();
    let encoded = header.strip_prefix("Basic ").unwrap().to_string();
    let echo = |what: &str| format!("{what} {header} {encoded} hunter2");
    let clean = |text: &str| !text.contains("hunter2") && !text.contains(&encoded);

    let script = Script::new(vec![reply(400, &json!({"detail": echo("session"), "type": "hunter2"}))]);
    let shown = block_on(Client::connect(script, SESSION, &auth)).unwrap_err().to_string();
    assert!(shown.contains("***") && clean(&shown), "{shown}");

    let method_error = json!([["error", {"type": "hunter2", "description": echo("method")}, "c0"]]);
    let script = Script::new(vec![
        reply(200, &session_doc()),
        reply(400, &json!({"detail": echo("request")})),
        answers_to(method_error),
        Err(Error::Io(format!("lost {header}"))),
    ]);
    let client = block_on(Client::connect(script, SESSION, &auth)).unwrap();
    let calls = || vec![("Mailbox/get", json!({}))];
    let refused = block_on(client.mail(calls())).unwrap_err().to_string();
    assert!(refused.contains("***") && clean(&refused), "{refused}");
    let answers = block_on(client.mail(calls())).unwrap();
    let failed = answers.get(0).unwrap_err().to_string();
    assert!(failed.contains("***") && clean(&failed), "{failed}");
    let lost = block_on(client.mail(calls())).unwrap_err().to_string();
    assert!(lost.contains("lost ***") && clean(&lost), "{lost}");
}

#[test]
fn a_secret_is_hidden_before_the_text_around_it_is_folded_or_cut() {
    // Blanks that folding the server's text would have made one, or taken away.
    for password in ["correct  horse", "pass\u{a0}word", "hunter2 ", " lead", "a\u{3000}b\u{3000}\u{3000}c"] {
        let script = Script::new(vec![reply(400, &json!({"detail": format!("bad secret {password} for you")}))]);
        let shown = block_on(Client::connect(script, SESSION, &Auth::basic("ada", password))).unwrap_err().to_string();
        assert!(shown.contains("***") && shown.contains("for you"), "{password:?}: {shown}");
        for piece in password.split(char::is_whitespace).filter(|piece| piece.len() > 1) {
            assert!(!shown.contains(piece), "{password:?}: {shown}");
        }
    }
    // A token that the cut to size would have left a piece of: in what the HTTP layer says, and in what a method says.
    let token = format!("tk_{}", "0123456789".repeat(40));
    let auth = Auth::bearer(&token);
    let pieces = |text: &str| text.contains("tk_0123") || text.contains("56789");
    let at_the_cut = format!("{}{token}", "x".repeat(MAX_ERROR_TEXT - 10));
    let script = Script::new(vec![reply(400, &json!({"detail": at_the_cut}))]);
    let shown = block_on(Client::connect(script, SESSION, &auth)).unwrap_err().to_string();
    assert!(shown.contains("***") && !pieces(&shown), "{shown}");
    let kind = format!("{}{token}", "e".repeat(50));
    let described = format!("{}{token}", "d".repeat(MAX_ERROR_TEXT - 10));
    let script = Script::new(vec![reply(200, &session_doc()), answers_to(json!([["error", {"type": kind, "description": described}, "c0"]]))]);
    let client = block_on(Client::connect(script, SESSION, &auth)).unwrap();
    let answers = block_on(client.mail(vec![("Mailbox/get", json!({}))])).unwrap();
    let failed = format!("{:?}", answers.get(0).unwrap_err());
    assert!(failed.contains("***") && !pieces(&failed), "{failed}");
}

#[test]
fn an_address_the_user_or_the_session_holds_is_hidden_in_what_a_server_says() {
    let auth = Auth::basic("ada@example.org", "pw-12345");
    let said = [
        "no mailbox for ada@example.org",
        "no mailbox for ada%40example.org",
        "see https://jmap.example.org/jmap/session?account=ada%40example.org please",
        "the account=ada%40example.org part",
    ];
    for text in said {
        let script = Script::new(vec![reply(404, &json!({"detail": text}))]);
        let shown = block_on(Client::connect(script, SESSION, &auth)).unwrap_err().to_string();
        assert!(shown.contains("***") && !shown.contains("ada@") && !shown.contains("ada%40") && !shown.contains("account=ada"), "{text}: {shown}");
    }
    // At a method, and in what a transport says of the address it was given.
    let script = Script::new(vec![
        reply(200, &session_doc()),
        answers_to(json!([["error", {"type": "forbidden", "description": "ada@example.org may not read this"}, "c0"]])),
        Err(Error::Io(format!("error sending request for url ({SESSION})"))),
        Err(Error::Io("unknown user ada@example.org".into())),
    ]);
    let client = block_on(Client::connect(script, SESSION, &auth)).unwrap();
    let call = || block_on(client.mail(vec![("Mailbox/get", json!({}))]));
    assert_eq!(call().unwrap().get(0).unwrap_err().to_string(), "the server refused: forbidden *** may not read this");
    assert_eq!(call().unwrap_err().to_string(), "network: error sending request for url (***)");
    assert_eq!(call().unwrap_err().to_string(), "network: unknown user ***");
    // The user name alone, when the session's address holds no address.
    let plain = "https://jmap.example.org/jmap/session";
    let script = Script::new(vec![reply(404, &json!({"detail": "no mailbox for ada@example.org"}))]);
    let shown = block_on(Client::connect(script, plain, &auth)).unwrap_err().to_string();
    assert_eq!(shown, "the server rejected the request: HTTP 404 no mailbox for ***");
    // The address in the session's query alone, when the user name is none and no password is.
    for (text, hidden) in [
        ("no mailbox for ada@example.org", "no mailbox for ***"),
        ("no mailbox for ada%40example.org", "no mailbox for ***"),
        ("ada%40example.org and ada@example.org", "*** and ***"),
    ] {
        let script = Script::new(vec![reply(404, &json!({"detail": text}))]);
        let shown = block_on(Client::connect(script, SESSION, &Auth::bearer(TOKEN))).unwrap_err().to_string();
        assert_eq!(shown, format!("the server rejected the request: HTTP 404 {hidden}"), "{text}");
    }
    // A secret in the form an address holds it.
    for (auth, echoed) in [
        (Auth::basic("ada", "p@ss/w rd"), "retry https://x.example/?password=p%40ss%2Fw%20rd now"),
        (Auth::bearer("a/b+c=d"), "retry https://x.example/?token=a%2Fb%2Bc%3Dd now"),
    ] {
        let script = Script::new(vec![reply(400, &json!({"detail": echoed}))]);
        let shown = block_on(Client::connect(script, plain, &auth)).unwrap_err().to_string();
        assert_eq!(
            shown,
            "the server rejected the request: HTTP 400 retry https://x.example/?password=*** now"
                .replace("password", if echoed.contains("token") { "token" } else { "password" }),
            "{echoed}"
        );
    }
    // A user name that is no address, and words in a query that are none, are left alone: hiding `me`
    // would garble every message.
    let script = Script::new(vec![reply(404, &json!({"detail": "message from me about the account"}))]);
    let shown =
        block_on(Client::connect(script, "https://jmap.example.org/jmap/session?account=me&mode=plain", &Auth::basic("me", "pw-12345"))).unwrap_err();
    assert_eq!(shown.to_string(), "the server rejected the request: HTTP 404 message from me about the account");
}
#[test]
fn debug_output_shows_no_credential_address_or_mail() {
    let request = Request::post("https://jmap.example.org/api?account=ada%40example.org", "application/json", b"Subject: private".to_vec(), 10)
        .header("Authorization", "Bearer s3cr3t");
    let response = Response { status: 200, body: b"Subject: private".to_vec(), retry_after: None };
    let (client, _) = client(vec![]);
    let shown = format!("{request:?} {response:?} {client:?}");
    for secret in ["s3cr3t", "ada", "private", "account=", "/api", "3f9a7c"] {
        assert!(!shown.contains(secret), "{secret}: {shown}");
    }
    assert!(shown.contains("https://jmap.example.org") && shown.contains("Authorization"), "{shown}");
    assert!(format!("{:?}", Request::default()).contains("not a web address"));
}

#[test]
fn an_answer_can_be_asked_to_be_smaller_than_the_most_and_a_result_can_be_taken_out() {
    let responses = json!([["Email/get", {"list": [{"id": "E1"}]}, "c0"], ["Mailbox/get", {"list": []}, "c1"]]);
    let (client, script) = client(vec![answers_to(responses.clone()), answers_to(responses)]);
    let calls = || vec![("Email/get", json!({})), ("Mailbox/get", json!({}))];
    // The request says how large an answer it takes, never more than the most there is.
    let mut answers = block_on(client.mail_within(calls(), 1 << 20)).unwrap();
    assert_eq!(script.sent().get(1).map(|r| r.max), Some(1 << 20));
    // A result is taken out, once; it is not there afterwards, and a call that was not made has none.
    assert_eq!(answers.take(0).unwrap(), json!({"list": [{"id": "E1"}]}));
    assert_eq!(answers.take(0).unwrap_err(), Error::Protocol("JMAP: that answer was taken".into()));
    assert!(answers.get(0).is_err() && answers.get(1).is_ok());
    assert_eq!(answers.take(1).unwrap(), json!({"list": []}));
    assert!(matches!(answers.take(2), Err(Error::Protocol(_))));
    let mut more = block_on(client.mail_within(calls(), usize::MAX)).unwrap();
    assert_eq!(script.sent().get(2).map(|r| r.max), Some(MAX_JSON));
    assert!(more.take(0).is_ok());
    // An answer over what was asked is refused, though it is under the most.
    let (client, _) = self::client(vec![Ok(Response { status: 200, body: vec![b' '; 1001], retry_after: None })]);
    assert!(matches!(block_on(client.mail_within(calls(), 1000)), Err(Error::TooLarge(_))));
}

#[test]
fn a_reference_names_the_call_the_path_and_the_method() {
    assert_eq!(result_of(3, "Email/query", "/ids"), json!({"resultOf": "c3", "name": "Email/query", "path": "/ids"}));
}

#[test]
fn block_on_runs_a_future_that_waits() {
    struct Once(bool);
    impl std::future::Future for Once {
        type Output = u8;
        fn poll(mut self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<u8> {
            if std::mem::replace(&mut self.0, true) { std::task::Poll::Ready(7) } else { std::task::Poll::Pending }
        }
    }
    assert_eq!(block_on(Once(false)), 7);
    assert_eq!(block_on(async { 1 + 1 }), 2);
}

/// Any JSON at all, in the names and values a session and an answer deal in.
pub(crate) fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>().prop_map(|n| serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)),
        "[ -~]{0,12}".prop_map(Value::from),
        proptest::sample::select(vec![
            MAIL,
            CORE,
            "A1",
            "error",
            "c0",
            "c1",
            "Mailbox/get",
            "Email/query",
            "/jmap/api",
            "https://jmap.example.org/x/{accountId}",
            "http://jmap.example.org/x",
            "//other.example/x",
            "https://user:pw@jmap.example.org/",
            "line\r\nbreak",
        ])
        .prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        let name = proptest::sample::select(vec![
            "capabilities",
            "primaryAccounts",
            "apiUrl",
            "maxObjectsInGet",
            "maxCallsInRequest",
            "type",
            "description",
            MAIL,
            CORE,
            "x",
        ]);
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            proptest::collection::vec((name, inner), 0..6)
                .prop_map(|members| Value::Object(members.into_iter().map(|(name, value)| (name.to_string(), value)).collect())),
        ]
    })
}

/// A session with what it takes to be used in some places, and anything at all in the others.
fn session_like() -> impl Strategy<Value = Value> {
    let part = |valid: Value| prop_oneof![3 => Just(valid), 1 => any_json()];
    let capabilities = part(json!({CORE: {"maxCallsInRequest": 16, "maxObjectsInGet": 500}, MAIL: {}}));
    (capabilities, part(json!({MAIL: "A1"})), part(json!("/jmap/api")), part(json!("/d/{blobId}")), part(json!("/u/{accountId}"))).prop_map(
        |(capabilities, accounts, api, download, upload)| {
            json!({"capabilities": capabilities, "primaryAccounts": accounts, "apiUrl": api, "downloadUrl": download, "uploadUrl": upload})
        },
    )
}

/// Answers with what it takes to be read in some places, and anything at all in the others.
fn answers_like() -> impl Strategy<Value = Value> {
    let name = proptest::sample::select(vec!["Mailbox/get", "Email/query", "Email/get", "error", "other"]);
    let id = proptest::sample::select(vec!["c0", "c1", "c2", "c9", "", "x"]);
    proptest::collection::vec((name, any_json(), id), 0..6)
        .prop_map(|answers| json!({"methodResponses": answers.into_iter().map(|(name, args, id)| json!([name, args, id])).collect::<Vec<_>>()}))
}

proptest! {
    #[test]
    fn any_session_is_used_or_refused_and_never_panics(doc in prop_oneof![session_like(), any_json()], noise in proptest::collection::vec(any::<u8>(), 0..64)) {
        for body in [serde_json::to_vec(&doc).unwrap(), noise] {
            let script = Script::new(vec![Ok(Response { status: 200, body, retry_after: None })]);
            if let Ok(client) = block_on(Client::connect(script, SESSION, &Auth::bearer(TOKEN))) {
                prop_assert!((1..=1000).contains(&client.max_objects_in_get()) && (1..=64).contains(&client.max_calls_in_request()));
                prop_assert!(valid_id(client.account_id()));
                for address in [client.upload_url().ok(), client.download_url("B1", "message/rfc822", "m.eml").ok()].into_iter().flatten() {
                    prop_assert!(address.starts_with("https://jmap.example.org/") && address.bytes().all(|b| b.is_ascii_graphic()));
                    prop_assert!(address.len() <= MAX_EXPANDED);
                }
            }
        }
    }

    #[test]
    fn any_answer_is_read_or_refused_and_never_panics(answers in prop_oneof![answers_like(), any_json()], noise in proptest::collection::vec(any::<u8>(), 0..64), status in prop_oneof![Just(200u16), any::<u16>()]) {
        for body in [serde_json::to_vec(&answers).unwrap(), noise] {
            let (client, _) = client(vec![Ok(Response { status, body, retry_after: None })]);
            let calls = ["Mailbox/get", "Email/query", "Email/get"].map(|name| (name, json!({}))).to_vec();
            if let Ok(found) = block_on(client.mail(calls)) {
                prop_assert_eq!(found.len(), 3);
                for call in 0..4 {
                    match found.get(call) {
                        Ok(_) => {}
                        Err(Error::Refused { kind, description }) => prop_assert!(kind.chars().count() <= 65 && description.chars().count() <= MAX_ERROR_TEXT + 1),
                        Err(_) => {}
                    }
                }
            }
        }
    }

    #[test]
    fn any_text_is_made_into_an_address_or_refused_and_never_panics(text in ".{0,200}") {
        let address = session_url(&text);
        // It is a session address, and giving it to `session_url` again changes nothing.
        prop_assert!(address.contains("://"));
        prop_assert_eq!(session_url(&address), address.clone());
        if let Ok(url) = Url::parse(&address) {
            prop_assert!(url.full().bytes().all(|b| b.is_ascii_graphic()));
        }
        if let Ok(filled) = expand(&text, &[("a", &text), ("accountId", "A1")]) {
            prop_assert!(filled.len() <= MAX_EXPANDED);
        }
        let _ = well_formed(&text, true);
        let _ = well_formed(&text, false);
        let _ = tidy(&text, 20);
        let mut secrets = Secrets::default();
        secrets.add(&text);
        let hidden = secrets.hide(&["a", &text, "b"].join(" "));
        prop_assert!(text.is_empty() || text.contains('*') || !hidden.contains(&text));
        let _ = Auth::basic(text.clone(), text.clone()).header();
        let _ = Auth::bearer(text).header();
    }
}
