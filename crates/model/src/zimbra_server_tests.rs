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
fn corporate_hosts_and_ip_addresses_need_no_provider_allowlist() {
    for address in ["https://mail.corp.example:8443/zimbra", "https://zimbra", "https://192.0.2.20:8443", "https://[2001:db8::20]:8443"] {
        assert_eq!(ZimbraServer::check_url(address), Ok(()), "{address}");
        let server = ZimbraServer { url: address.into(), ..Default::default() };
        assert!(server.host().is_some() && server.problem().is_none(), "{address}");
    }
}

#[test]
fn a_certificate_pin_does_not_relax_server_address_rules() {
    for address in [
        "http://mail.corp.example",
        "https://ada:pw@mail.corp.example",
        "https://mail.corp.example/?zauthtoken=synthetic",
        "https://mail.corp.example/#fragment",
    ] {
        let server = ZimbraServer { url: address.into(), trusted: "ab".repeat(32), ..Default::default() };
        assert!(server.problem().is_some(), "{address}");
    }
}

#[test]
fn the_same_server_is_told_whatever_the_case_of_its_name() {
    let server = ZimbraServer { url: "https://Mail.Example.org:8443/zimbra/".into(), ..Default::default() };
    // The scheme and the server's name are read in any case, and a `/` at the end names nothing.
    for same in ["https://mail.example.org:8443/zimbra", "HTTPS://MAIL.EXAMPLE.ORG:8443/zimbra/", " https://mail.example.org:8443/zimbra// "] {
        assert!(server.same_server(same), "{same}");
    }
    // A path is the server's own to tell apart, and a port or a scheme is another service.
    for other in [
        "https://mail.example.org:8443/ZIMBRA",
        "https://mail.example.org:8443",
        "https://mail.example.org/zimbra",
        "https://mail.example.org:8444/zimbra",
        "http://mail.example.org:8443/zimbra",
        "https://other.example.org:8443/zimbra",
        "https://mail.example.org:8443/zimbra/more",
        "mail.example.org:8443/zimbra",
        "",
    ] {
        assert!(!server.same_server(other), "{other:?}");
    }
    let plain = ZimbraServer { url: "https://mail.example.org".into(), ..Default::default() };
    assert!(plain.same_server("HTTPS://MAIL.EXAMPLE.ORG/") && !plain.same_server("https://mail.example.org/zimbra"));
    // The port a scheme goes to anyway names nothing either; any other port is another service.
    for (address, same, other) in [
        ("https://mail.example.org/zimbra", "HTTPS://MAIL.EXAMPLE.ORG:443/zimbra/", "https://mail.example.org:80/zimbra"),
        ("https://mail.example.org:443", "https://mail.example.org", "https://mail.example.org:4430"),
        ("http://localhost:80/zimbra", "http://LOCALHOST/zimbra", "http://localhost:443/zimbra"),
        ("https://[2001:db8::20]:443", "https://[2001:DB8::20]/", "https://[2001:db8::20]:8443"),
    ] {
        let server = ZimbraServer { url: address.into(), ..Default::default() };
        assert!(server.same_server(same) && server.same_server(address), "{address} and {same}");
        assert!(!server.same_server(other), "{address} and {other}");
    }
    // No address is no server: it is the same as none, not even as another empty one.
    assert!(!ZimbraServer::default().same_server("") && !ZimbraServer::default().same_server("https://mail.example.org"));
    // (Text of any kind is compared, never sliced inside a character.)
    assert!(!server.same_server("https://mäil.example.org:8443/zimbra") && !server.same_server("://") && !server.same_server("é://é/é"));
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
