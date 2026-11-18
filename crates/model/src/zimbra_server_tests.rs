// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

#[test]
fn an_https_address_may_be_used() {
    for good in ["https://mail.example.org", "https://mail.example.org/", "HTTPS://Mail.Example.org:8443/zimbra", "http://localhost:7070"] {
        assert_eq!(ZimbraServer::check_url(good), Ok(()), "{good}");
    }
    let server = ZimbraServer { url: "https://mail.example.org/".into(), ..Default::default() };
    assert_eq!((server.host(), server.base(), server.problem()), (Some("mail.example.org"), "https://mail.example.org", None));
}

#[test]
fn other_addresses_are_refused() {
    for bad in [
        "",
        "mail.example.org",
        "http://mail.example.org",
        "ftp://mail.example.org/",
        "https://ada:pw@mail.example.org/",
        "https://mail.example.org/a b",
        "https://mail.example.org\\x",
        "https://mail.example.org/?zauthtoken=x",
        "https://mail.example.org/#x",
        "https://mail.example.org:0/",
        "https://outlook.office365.com/",
        "https://mail.google.com/",
    ] {
        assert!(ZimbraServer::check_url(bad).is_err(), "{bad:?}");
    }
    assert!(ZimbraServer::check_url(&format!("https://example.org/{}", "a".repeat(3000))).is_err());
    let server = ZimbraServer { url: "https://mail.example.org/?x".into(), ..Default::default() };
    assert!(server.host().is_none() && server.problem().is_some());
}

#[test]
fn user_names_fingerprints_and_tokens_are_checked_as_text() {
    assert!(ZimbraServer::check_username("ada").is_ok() && ZimbraServer::check_username("ada@example.org").is_ok());
    assert!(ZimbraServer::check_username("ada\r\nX: y").is_err());
    assert!(ZimbraServer::check_fingerprint("").is_ok() && ZimbraServer::check_fingerprint(&"ab".repeat(32)).is_ok());
    assert!(ZimbraServer::check_fingerprint("ab").is_err());
    for good in ["", "1234", "1234-5678", "a1b2:12"] {
        assert!(ZimbraServer::token_ok(good), "{good}");
    }
    for bad in ["12 34", "12\n", "<x>", &"1".repeat(300)] {
        assert!(!ZimbraServer::token_ok(bad), "{bad:?}");
    }
}

#[test]
fn nothing_of_the_server_shows_in_debug_output() {
    let server = ZimbraServer {
        url: "https://mail.example.org".into(),
        username: "ada".into(),
        trusted: "ab".repeat(32),
        token: "1234".into(),
        ..ZimbraServer::default()
    };
    let shown = format!("{server:?}");
    for secret in ["mail.example.org", "ada", "abab", "1234"] {
        assert!(!shown.contains(secret), "{shown}");
    }
}
