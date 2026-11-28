// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding an identity provider's addresses from its issuer, against synthetic documents.

use std::time::Duration;

use proptest::prelude::*;

use mailcraft_model::ClientAuth;

use super::discovery::{absolute, addresses, ask_public, client_auth_of, discover, may_redirect, next_hop, read};
use crate::NetError;
use crate::fake::{FakeHttp, json_answer};

const ISSUER: &str = "https://id.example.org/realms/mail";

/// What a Keycloak-shaped provider says of itself.
fn keycloak(issuer: &str) -> String {
    serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/protocol/openid-connect/auth"),
        "token_endpoint": format!("{issuer}/protocol/openid-connect/token"),
        "revocation_endpoint": format!("{issuer}/protocol/openid-connect/revoke"),
        "userinfo_endpoint": format!("{issuer}/protocol/openid-connect/userinfo"),
        "jwks_uri": format!("{issuer}/protocol/openid-connect/certs"),
        "grant_types_supported": ["authorization_code", "refresh_token", "password"],
        "code_challenge_methods_supported": ["plain", "S256"],
        "scopes_supported": ["openid", "email", "profile", "offline_access"],
        "token_endpoint_auth_methods_supported": ["private_key_jwt", "client_secret_basic", "client_secret_post"],
    })
    .to_string()
}

fn error(result: Result<super::discovery::Discovered, NetError>) -> String {
    match result {
        Err(NetError::Config(why)) => why.to_string(),
        other => panic!("expected the description to be refused, got {other:?}"),
    }
}

#[test]
fn a_keycloak_shaped_description_gives_the_addresses() {
    let found = read(ISSUER, keycloak(ISSUER).as_bytes()).unwrap();
    assert_eq!(found.authorize, format!("{ISSUER}/protocol/openid-connect/auth"));
    assert_eq!(found.token, format!("{ISSUER}/protocol/openid-connect/token"));
    assert_eq!(found.revoke, format!("{ISSUER}/protocol/openid-connect/revoke"));
    assert!(found.outside.is_empty() && !found.lacks_offline_access());
    // It lists both: the header, which every provider takes (RFC 6749 §2.3.1).
    assert_eq!(found.client_auth, ClientAuth::Basic);
    assert!(!found.issuer_in_response);
}

#[test]
fn an_authentik_shaped_one_with_its_token_on_another_host_says_so() {
    let issuer = "https://auth.example.org/application/o/mail/";
    let doc = serde_json::json!({
        "issuer": issuer,
        "authorization_endpoint": "https://auth.example.org/application/o/authorize/",
        "token_endpoint": "https://tokens.example.net/application/o/token/",
        "scopes_supported": ["openid", "email"],
        "token_endpoint_auth_methods_supported": ["client_secret_basic"],
    });
    let found = read(issuer, doc.to_string().as_bytes()).unwrap();
    assert_eq!(found.outside, ["tokens.example.net"]);
    // It lists its scopes, and offline_access isn't one: no refresh tokens.
    assert!(found.lacks_offline_access());
    assert_eq!(found.client_auth, ClientAuth::Basic);
    assert!(found.revoke.is_empty());
}

#[test]
fn a_description_naming_another_issuer_is_refused() {
    for typed in [
        "https://id.example.org/realms/other",
        "https://id.example.org/realms/mail/",
        "https://ID.example.org/realms/mail",
        " https://evil.example/realms/mail",
    ] {
        let why = error(read(typed, keycloak(ISSUER).as_bytes()));
        assert!(why.contains("another issuer"), "{typed}: {why}");
    }
}

#[test]
fn addresses_that_cant_be_used_are_refused() {
    for (field, value) in [
        ("token_endpoint", "http://id.example.org/token"),
        ("authorization_endpoint", "javascript:alert(1)"),
        ("token_endpoint", "https://user:pw@id.example.org/token"),
        ("token_endpoint", "https://id.example.org/token#frag"),
        ("authorization_endpoint", ""),
        ("revocation_endpoint", "ftp://id.example.org/revoke"),
    ] {
        let mut doc: serde_json::Value = serde_json::from_str(&keycloak(ISSUER)).unwrap();
        doc[field] = serde_json::json!(value);
        assert!(read(ISSUER, doc.to_string().as_bytes()).is_err(), "{field}={value}");
    }
    // A token address that isn't text at all is no address.
    let mut doc: serde_json::Value = serde_json::from_str(&keycloak(ISSUER)).unwrap();
    doc["token_endpoint"] = serde_json::json!({"deep": [1, 2]});
    assert!(read(ISSUER, doc.to_string().as_bytes()).is_err());
}

#[test]
fn a_provider_without_s256_or_the_code_grant_is_refused_and_one_that_doesnt_say_is_not() {
    let mut doc: serde_json::Value = serde_json::from_str(&keycloak(ISSUER)).unwrap();
    doc["code_challenge_methods_supported"] = serde_json::json!(["plain"]);
    assert!(error(read(ISSUER, doc.to_string().as_bytes())).contains("PKCE"));
    doc["code_challenge_methods_supported"] = serde_json::json!("S256");
    assert!(read(ISSUER, doc.to_string().as_bytes()).is_err());
    doc.as_object_mut().unwrap().remove("code_challenge_methods_supported");
    assert!(read(ISSUER, doc.to_string().as_bytes()).is_ok());
    doc["grant_types_supported"] = serde_json::json!(["client_credentials"]);
    assert!(read(ISSUER, doc.to_string().as_bytes()).is_err());
}

#[test]
fn what_isnt_a_description_is_refused_without_panicking() {
    for body in [&b""[..], b"null", b"[1,2]", b"{\"issuer\":", b"\xff\xfe{}", b"\"text\""] {
        assert!(read(ISSUER, body).is_err(), "{body:?}");
    }
    // Deep nesting and huge lists are capped, not followed.
    let deep = format!("{}1{}", "[".repeat(5_000), "]".repeat(5_000));
    assert!(read(ISSUER, deep.as_bytes()).is_err());
    let mut doc: serde_json::Value = serde_json::from_str(&keycloak(ISSUER)).unwrap();
    doc["scopes_supported"] = serde_json::json!((0..1_000).map(|n| format!("s{n}")).collect::<Vec<_>>());
    let found = read(ISSUER, doc.to_string().as_bytes()).unwrap();
    assert!(found.scopes_supported.len() <= 64);
    assert!(read(ISSUER, &vec![b' '; 200_000]).is_err());
}

#[test]
fn the_documents_are_looked_for_where_the_two_standards_say() {
    assert_eq!(
        addresses(ISSUER),
        [
            "https://id.example.org/realms/mail/.well-known/openid-configuration".to_string(),
            "https://id.example.org/.well-known/oauth-authorization-server/realms/mail".to_string()
        ]
    );
    assert_eq!(
        addresses("https://id.example.org/"),
        [
            "https://id.example.org/.well-known/openid-configuration".to_string(),
            "https://id.example.org/.well-known/oauth-authorization-server".to_string()
        ]
    );
}

#[test]
fn discovery_fetches_only_the_issuers_document_and_follows_redirects_it_may() {
    let issuer_of = |port: u16| format!("http://127.0.0.1:{port}/realms/mail");
    let server = FakeHttp::start(move |n, request| {
        let port = request.lines().find_map(|l| l.strip_prefix("Host: 127.0.0.1:")).and_then(|p| p.trim().parse().ok()).unwrap_or(0);
        match n {
            0 => b"HTTP/1.1 302 Found\r\nLocation: /moved\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
            _ => json_answer("200 OK", &keycloak(&issuer_of(port))),
        }
    });
    let found = discover(&issuer_of(server.port)).unwrap();
    assert_eq!(found.token, format!("{}/protocol/openid-connect/token", issuer_of(server.port)));
    let first = server.request(0);
    assert!(first.starts_with("GET /realms/mail/.well-known/openid-configuration HTTP/1.1"), "{first}");
    assert!(server.request(1).starts_with("GET /moved HTTP/1.1"));
    // Nothing of the owner goes along: no address, no client id, no cookie.
    for request in [first, server.request(1)] {
        assert!(!request.to_ascii_lowercase().contains("cookie") && !request.contains('@') && !request.contains("client"), "{request}");
    }
}

#[test]
fn the_second_standards_address_is_tried_when_the_first_has_nothing() {
    let server = FakeHttp::start(move |n, request| match n {
        0 => json_answer("404 Not Found", "{}"),
        _ => {
            let port = request.lines().find_map(|l| l.strip_prefix("Host: 127.0.0.1:")).unwrap_or("0").trim().to_string();
            json_answer("200 OK", &keycloak(&format!("http://127.0.0.1:{port}/realms/mail")))
        }
    });
    assert!(discover(&server.url("/realms/mail")).is_ok());
    assert!(server.request(1).starts_with("GET /.well-known/oauth-authorization-server/realms/mail HTTP/1.1"));
}

#[test]
fn a_redirect_to_plain_http_elsewhere_or_round_and_round_is_not_followed() {
    let away =
        FakeHttp::answering(b"HTTP/1.1 301 Moved\r\nLocation: http://id.example.org/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
    assert!(discover(&away.url("/realms/mail")).is_err());
    let round = FakeHttp::answering(b"HTTP/1.1 302 Found\r\nLocation: /again\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
    assert!(discover(&round.url("/realms/mail")).is_err());
    // Four for the first address and four for the second, no more.
    assert!(round.asked() <= 8, "{}", round.asked());
}

#[test]
fn an_issuer_that_isnt_one_is_refused_before_anything_is_fetched() {
    for issuer in ["", "http://id.example.org", "https://id.example.org/?x=1", "https://id.example.org/#f", "id.example.org"] {
        assert!(matches!(discover(issuer), Err(NetError::Config(_))), "{issuer:?}");
    }
}

proptest! {
    #[test]
    fn reading_any_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..4_000)) {
        let _ = read(ISSUER, &bytes);
    }

    #[test]
    fn reading_any_json_never_panics(keys in proptest::collection::vec(("[a-z_]{1,30}", ".{0,60}"), 0..20)) {
        let mut doc = serde_json::Map::new();
        doc.insert("issuer".into(), serde_json::json!(ISSUER));
        for (key, value) in keys {
            doc.insert(key, serde_json::json!(value));
        }
        let _ = read(ISSUER, serde_json::Value::Object(doc).to_string().as_bytes());
    }
}

#[test]
fn the_secret_goes_in_the_form_only_when_the_provider_lists_that_and_not_the_header() {
    let list = |methods: &[&str]| methods.iter().map(|m| m.to_string()).collect::<Vec<_>>();
    // RFC 8414 §2: a provider that lists nothing takes client_secret_basic.
    assert_eq!(client_auth_of(&[]), ClientAuth::Basic);
    assert_eq!(client_auth_of(&list(&["client_secret_basic", "client_secret_post"])), ClientAuth::Basic);
    assert_eq!(client_auth_of(&list(&["client_secret_post"])), ClientAuth::Post);
    assert_eq!(client_auth_of(&list(&["private_key_jwt", "client_secret_post", "none"])), ClientAuth::Post);
    assert_eq!(client_auth_of(&list(&["private_key_jwt", "none"])), ClientAuth::Basic);
    let mut doc: serde_json::Value = serde_json::from_str(&keycloak(ISSUER)).unwrap();
    doc["token_endpoint_auth_methods_supported"] = serde_json::json!(["client_secret_post"]);
    assert_eq!(read(ISSUER, doc.to_string().as_bytes()).unwrap().client_auth, ClientAuth::Post);
}

#[test]
fn a_provider_that_names_itself_in_its_answers_is_noted() {
    let mut doc: serde_json::Value = serde_json::from_str(&keycloak(ISSUER)).unwrap();
    doc["authorization_response_iss_parameter_supported"] = serde_json::json!(true);
    assert!(read(ISSUER, doc.to_string().as_bytes()).unwrap().issuer_in_response);
    // Only a true is a yes.
    doc["authorization_response_iss_parameter_supported"] = serde_json::json!("true");
    assert!(!read(ISSUER, doc.to_string().as_bytes()).unwrap().issuer_in_response);
}

/// A redirect's `Location` is resolved as a URI reference against the document it came from
/// (RFC 3986 §5.2, #1844 review).
#[test]
fn a_redirect_is_resolved_as_a_uri_reference() {
    let from = crate::http::Url::parse("https://id.example.org/realms/mail/.well-known/openid-configuration?x=1").unwrap();
    for (location, resolved) in [
        ("https://other.example.org/doc", "https://other.example.org/doc"),
        ("/moved", "https://id.example.org/moved"),
        ("metadata.json", "https://id.example.org/realms/mail/.well-known/metadata.json"),
        ("../up.json", "https://id.example.org/realms/mail/up.json"),
        ("./here.json", "https://id.example.org/realms/mail/.well-known/here.json"),
        ("//other.example.org/meta", "https://other.example.org/meta"),
        ("?y=2", "https://id.example.org/realms/mail/.well-known/openid-configuration?y=2"),
        ("/a/../../b", "https://id.example.org/b"),
        // A path whose query holds an address is a path, not an address elsewhere.
        ("/next?u=https://x.example", "https://id.example.org/next?u=https://x.example"),
        ("next?u=https://x.example", "https://id.example.org/realms/mail/.well-known/next?u=https://x.example"),
        ("HTTPS://other.example.org/doc", "HTTPS://other.example.org/doc"),
    ] {
        assert_eq!(absolute(&from, location), resolved, "{location}");
    }
}

/// Before the owner has agreed to anything, a provider on the network never sends discovery on to
/// this computer, a private network or plain `http`; an issuer on this computer (a test's) may
/// (#1844 review).
#[test]
fn a_redirect_from_the_network_to_this_computer_a_private_network_or_http_is_not_followed() {
    let url = |text: &str| crate::http::Url::parse(text).unwrap();
    let from = url("https://id.example.org/.well-known/openid-configuration");
    for next in [
        "https://127.0.0.1/x",
        "https://localhost:8443/x",
        "https://[::1]/x",
        "http://127.0.0.1:8080/x",
        "https://10.1.2.3/x",
        "https://192.168.1.10/x",
        "https://169.254.169.254/latest/meta-data",
    ] {
        assert!(!may_redirect(&from, &url(next)), "{next}");
    }
    assert!(may_redirect(&from, &url("https://login.example.net/.well-known/openid-configuration")));
    let local = url("http://127.0.0.1:9000/realms/mail");
    assert!(may_redirect(&local, &url("http://localhost:9001/x")) && may_redirect(&local, &url("https://id.example.org/x")));
}

/// A redirect's address is checked as the issuer's is: outside a test, an `https://` provider on
/// this computer can't send discovery on to plain `http://` there either (#1844 review).
#[test]
fn a_redirect_target_is_checked_as_the_issuer_is() {
    let url = |text: &str| crate::http::Url::parse(text).unwrap();
    let first = url("https://127.0.0.1:8443/realms/mail/.well-known/openid-configuration");
    assert!(matches!(next_hop(&first, &first, "http://127.0.0.1:8080/doc", false), Err(NetError::Config(_))));
    assert!(next_hop(&first, &first, "http://127.0.0.1:8080/doc", true).is_ok(), "a test's");
    assert!(next_hop(&first, &first, "https://127.0.0.1:8443/doc", false).is_ok());
    let public = url("https://id.example.org/.well-known/openid-configuration");
    assert!(next_hop(&public, &public, "https://10.1.2.3/doc", true).is_err());
    assert_eq!(next_hop(&public, &public, "/moved", false).unwrap().path, "/moved");
}

/// An address only a redirect names is asked the way a picture is: its name is looked up here, and
/// one that leads to this computer or a private network is never connected to (#1844 review).
#[test]
fn an_address_a_redirect_names_is_asked_only_on_the_public_internet() {
    let url = |text: &str| crate::http::Url::parse(text).unwrap();
    let server = FakeHttp::start(|_, _| json_answer("200 OK", "{}"));
    let at: std::net::SocketAddr = format!("127.0.0.1:{}", server.port).parse().unwrap();
    // A name that leads to a private address, or to this computer, is refused before anything is sent.
    let private = |_: &str, _: u16, _: Duration, _: &crate::pictures::Cancel| vec![std::net::SocketAddr::from(([10, 1, 2, 3], 443))];
    let reach = crate::pictures::Reach { lookup: &private, stand_ins: &[], dial: None };
    assert!(matches!(ask_public(&url("https://metadata.example.org/doc"), &reach), Err(NetError::Config(_))));
    let here = move |_: &str, _: u16, _: Duration, _: &crate::pictures::Cancel| vec![at];
    let reach = crate::pictures::Reach { lookup: &here, stand_ins: &[], dial: None };
    assert!(ask_public(&url("https://intranet.example.org/doc"), &reach).is_err());
    assert_eq!(server.asked(), 0);
    // One on the public internet (a stand-in for it here) is asked, and nothing of the owner goes along.
    let allowed = [at];
    let reach = crate::pictures::Reach { lookup: &here, stand_ins: &allowed, dial: None };
    let got = ask_public(&url(&server.url("/realms/mail/doc")), &reach).unwrap();
    assert_eq!((got.status, got.body.as_slice()), (200, b"{}".as_slice()));
    let request = server.request(0);
    assert!(request.starts_with("GET /realms/mail/doc HTTP/1.1"), "{request}");
    assert!(!request.to_ascii_lowercase().contains("cookie") && !request.contains('@'), "{request}");
}
