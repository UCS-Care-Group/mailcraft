// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding a domain's servers (#1063): the exchangers first, the SRV records of RFC 6186 when
//! they name nobody known, then the domain's own autoconfig document and Mozilla's database for
//! what is still missing; implicit TLS before STARTTLS, and only the domain ever asked about.

use std::collections::HashMap;

use super::*;
use crate::fake::{FakeHttp, json_answer};
use crate::http;

/// A name server that answers each `name` (with its record type, `_imaps._tcp.example.org SRV`)
/// with the given records' `data`, and "no such name" for everything else.
fn name_server(records: &[(&str, &[&str])]) -> FakeHttp {
    let records: HashMap<String, Vec<String>> =
        records.iter().map(|(name, data)| (name.to_string(), data.iter().map(|d| d.to_string()).collect())).collect();
    FakeHttp::start(move |_, request| {
        let line = request.lines().next().unwrap_or_default();
        let query = line.strip_prefix("GET /dns-query?").and_then(|rest| rest.split_once(" HTTP/")).map(|(query, _)| query).unwrap_or_default();
        let pairs = http::pairs(query);
        let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_default();
        let (name, kind) = (value("name"), value("type"));
        let record_type = if kind == "MX" { 15 } else { 33 };
        let body = match records.get(&format!("{name} {kind}")) {
            Some(data) => {
                let records: Vec<String> =
                    data.iter().map(|d| format!(r#"{{"name":"{name}","type":{record_type},"TTL":300,"data":"{d}"}}"#)).collect();
                format!(r#"{{"Status":0,"Answer":[{}]}}"#, records.join(","))
            }
            None => r#"{"Status":3}"#.to_string(),
        };
        json_answer("200 OK", &body)
    })
}

/// The name servers alone: no documents are looked for.
fn dns<'a>(resolvers: &'a [&'a str]) -> Places<'a> {
    Places { resolvers, autoconfig: &[], ispdb: None }
}

/// The questions a name server was asked: `name type`, in order.
fn asked(server: &FakeHttp) -> Vec<String> {
    (0..server.asked())
        .map(|n| {
            let request = server.request(n);
            let line = request.lines().next().unwrap_or_default();
            let query = line.strip_prefix("GET /dns-query?").and_then(|rest| rest.split_once(" HTTP/")).map(|(query, _)| query).unwrap_or_default();
            let pairs = http::pairs(query);
            let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_default();
            format!("{} {}", value("name"), value("type"))
        })
        .collect()
}

fn server_of(found: &Option<FoundServer>) -> Option<(&str, u16, Security, Via)> {
    found.as_ref().map(|f| (f.server.host.as_str(), f.server.port, f.server.security, f.via))
}

#[test]
fn implicit_tls_records_are_preferred_and_starttls_ones_fall_back() {
    let server = name_server(&[
        ("example.org MX", &["10 mx.example.org."]),
        // Implicit TLS for IMAP, best priority then highest weight; the STARTTLS record is not even asked for.
        ("_imaps._tcp.example.org SRV", &["20 0 993 backup.example.org.", "10 20 993 Imap.Example.ORG.", "10 10 9993 other.example.org."]),
        ("_imap._tcp.example.org SRV", &["0 0 143 plain.example.org."]),
        // No implicit-TLS record for sending: the STARTTLS one is taken, with STARTTLS.
        ("_submission._tcp.example.org SRV", &["0 0 587 smtp.example.org."]),
    ]);
    let found = discover_at(&dns(&[&server.url("/dns-query")]), " Example.ORG. ").unwrap();
    assert_eq!(found.exchangers, ["mx.example.org"]);
    assert_eq!(server_of(&found.incoming), Some(("imap.example.org", 993, Security::Tls, Via::Srv)));
    assert_eq!(server_of(&found.outgoing), Some(("smtp.example.org", 587, Security::StartTls, Via::Srv)));
    assert_eq!(
        asked(&server),
        ["example.org MX", "_imaps._tcp.example.org SRV", "_submissions._tcp.example.org SRV", "_submission._tcp.example.org SRV"]
    );
    // Only the domain goes out: no mailbox name, nothing of who asks.
    for n in 0..server.asked() {
        let request = server.request(n);
        assert!(!request.contains('@') && !request.to_lowercase().contains("cookie"), "{request}");
    }
    // The hosts are the organisation's own and not for a log.
    assert!(!format!("{found:?}").contains("example"));
}

#[test]
fn a_known_provider_among_the_exchangers_ends_the_search() {
    let server = name_server(&[
        ("example.org MX", &["10 mx.example.org.", "20 aspmx.l.google.com."]),
        ("_imaps._tcp.example.org SRV", &["0 0 993 imap.example.org."]),
    ]);
    let found = discover_at(&dns(&[&server.url("/dns-query")]), "example.org").unwrap();
    assert_eq!(found.exchangers, ["mx.example.org", "aspmx.l.google.com"]);
    assert!(found.incoming.is_none() && found.outgoing.is_none());
    assert_eq!(asked(&server), ["example.org MX"]);
}

#[test]
fn no_records_and_no_service_leave_nothing_found() {
    // No record at all; "not offered" (a target of `.`); a port of 0; a target that isn't a host.
    for (imaps, imap) in [
        (vec![], vec![]),
        (vec!["0 0 0 ."], vec!["0 0 0 ."]),
        (vec!["0 0 0 imap.example.org."], vec![]),
        (vec!["0 0 993 evil host.example.org."], vec!["0 0 143 http://evil.example.org"]),
    ] {
        let server = name_server(&[
            ("example.org MX", &["10 mx.example.org."]),
            ("_imaps._tcp.example.org SRV", &imaps),
            ("_imap._tcp.example.org SRV", &imap),
        ]);
        let found = discover_at(&dns(&[&server.url("/dns-query")]), "example.org").unwrap();
        assert_eq!(found.exchangers, ["mx.example.org"]);
        assert!(found.incoming.is_none() && found.outgoing.is_none(), "{imaps:?} {imap:?}");
        // Both IMAP services were asked for before giving up, and both for sending.
        assert_eq!(server.asked(), 5);
    }
    // A domain with no mail at all.
    let server = name_server(&[]);
    let found = discover_at(&dns(&[&server.url("/dns-query")]), "nowhere.example").unwrap();
    assert_eq!(found, Discovered::default());
}

#[test]
fn a_failed_srv_lookup_is_passed_over_but_an_unreachable_name_server_is_not() {
    // The name server breaks on the SRV questions: the exchangers are kept, nothing else is.
    let server = FakeHttp::start(|_, request| {
        if request.contains("type=MX") {
            json_answer("200 OK", r#"{"Status":0,"Answer":[{"type":15,"data":"10 mx.example.org."}]}"#)
        } else {
            json_answer("500 Internal Server Error", "{}")
        }
    });
    let found = discover_at(&dns(&[&server.url("/dns-query")]), "example.org").unwrap();
    assert_eq!(found, Discovered::from_exchangers(vec!["mx.example.org".into()]));
    // Nobody to ask: an error, as for the exchangers alone.
    assert!(matches!(discover_at(&dns(&["http://127.0.0.1:1/dns-query"]), "example.org"), Err(NetError::Connect { .. })));
    assert_eq!(discover_at(&dns(&[]), "example.org"), Err(NetError::Config("there is no name server to ask".into())));
}

#[test]
fn only_a_domain_is_ever_asked_about() {
    let server = name_server(&[]);
    for bad in ["", "ada@example.org", "example.org/path", "exa mple.org", "example.org\r\nHost: evil", "localhost", "_imaps._tcp.example.org"] {
        assert!(matches!(discover_at(&dns(&[&server.url("/dns-query")]), bad), Err(NetError::Config(_))), "{bad:?}");
        assert!(matches!(discover(bad), Err(NetError::Config(_))), "{bad:?}");
    }
    assert_eq!(server.asked(), 0);
}

/// An autoconfig document naming these servers (`host port socketType`), with a user name.
fn document(imap: Option<&str>, smtp: Option<&str>, username: &str) -> String {
    let server = |kind: &str, server: Option<&str>| {
        server
            .map(|server| {
                let mut words = server.split_whitespace();
                let (host, port, socket) = (words.next().unwrap_or_default(), words.next().unwrap_or_default(), words.next().unwrap_or_default());
                let element = if kind == "imap" { "incomingServer" } else { "outgoingServer" };
                format!(r#"<{element} type="{kind}"><hostname>{host}</hostname><port>{port}</port><socketType>{socket}</socketType><username>{username}</username></{element}>"#)
            })
            .unwrap_or_default()
    };
    format!(r#"<clientConfig version="1.1"><emailProvider id="x">{}{}</emailProvider></clientConfig>"#, server("imap", imap), server("smtp", smtp))
}

/// One fake server standing for the name server (at `/dns-query`), the domain's own autoconfig
/// hosts (at `/autoconfig/<domain>` and `/well-known/<domain>`) and the ISP database (at
/// `/ispdb/<domain>`): the documents are served at the paths given, nothing at any other.
fn world(records: &[(&str, &[&str])], documents: &[(&str, &str)]) -> FakeHttp {
    let records: HashMap<String, Vec<String>> =
        records.iter().map(|(name, data)| (name.to_string(), data.iter().map(|d| d.to_string()).collect())).collect();
    let documents: HashMap<String, String> = documents.iter().map(|(path, body)| (path.to_string(), body.to_string())).collect();
    FakeHttp::start(move |_, request| {
        let line = request.lines().next().unwrap_or_default();
        let path = line.strip_prefix("GET ").and_then(|rest| rest.split_once(" HTTP/")).map(|(path, _)| path).unwrap_or_default();
        if let Some(query) = path.strip_prefix("/dns-query?") {
            let pairs = http::pairs(query);
            let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_default();
            let (name, kind) = (value("name"), value("type"));
            let record_type = if kind == "MX" { 15 } else { 33 };
            let body = match records.get(&format!("{name} {kind}")) {
                Some(data) => {
                    let records: Vec<String> =
                        data.iter().map(|d| format!(r#"{{"name":"{name}","type":{record_type},"TTL":300,"data":"{d}"}}"#)).collect();
                    format!(r#"{{"Status":0,"Answer":[{}]}}"#, records.join(","))
                }
                None => r#"{"Status":3}"#.to_string(),
            };
            return json_answer("200 OK", &body);
        }
        match documents.get(path) {
            Some(body) => {
                format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
            }
            None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        }
    })
}

/// The paths asked of a [`world`], in order, the name server's questions as `name type`.
fn paths(server: &FakeHttp) -> Vec<String> {
    (0..server.asked())
        .map(|n| {
            let request = server.request(n);
            let line = request.lines().next().unwrap_or_default();
            let path = line.strip_prefix("GET ").and_then(|rest| rest.split_once(" HTTP/")).map(|(path, _)| path).unwrap_or_default();
            match path.strip_prefix("/dns-query?") {
                Some(query) => {
                    let pairs = http::pairs(query);
                    let value = |key: &str| pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_default();
                    format!("{} {}", value("name"), value("type"))
                }
                None => path.to_string(),
            }
        })
        .collect()
}

/// [`Places`] at a [`world`].
fn places_at(server: &FakeHttp) -> (String, String, String, String) {
    (server.url("/dns-query"), server.url("/autoconfig/{domain}"), server.url("/well-known/{domain}"), server.url("/ispdb/"))
}

#[test]
fn the_domains_own_document_is_read_after_the_dns_and_the_database_only_for_what_is_left() {
    // The SRV records name the IMAP server; the domain's document names both, and its SMTP
    // server is taken; the database is never asked. The document's user name is its own IMAP
    // server's, which wasn't taken: the address stays the user name (RFC 6186).
    let server = world(
        &[("example.org MX", &["10 mx.example.org."]), ("_imaps._tcp.example.org SRV", &["0 0 993 imap.example.org."])],
        &[("/autoconfig/example.org", &document(Some("other.example.org 993 SSL"), Some("smtp.example.org 465 SSL"), "%EMAILLOCALPART%"))],
    );
    let (dns, autoconfig, well_known, ispdb) = places_at(&server);
    let places = Places { resolvers: &[&dns], autoconfig: &[&autoconfig, &well_known], ispdb: Some(&ispdb) };
    let found = discover_at(&places, "example.org").unwrap();
    assert_eq!(server_of(&found.incoming), Some(("imap.example.org", 993, Security::Tls, Via::Srv)));
    assert_eq!(server_of(&found.outgoing), Some(("smtp.example.org", 465, Security::Tls, Via::Autoconfig)));
    assert_eq!(found.username, "");
    assert_eq!(
        paths(&server),
        [
            "example.org MX",
            "_imaps._tcp.example.org SRV",
            "_submissions._tcp.example.org SRV",
            "_submission._tcp.example.org SRV",
            "/autoconfig/example.org"
        ]
    );
    // The well-known place when the autoconfig host has nothing; the database when neither has.
    let server = world(
        &[("example.org MX", &["10 mx.example.org."])],
        &[("/well-known/example.org", &document(Some("imap.example.org 143 STARTTLS"), None, "%EMAILADDRESS%"))],
    );
    let (dns, autoconfig, well_known, ispdb) = places_at(&server);
    let places = Places { resolvers: &[&dns], autoconfig: &[&autoconfig, &well_known], ispdb: Some(&ispdb) };
    let found = discover_at(&places, "example.org").unwrap();
    assert_eq!(server_of(&found.incoming), Some(("imap.example.org", 143, Security::StartTls, Via::Autoconfig)));
    assert!(found.outgoing.is_none());
    assert_eq!(found.username, "%EMAILADDRESS%");
    assert_eq!(&paths(&server)[5..], ["/autoconfig/example.org", "/well-known/example.org", "/ispdb/example.org"]);
    // (The exchanger is in the domain: no other domain to ask the database about.)
    assert_eq!(server.asked(), 8);
}

#[test]
fn a_user_name_goes_only_with_the_incoming_server_of_its_own_document() {
    // The domain's document names the IMAP server and no user name (so the address); the
    // database names the SMTP server with one. The second must not replace the first's.
    let server = world(
        &[("example.org MX", &["10 mx.example.org."])],
        &[
            ("/autoconfig/example.org", &document(Some("imap.example.org 993 SSL"), None, "")),
            ("/ispdb/example.org", &document(Some("imap.other.example 993 SSL"), Some("smtp.example.org 465 SSL"), "%EMAILLOCALPART%")),
        ],
    );
    let (dns, autoconfig, well_known, ispdb) = places_at(&server);
    let places = Places { resolvers: &[&dns], autoconfig: &[&autoconfig, &well_known], ispdb: Some(&ispdb) };
    let found = discover_at(&places, "example.org").unwrap();
    assert_eq!(server_of(&found.incoming), Some(("imap.example.org", 993, Security::Tls, Via::Autoconfig)));
    assert_eq!(server_of(&found.outgoing), Some(("smtp.example.org", 465, Security::Tls, Via::Ispdb)));
    assert_eq!(found.username, "");
    // And the incoming server's user name is the one taken with it, whoever names the other.
    let server = world(
        &[("example.org MX", &["10 mx.example.org."]), ("_submissions._tcp.example.org SRV", &["0 0 465 smtp.example.org."])],
        &[("/autoconfig/example.org", &document(Some("imap.example.org 993 SSL"), Some("other.example.org 465 SSL"), "%EMAILLOCALPART%"))],
    );
    let (dns, autoconfig, well_known, ispdb) = places_at(&server);
    let places = Places { resolvers: &[&dns], autoconfig: &[&autoconfig, &well_known], ispdb: Some(&ispdb) };
    let found = discover_at(&places, "example.org").unwrap();
    assert_eq!(server_of(&found.incoming), Some(("imap.example.org", 993, Security::Tls, Via::Autoconfig)));
    assert_eq!(server_of(&found.outgoing), Some(("smtp.example.org", 465, Security::Tls, Via::Srv)));
    assert_eq!(found.username, "%EMAILLOCALPART%");
}

#[test]
fn the_database_is_asked_about_the_domain_and_then_about_its_exchangers_domain() {
    let server = world(
        &[("example.org MX", &["10 mx1.mail.hosting.example.", "20 mx2.mail.hosting.example."])],
        &[("/ispdb/hosting.example", &document(Some("imap.hosting.example 993 SSL"), Some("smtp.hosting.example 587 STARTTLS"), "%EMAILADDRESS%"))],
    );
    let (dns, autoconfig, well_known, ispdb) = places_at(&server);
    let places = Places { resolvers: &[&dns], autoconfig: &[&autoconfig, &well_known], ispdb: Some(&ispdb) };
    let found = discover_at(&places, "example.org").unwrap();
    assert_eq!(server_of(&found.incoming), Some(("imap.hosting.example", 993, Security::Tls, Via::Ispdb)));
    assert_eq!(server_of(&found.outgoing), Some(("smtp.hosting.example", 587, Security::StartTls, Via::Ispdb)));
    assert_eq!(found.username, "%EMAILADDRESS%");
    assert_eq!(&paths(&server)[5..], ["/autoconfig/example.org", "/well-known/example.org", "/ispdb/example.org", "/ispdb/hosting.example"]);
    // Only the domain is in any request: no mailbox name, nothing of who asks.
    for n in 0..server.asked() {
        let request = server.request(n);
        assert!(!request.contains('@') && !request.contains("emailaddress") && !request.to_lowercase().contains("cookie"), "{request}");
    }
    // Under a suffix anybody has a name under, the name under it is the domain asked about.
    assert_eq!(base_domain("mx1.mail.hosting.co.uk").as_deref(), Some("hosting.co.uk"));
    assert_eq!(base_domain("mx.hosting.example").as_deref(), Some("hosting.example"));
    assert_eq!(base_domain("hosting.example").as_deref(), Some("hosting.example"));
    assert_eq!(base_domain("example"), None);
    assert_eq!(base_domain("co.uk"), None);
}

#[test]
fn a_document_without_encryption_or_with_nothing_in_it_finds_nothing() {
    let server = world(
        &[("example.org MX", &["10 mx.example.org."])],
        &[
            ("/autoconfig/example.org", &document(Some("imap.example.org 143 plain"), Some("smtp.example.org 25 plain"), "%EMAILADDRESS%")),
            ("/well-known/example.org", "<html>not a document</html>"),
            ("/ispdb/example.org", "<clientConfig><emailProvider></emailProvider></clientConfig>"),
        ],
    );
    let (dns, autoconfig, well_known, ispdb) = places_at(&server);
    let places = Places { resolvers: &[&dns], autoconfig: &[&autoconfig, &well_known], ispdb: Some(&ispdb) };
    let found = discover_at(&places, "example.org").unwrap();
    assert_eq!(found, Discovered::from_exchangers(vec!["mx.example.org".into()]));
    assert_eq!(server.asked(), 8);
    // A document host that can't be reached is passed over.
    let server =
        world(&[("example.org MX", &["10 mx.example.org."])], &[("/ispdb/example.org", &document(Some("imap.example.org 993 SSL"), None, ""))]);
    let (dns, _, _, ispdb) = places_at(&server);
    let places = Places { resolvers: &[&dns], autoconfig: &["http://127.0.0.1:1/{domain}"], ispdb: Some(&ispdb) };
    let found = discover_at(&places, "example.org").unwrap();
    assert_eq!(server_of(&found.incoming), Some(("imap.example.org", 993, Security::Tls, Via::Ispdb)));
    // The real places are encrypted ones, the domain's own and Mozilla's.
    assert!(PLACES.autoconfig.iter().all(|place| place.starts_with("https://") && place.contains("{domain}")));
    assert!(PLACES.ispdb.is_some_and(|place| place.starts_with("https://autoconfig.thunderbird.net/")));
}
