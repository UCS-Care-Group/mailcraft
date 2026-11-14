// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The autoconfig document (#1063): what is taken from it, what is preferred, and what a
//! hostile one can't do.

use proptest::prelude::*;

use super::*;
use crate::fake::FakeHttp;

/// A document as a provider publishes it, with more in it than is taken.
const DOCUMENT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="example.org">
    <domain>example.org</domain>
    <domain>example.net</domain>
    <displayName>Example Mail</displayName>
    <displayShortName>Example</displayShortName>
    <incomingServer type="pop3">
      <hostname>pop.example.org</hostname>
      <port>995</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>imap.example.org</hostname>
      <port>143</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILLOCALPART%</username>
      <authentication>password-cleartext</authentication>
    </incomingServer>
    <incomingServer type="imap">
      <hostname>Imap.Example.ORG.</hostname>
      <port>993</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
      <authentication>OAuth2</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.example.org</hostname>
      <port>587</port>
      <socketType>STARTTLS</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
    <documentation url="https://www.example.org/help">
      <descr lang="en">Settings</descr>
    </documentation>
  </emailProvider>
  <clientConfigUpdate url="https://www.example.org/config.xml"/>
</clientConfig>
"#;

fn server(host: &str, port: u16, security: Security) -> Option<Server> {
    Some(Server { host: host.into(), port, security })
}

fn document(incoming: &[&str], outgoing: &[&str]) -> String {
    let incoming: Vec<String> = incoming.iter().map(|body| format!(r#"<incomingServer type="imap">{body}</incomingServer>"#)).collect();
    let outgoing: Vec<String> = outgoing.iter().map(|body| format!(r#"<outgoingServer type="smtp">{body}</outgoingServer>"#)).collect();
    format!(
        r#"<clientConfig version="1.1"><emailProvider id="example.org">{}{}</emailProvider></clientConfig>"#,
        incoming.join(""),
        outgoing.join("")
    )
}

#[test]
fn the_imap_and_smtp_servers_are_taken_implicit_tls_first() {
    let config = read(DOCUMENT.as_bytes()).unwrap().unwrap();
    // The SSL one of the two IMAP servers, lower-cased without its dot; the POP3 one is passed over.
    assert_eq!(config.incoming, server("imap.example.org", 993, Security::Tls));
    assert_eq!(config.outgoing, server("smtp.example.org", 587, Security::StartTls));
    // The user name of the incoming server taken, as the document writes it.
    assert_eq!(config.username, "%EMAILADDRESS%");
    // An outgoing server's user name is not the account's: without an incoming server, none.
    let outgoing_only = document(&[], &["<hostname>smtp.example.org</hostname><socketType>SSL</socketType><username>%EMAILLOCALPART%</username>"]);
    let config = read(outgoing_only.as_bytes()).unwrap().unwrap();
    assert_eq!((config.incoming, config.outgoing, config.username.as_str()), (None, server("smtp.example.org", 465, Security::Tls), ""));
}

#[test]
fn a_server_without_encryption_is_never_taken() {
    let plain = document(&["<hostname>imap.example.org</hostname><port>143</port><socketType>plain</socketType>"], &[]);
    assert_eq!(read(plain.as_bytes()), Ok(None));
    // STARTTLS is taken when nothing better is listed; plain beside it changes nothing.
    let mixed = document(
        &[
            "<hostname>plain.example.org</hostname><port>143</port><socketType>plain</socketType><username>%EMAILLOCALPART%</username>",
            "<hostname>imap.example.org</hostname><port>143</port><socketType>STARTTLS</socketType><username>%EMAILLOCALPART%</username>",
        ],
        &["<hostname>smtp.example.org</hostname><socketType>plain</socketType>"],
    );
    let config = read(mixed.as_bytes()).unwrap().unwrap();
    assert_eq!(
        (config.incoming, config.outgoing, config.username.as_str()),
        (server("imap.example.org", 143, Security::StartTls), None, "%EMAILLOCALPART%")
    );
    // No socket type, or one nobody knows, is no encryption.
    for socket in ["", "<socketType></socketType>", "<socketType>TLS</socketType>", "<socketType>none</socketType>"] {
        let doc = document(&[&format!("<hostname>imap.example.org</hostname>{socket}")], &[]);
        assert_eq!(read(doc.as_bytes()), Ok(None), "{socket}");
    }
}

#[test]
fn the_usual_port_stands_in_and_a_bad_one_drops_the_server() {
    let none = document(
        &["<hostname>imap.example.org</hostname><socketType>SSL</socketType>"],
        &["<hostname>smtp.example.org</hostname><socketType>STARTTLS</socketType>"],
    );
    let config = read(none.as_bytes()).unwrap().unwrap();
    assert_eq!(
        (config.incoming, config.outgoing),
        (server("imap.example.org", 993, Security::Tls), server("smtp.example.org", 587, Security::StartTls))
    );
    for port in ["0", "70000", "-1", "993a", "9 93"] {
        let doc = document(&[&format!("<hostname>imap.example.org</hostname><port>{port}</port><socketType>SSL</socketType>")], &[]);
        assert_eq!(read(doc.as_bytes()), Ok(None), "{port}");
    }
}

#[test]
fn a_hostile_document_gives_a_host_name_or_nothing() {
    for host in [
        "evil host.example.org",
        "evil.example.org/path",
        "http://evil.example.org",
        "user@evil.example.org",
        "evil.example.org:993",
        "évil.example.org",
        "localhost",
        "",
        &"a".repeat(300),
    ] {
        let doc = document(&[&format!("<hostname>{host}</hostname><socketType>SSL</socketType>")], &[]);
        assert_eq!(read(doc.as_bytes()), Ok(None), "{host:?}");
    }
    // A reference to something that isn't a character spoils the field; a character is a character.
    let entity = document(&["<hostname>&evil;imap.example.org</hostname><socketType>SSL</socketType>"], &[]);
    assert_eq!(read(entity.as_bytes()), Ok(None));
    let char_ref = document(&["<hostname>imap&#46;example.org</hostname><socketType>SSL</socketType>"], &[]);
    assert_eq!(read(char_ref.as_bytes()).unwrap().unwrap().incoming, server("imap.example.org", 993, Security::Tls));
    // A DTD is not read; nor is a document that ends in the middle, or one with ends that don't match.
    let dtd = format!("<!DOCTYPE clientConfig [<!ENTITY evil SYSTEM \"file:///etc/passwd\">]>{}", document(&[], &[]));
    assert_eq!(read(dtd.as_bytes()), Err(Malformed));
    assert_eq!(read(b"<clientConfig><emailProvider><incomingServer type=\"imap\"><hostname>imap.example.org"), Err(Malformed));
    assert_eq!(read(b"<clientConfig><emailProvider></clientConfig></emailProvider>"), Err(Malformed));
    // Nesting without end.
    let deep = format!("{}{}", "<a>".repeat(5_000), "</a>".repeat(5_000));
    assert_eq!(read(deep.as_bytes()), Err(Malformed));
    // Servers in the wrong place are not servers; a field too long is not taken.
    let elsewhere = r#"<clientConfig><incomingServer type="imap"><hostname>imap.example.org</hostname><socketType>SSL</socketType></incomingServer></clientConfig>"#;
    assert_eq!(read(elsewhere.as_bytes()), Ok(None));
    let long =
        document(&[&format!("<hostname>imap.example.org</hostname><socketType>SSL</socketType><username>{}</username>", "u".repeat(600))], &[]);
    assert_eq!(read(long.as_bytes()).unwrap().unwrap().username, "");
    // Not XML at all.
    for garbage in ["", "not xml", "{\"hostname\":\"imap.example.org\"}", "<html><body>blocked</body></html>"] {
        assert!(matches!(read(garbage.as_bytes()), Ok(None) | Err(Malformed)), "{garbage:?}");
    }
    // Servers beyond the cap are not read; the first of a field is the one taken.
    let many: Vec<String> = (1..=100).map(|n| format!("<hostname>imap{n}.example.org</hostname><socketType>STARTTLS</socketType>")).collect();
    let mut many: Vec<&str> = many.iter().map(String::as_str).collect();
    many.push("<hostname>imap.example.org</hostname><socketType>SSL</socketType>");
    let doc = document(&many, &[]);
    assert_eq!(read(doc.as_bytes()).unwrap().unwrap().incoming, server("imap1.example.org", 143, Security::StartTls));
    let twice = document(&["<hostname>imap.example.org</hostname><hostname>evil.example.org</hostname><socketType>SSL</socketType>"], &[]);
    assert_eq!(read(twice.as_bytes()).unwrap().unwrap().incoming, server("imap.example.org", 993, Security::Tls));
}

#[test]
fn the_document_is_fetched_with_the_domain_alone_and_nothing_is_a_document_but_a_200() {
    let server = FakeHttp::answering(
        format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{DOCUMENT}", DOCUMENT.len())
            .into_bytes(),
    );
    let config = fetch(&server.url("/mail/config-v1.1.xml"), PATIENCE).unwrap().unwrap();
    assert_eq!(config.incoming.map(|s| s.host), Some("imap.example.org".to_string()));
    let request = server.request(0);
    assert!(request.starts_with("GET /mail/config-v1.1.xml HTTP/1.1\r\n"), "{request}");
    assert!(!request.contains('@') && !request.contains("emailaddress") && !request.to_lowercase().contains("cookie"), "{request}");
    assert!(request.ends_with("\r\n\r\n") && !request.to_lowercase().contains("content-length"), "{request}");
    for (status, body) in
        [("404 Not Found", DOCUMENT), ("301 Moved Permanently", DOCUMENT), ("500 Internal Server Error", ""), ("200 OK", "<html></html>")]
    {
        let server =
            FakeHttp::answering(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes());
        assert_eq!(fetch(&server.url("/x"), PATIENCE), Ok(None), "{status}");
    }
    // Only an encrypted address, or one on this machine.
    assert!(matches!(fetch("http://autoconfig.example.org/mail/config-v1.1.xml", PATIENCE), Err(NetError::Config(_))));
    assert!(matches!(fetch("not an address", PATIENCE), Err(NetError::Config(_))));
    // Nothing listens there.
    assert!(matches!(fetch("http://127.0.0.1:1/x", PATIENCE), Err(NetError::Connect { .. })));
}

proptest! {
    #[test]
    fn arbitrary_documents_never_panic(body in proptest::collection::vec(any::<u8>(), 0..400)) {
        let _ = read(&body);
    }

    #[test]
    fn document_shaped_noise_gives_host_names_or_nothing(
        servers in proptest::collection::vec(
            (
                prop_oneof![Just("imap".to_string()), Just("smtp".to_string()), Just("pop3".to_string()), "[a-z]{0,6}"],
                "[a-zA-Z0-9.-]{0,20}|[ -~&&[^\"\\\\<&]]{0,20}",
                "[0-9]{0,6}",
                prop_oneof![Just("SSL".to_string()), Just("STARTTLS".to_string()), Just("plain".to_string()), "[A-Za-z]{0,8}"],
            ),
            0..10,
        ),
    ) {
        let incoming: Vec<String> = servers.iter().map(|(kind, host, port, socket)| format!(r#"<incomingServer type="{kind}"><hostname>{host}</hostname><port>{port}</port><socketType>{socket}</socketType></incomingServer>"#)).collect();
        let body = format!(r#"<clientConfig><emailProvider>{}</emailProvider></clientConfig>"#, incoming.join(""));
        if let Ok(Some(config)) = read(body.as_bytes()) {
            let server = config.incoming.unwrap();
            let checked = domain_name(&server.host);
            prop_assert_eq!(checked.as_ref(), Some(&server.host));
            prop_assert!(server.port != 0 && server.security != Security::None);
        }
    }
}
