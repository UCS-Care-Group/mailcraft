// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding an identity provider's addresses from its issuer, against synthetic documents.

use proptest::prelude::*;

use mailcraft_model::ClientAuth;

use super::discovery::{addresses, client_auth_of, discover, read};
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
