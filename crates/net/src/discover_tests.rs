// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Finding a domain's servers (#1063): the exchangers first, the SRV records of RFC 6186 when
//! they name nobody known, implicit TLS before STARTTLS, and only the domain ever asked about.

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
    let found = discover_at(&[&server.url("/dns-query")], " Example.ORG. ").unwrap();
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
    let found = discover_at(&[&server.url("/dns-query")], "example.org").unwrap();
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
        let found = discover_at(&[&server.url("/dns-query")], "example.org").unwrap();
        assert_eq!(found.exchangers, ["mx.example.org"]);
        assert!(found.incoming.is_none() && found.outgoing.is_none(), "{imaps:?} {imap:?}");
        // Both IMAP services were asked for before giving up, and both for sending.
        assert_eq!(server.asked(), 5);
    }
    // A domain with no mail at all.
    let server = name_server(&[]);
    let found = discover_at(&[&server.url("/dns-query")], "nowhere.example").unwrap();
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
    let found = discover_at(&[&server.url("/dns-query")], "example.org").unwrap();
    assert_eq!(found, Discovered::from_exchangers(vec!["mx.example.org".into()]));
    // Nobody to ask: an error, as for the exchangers alone.
    assert!(matches!(discover_at(&["http://127.0.0.1:1/dns-query"], "example.org"), Err(NetError::Connect { .. })));
    assert_eq!(discover_at(&[], "example.org"), Err(NetError::Config("there is no name server to ask".into())));
}

#[test]
fn only_a_domain_is_ever_asked_about() {
    let server = name_server(&[]);
    for bad in ["", "ada@example.org", "example.org/path", "exa mple.org", "example.org\r\nHost: evil", "localhost", "_imaps._tcp.example.org"] {
        assert!(matches!(discover_at(&[&server.url("/dns-query")], bad), Err(NetError::Config(_))), "{bad:?}");
        assert!(matches!(discover(bad), Err(NetError::Config(_))), "{bad:?}");
    }
    assert_eq!(server.asked(), 0);
}
