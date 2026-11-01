// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where a domain's mail goes: its mail exchangers (MX records, RFC 1035), from which a host
//! can tell who provides an address's mail before any account is set up.
//!
//! The standard library only looks up addresses, so the question goes over HTTPS to a public
//! name server that answers in JSON (DNS over HTTPS). Only the domain is ever sent, never a
//! mailbox name; the name server's operator gets to see it, the local network doesn't.

use std::time::Duration;

use mailcraft_model::provider::is_domain;
use mailcraft_proto::ProtoError;
use serde_json::Value;

use crate::NetError;
use crate::http::{self, Answer, Url};

/// The name servers asked, in turn: the next one only when the one before failed.
pub(crate) const RESOLVERS: [&str; 2] = ["https://cloudflare-dns.com/dns-query", "https://dns.google/resolve"];
/// Most name servers tried for one lookup.
pub(crate) const MAX_RESOLVERS: usize = 4;
/// How long a name server has to be reached, and to answer.
pub(crate) const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);
/// Most mail exchangers handed back, and most records of one answer looked at.
const MAX_HOSTS: usize = 20;
pub(crate) const MAX_RECORDS: usize = 5_000;
/// The record type of a mail exchanger.
const MX: u64 = 15;
/// What a name server says of a lookup: it went well, or there is no such domain.
pub(crate) const NO_ERROR: u64 = 0;
pub(crate) const NO_SUCH_DOMAIN: u64 = 3;

/// The domain's mail exchangers, best first: lower-cased host names without the trailing dot.
///
/// Empty when the domain doesn't exist or names no mail exchangers. The domain, and nothing
/// else, is sent over HTTPS to a public name server (Cloudflare's; Google's when that one
/// fails). Blocks, for some seconds at most per name server.
pub fn lookup_mx(domain: &str) -> Result<Vec<String>, NetError> {
    lookup_mx_at(&RESOLVERS, domain)
}

/// [`lookup_mx`], asking the name servers at `endpoints` instead: addresses that answer DNS
/// questions in JSON, tried in turn until one does. They must be `https://` addresses, or
/// `http://` ones on this machine.
pub fn lookup_mx_at(endpoints: &[&str], domain: &str) -> Result<Vec<String>, NetError> {
    // Nothing goes out for a name that isn't a domain's.
    let Some(domain) = domain_name(domain) else {
        let shown: String = domain.chars().filter(|c| !c.is_control()).take(80).collect();
        return Err(NetError::Config(format!("\"{shown}\" isn't a domain that mail could go to")));
    };
    let mut failure = None;
    for endpoint in endpoints.iter().take(MAX_RESOLVERS) {
        match ask(endpoint, &domain) {
            Ok(hosts) => return Ok(hosts),
            Err(e) => {
                log::debug!("a name server couldn't be asked: {e}");
                // The first name server's trouble is the one worth telling.
                failure.get_or_insert(e);
            }
        }
    }
    Err(failure.unwrap_or_else(|| NetError::Config("there is no name server to ask".into())))
}

/// Ask one name server for the mail exchangers of `domain` (which has been checked).
fn ask(endpoint: &str, domain: &str) -> Result<Vec<String>, NetError> {
    let endpoint = endpoint.trim();
    let question = http::form(&[("name", domain), ("type", "MX")]);
    let url = Url::parse(&format!("{endpoint}{}{question}", if endpoint.contains('?') { '&' } else { '?' }))?;
    exchangers(&http::get(&url, &[("Accept", "application/dns-json")], LOOKUP_TIMEOUT)?)
}

/// A domain or host name as it is asked for and handed back: lower-cased, without the dot it may
/// end in. `None` for anything but letters, digits and hyphens in labels with dots between them.
pub(crate) fn domain_name(text: &str) -> Option<String> {
    let name = text.trim();
    let name = name.strip_suffix('.').unwrap_or(name);
    is_domain(name).then(|| name.to_ascii_lowercase())
}

/// The mail exchangers in a name server's answer: `{"Status":0,"Answer":[{"type":15,"data":"10 mx.example.org."}]}`.
/// Whatever else is in there, and whatever isn't a host name, is passed over.
fn exchangers(answer: &Answer) -> Result<Vec<String>, NetError> {
    let nonsense = |text: String| NetError::Proto(ProtoError::Protocol(text));
    if answer.status != 200 {
        return Err(nonsense(format!("the name server answered with status {}", answer.status)));
    }
    let json = serde_json::from_slice::<Value>(&answer.body).ok().filter(Value::is_object);
    let status = json.as_ref().and_then(|json| json.get("Status")).and_then(Value::as_u64);
    let (Some(json), Some(status)) = (&json, status) else { return Err(nonsense("the name server's answer isn't the JSON it should be".into())) };
    match status {
        NO_ERROR => {}
        NO_SUCH_DOMAIN => return Ok(Vec::new()),
        other => return Err(NetError::Proto(ProtoError::Rejected(format!("the name server couldn't look the domain up (code {other})")))),
    }
    let records = json.get("Answer").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut found: Vec<(u16, String)> = Vec::new();
    for record in records.iter().take(MAX_RECORDS) {
        if record.get("type").and_then(Value::as_u64) != Some(MX) {
            continue;
        }
        // `10 mx.example.org.`: the preference (lower is better) and the host.
        let mut words = record.get("data").and_then(Value::as_str).unwrap_or("").split_whitespace();
        let (Some(preference), Some(host), None) = (words.next().and_then(|n| n.parse::<u16>().ok()), words.next(), words.next()) else { continue };
        if let Some(host) = domain_name(host) {
            found.push((preference, host));
        }
    }
    // Best first; among equals, in the order the server gave.
    found.sort_by_key(|(preference, _)| *preference);
    let mut hosts: Vec<String> = Vec::new();
    for (_, host) in found {
        if hosts.len() < MAX_HOSTS && !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::fake::{FakeHttp, json_answer};

    /// An answer as the public name servers give it, with more in it than is asked for.
    const ANSWER: &str = r#"{"Status":0,"TC":false,"RD":true,"RA":true,"AD":false,"CD":false,
        "Question":[{"name":"example.org","type":15}],
        "Answer":[
            {"name":"example.org","type":15,"TTL":300,"data":"20 Backup.Example.ORG."},
            {"name":"example.org","type":15,"TTL":300,"data":"10 mx1.example.org."},
            {"name":"example.org","type":5,"TTL":300,"data":"alias.example.net."},
            {"name":"example.org","type":15,"TTL":300,"data":"10 mx2.example.org"},
            {"name":"example.org","type":15,"TTL":300,"data":"30 mx1.example.org."},
            {"name":"example.org","type":46,"TTL":300,"data":"MX 13 2 300 20261101000000 20261001000000 1234 example.org. c2ln"}
        ],
        "Comment":"Response from 192.0.2.53."}"#;

    fn ok(body: &str) -> Vec<u8> {
        json_answer("200 OK", body)
    }

    /// What a name server's answer with this body gives.
    fn read(body: &str) -> Result<Vec<String>, NetError> {
        exchangers(&Answer { status: 200, body: body.as_bytes().to_vec() })
    }

    /// An answer whose records have these `data`.
    fn records(data: &[&str]) -> String {
        let records: Vec<String> = data.iter().map(|data| format!(r#"{{"name":"example.org","type":15,"TTL":300,"data":{data}}}"#)).collect();
        format!(r#"{{"Status":0,"Answer":[{}]}}"#, records.join(","))
    }

    #[test]
    fn mail_exchangers_come_back_best_first() {
        let server = FakeHttp::answering(ok(ANSWER));
        let hosts = lookup_mx_at(&[&server.url("/dns-query")], " Example.ORG. ").unwrap();
        // By preference, then as the server listed them; each once, in lower case, without its dot.
        assert_eq!(hosts, ["mx1.example.org", "mx2.example.org", "backup.example.org"]);
        let request = server.request(0);
        assert!(request.starts_with("GET /dns-query?name=example.org&type=MX HTTP/1.1\r\n"), "{request}");
        for header in [format!("Host: 127.0.0.1:{}\r\n", server.port), "Accept: application/dns-json\r\n".into(), "Connection: close\r\n".into()] {
            assert!(request.contains(&header), "{header:?}: {request}");
        }
        // A question and nothing else: no body, and nothing about who is asking.
        assert!(request.ends_with("\r\n\r\n") && !request.to_lowercase().contains("content-length"), "{request}");
        assert!(request.contains("User-Agent: MailCraft\r\n") && !request.to_lowercase().contains("cookie"), "{request}");

        // A name server whose address has a question of its own.
        lookup_mx_at(&[&server.url("/resolve?ct=application/dns-json")], "example.org").unwrap();
        assert!(server.request(1).starts_with("GET /resolve?ct=application/dns-json&name=example.org&type=MX HTTP/1.1\r\n"));

        // As a real one reads.
        assert_eq!(
            read(r#"{"Status":0,"Answer":[{"name":"example.org","type":15,"TTL":3600,"data":"0 example-org.mail.protection.outlook.com."}]}"#),
            Ok(vec!["example-org.mail.protection.outlook.com".to_string()])
        );
    }

    #[test]
    fn the_next_name_server_is_asked_when_one_fails() {
        let good = FakeHttp::answering(ok(ANSWER));
        // Nothing listens there. Not a port that was just free: the stand-ins below could be given
        // it, and one of them would then be asked twice (#111).
        let closed = "http://127.0.0.1:1/dns-query".to_string();
        let broken = [
            FakeHttp::answering(json_answer("500 Internal Server Error", "{}")),
            FakeHttp::answering(json_answer("302 Found", ANSWER)),
            FakeHttp::answering(ok("<html>blocked by the network</html>")),
            FakeHttp::answering(ok(r#"{"Status":2,"Comment":"server failure"}"#)),
            FakeHttp::answering(b"220 smtp.example.org ESMTP\r\n".to_vec()),
            FakeHttp::answering(Vec::new()),
        ];
        let mut firsts: Vec<String> = broken.iter().map(|server| server.url("/dns-query")).collect();
        // Ones that are never spoken to: not encrypted and not on this machine, or no address at all.
        firsts.extend([closed.clone(), "http://dns.example.org/dns-query".to_string(), "not an address".to_string(), String::new()]);
        for (n, first) in firsts.iter().enumerate() {
            let hosts = lookup_mx_at(&[first, &good.url("/resolve")], "example.org");
            assert_eq!(hosts.map(|hosts| hosts.len()), Ok(3), "{first}");
            assert_eq!(good.asked(), n + 1, "{first}");
        }
        assert!(broken.iter().all(|server| server.asked() == 1));

        // When all fail, the first one's trouble is told.
        let failed = lookup_mx_at(&[&broken[0].url("/a"), &broken[4].url("/c"), &broken[3].url("/b")], "example.org");
        assert_eq!(failed, Err(NetError::Proto(ProtoError::Protocol("the name server answered with status 500".into()))));
        let failed = lookup_mx_at(&[&broken[3].url("/b"), &broken[0].url("/a")], "example.org");
        assert_eq!(failed, Err(NetError::Proto(ProtoError::Rejected("the name server couldn't look the domain up (code 2)".into()))));
        assert!(matches!(lookup_mx_at(&[&closed], "example.org"), Err(NetError::Connect { .. })));
        assert!(matches!(lookup_mx_at(&["http://dns.example.org/dns-query"], "example.org"), Err(NetError::Config(_))));
        assert_eq!(lookup_mx_at(&[], "example.org"), Err(NetError::Config("there is no name server to ask".into())));
        // No more than a few are tried, however many are given.
        let last = good.url("/resolve");
        let many: Vec<&str> = std::iter::repeat_n("http://dns.example.org/dns-query", 50).chain([last.as_str()]).collect();
        assert!(matches!(lookup_mx_at(&many, "example.org"), Err(NetError::Config(_))));
        assert_eq!(lookup_mx_at(&many[many.len() - MAX_RESOLVERS..], "example.org").map(|hosts| hosts.len()), Ok(3));
        assert_eq!(good.asked(), firsts.len() + 1);
    }

    #[test]
    fn a_domain_without_mail_is_not_an_error() {
        // No such domain: the first name server's word is taken for it.
        let nowhere = FakeHttp::answering(ok(
            r#"{"Status":3,"Authority":[{"name":"org","type":6,"TTL":900,"data":"a0.org.example. hostmaster.example. 1 2 3 4 5"}]}"#,
        ));
        let second = FakeHttp::answering(ok(ANSWER));
        assert_eq!(lookup_mx_at(&[&nowhere.url("/q"), &second.url("/q")], "nowhere.example"), Ok(Vec::new()));
        assert_eq!((nowhere.asked(), second.asked()), (1, 0));
        // A domain that is there and names no mail exchangers.
        for none in [
            r#"{"Status":0}"#,
            r#"{"Status":0,"Answer":[]}"#,
            r#"{"Status":0,"Answer":null}"#,
            r#"{"Status":0,"Answer":{"type":15,"data":"10 mx.example.org."}}"#,
            r#"{"Status":0,"Answer":[{"name":"example.org","type":5,"TTL":300,"data":"alias.example.net."}]}"#,
            // "No mail here" (RFC 7505).
            r#"{"Status":0,"Answer":[{"name":"example.org","type":15,"TTL":300,"data":"0 ."}]}"#,
        ] {
            assert_eq!(read(none), Ok(Vec::new()), "{none}");
        }
    }

    #[test]
    fn hostile_answers_give_host_names_or_nothing() {
        let hostile = records(&[
            r#""10 evil host.example.org.""#,
            r#""10 evil.example.org/path.""#,
            r#""10 http://evil.example.org""#,
            r#""10 evil.example.org:25""#,
            r#""10 user@evil.example.org""#,
            r#""10 evil.example.org\r\nRCPT TO:<eve@example.org>""#,
            r#""10 evil\u0000.example.org""#,
            r#""10 évil.example.org""#,
            r#""10 evil_host.example.org""#,
            r#""10 localhost.""#,
            r#""10 ..example.org""#,
            r#""10 .""#,
            r#""10""#,
            r#""""#,
            r#""mx.example.org.""#,
            r#""-1 mx.example.org.""#,
            r#""70000 mx.example.org.""#,
            r#""ten mx.example.org.""#,
            r#""10 mx.example.org. extra""#,
            "10",
            "null",
            r#"["10 mx.example.org."]"#,
            r#"{"preference":10,"exchange":"mx.example.org."}"#,
            r#""5   Good.Example.ORG.  ""#,
        ]);
        assert_eq!(read(&hostile), Ok(vec!["good.example.org".to_string()]));
        // Records that aren't records, or not of the right type.
        let odd = r#"{"Status":0,"Answer":[1,"x",null,[],{"type":"15","data":"1 a.example.org."},{"type":15.5,"data":"1 b.example.org."},
            {"type":-15,"data":"1 c.example.org."},{"data":"1 d.example.org."},{"type":15},{"type":15,"data":"7 mx.example.org."}]}"#;
        assert_eq!(read(odd), Ok(vec!["mx.example.org".to_string()]));
        // Names of the longest kind, and beyond.
        let label = "a".repeat(63);
        let longest = format!("{label}.{label}.{label}.{}", "b".repeat(61));
        assert_eq!(longest.len(), 253);
        let long = records(&[&format!("\"1 {longest}.\""), &format!("\"2 x{longest}\""), &format!("\"3 {}.example.org\"", "a".repeat(64))]);
        assert_eq!(read(&long), Ok(vec![longest]));

        for garbage in [
            "",
            "not json",
            "[]",
            "null",
            "42",
            r#""Status""#,
            "{}",
            r#"{"Status":"0"}"#,
            r#"{"Status":-1}"#,
            r#"{"Status":0.5}"#,
            r#"{"Answer":[]}"#,
            "{\"Status\":0",
        ] {
            assert!(matches!(read(garbage), Err(NetError::Proto(ProtoError::Protocol(_)))), "{garbage:?}");
        }
        for failed in [1, 2, 4, 5, 9, 65_535] {
            assert!(matches!(read(&format!(r#"{{"Status":{failed}}}"#)), Err(NetError::Proto(ProtoError::Rejected(_)))), "{failed}");
        }
        let nested = format!(r#"{{"Status":0,"Answer":{}{}}}"#, "[".repeat(5_000), "]".repeat(5_000));
        assert!(matches!(read(&nested), Err(NetError::Proto(ProtoError::Protocol(_)))));
    }

    #[test]
    fn answers_with_records_without_end_are_cut_short() {
        // More than is ever handed back: the best twenty, whatever order they came in.
        let data: Vec<String> = (1..=700).rev().map(|n| format!("\"{n} mx{n}.example.org.\"")).collect();
        let many = records(&data.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(many.len() < http::MAX_BODY, "{}", many.len());
        let server = FakeHttp::answering(ok(&many));
        let hosts = lookup_mx_at(&[&server.url("/q")], "example.org").unwrap();
        assert_eq!(hosts.len(), MAX_HOSTS);
        assert_eq!(hosts, (1..=20).map(|n| format!("mx{n}.example.org")).collect::<Vec<_>>());
        // The same host five thousand times is one host.
        let same = format!(r#"{{"Status":0,"Answer":[{}]}}"#, vec![r#"{"type":15,"data":"1 a.b."}"#; 2_000].join(","));
        assert_eq!(read(&same), Ok(vec!["a.b".to_string()]));

        // More than an answer may be: refused, not read.
        let data: Vec<String> = (1..=5_000).map(|n| format!("\"{n} mx{n}.example.org.\"")).collect();
        let too_many = records(&data.iter().map(String::as_str).collect::<Vec<_>>());
        let server = FakeHttp::answering(ok(&too_many));
        let refused = lookup_mx_at(&[&server.url("/q")], "example.org");
        assert!(matches!(&refused, Err(NetError::Proto(ProtoError::Protocol(why))) if why.contains("longer than 64 KB")), "{refused:?}");
        let endless = format!("HTTP/1.1 200 OK\r\n\r\n{too_many}");
        let server = FakeHttp::answering(endless.into_bytes());
        assert!(matches!(lookup_mx_at(&[&server.url("/q")], "example.org"), Err(NetError::Proto(ProtoError::Protocol(_)))));
    }

    #[test]
    fn only_a_domain_is_ever_asked_about() {
        let server = FakeHttp::answering(ok(ANSWER));
        let endpoint = server.url("/dns-query");
        let long_label = format!("{}.example.org", "a".repeat(64));
        let long_name = format!("{}.org", "a.".repeat(126));
        for bad in [
            "",
            " ",
            ".",
            "localhost",
            "example",
            "ada@example.org",
            "Ada Lovelace <ada@example.org>",
            "example.org/path",
            "example.org?type=A",
            "example.org&type=A",
            "exa mple.org",
            "example.org:53",
            "https://example.org",
            "[2001:db8::1]",
            "exämple.org",
            "example.org\r\nHost: evil.example",
            "example.org\0",
            "example..org",
            ".example.org",
            "example.org..",
            "under_score.example.org",
            "*.example.org",
            long_label.as_str(),
            long_name.as_str(),
        ] {
            let refused = lookup_mx_at(&[&endpoint], bad);
            assert!(
                matches!(&refused, Err(NetError::Config(why)) if !why.contains('\r') && !why.contains('\0') && why.len() < 200),
                "{bad:?}: {refused:?}"
            );
            // The real name servers are no more asked than the fake one.
            assert!(matches!(lookup_mx(bad), Err(NetError::Config(_))), "{bad:?}");
        }
        assert_eq!(server.asked(), 0);
        // What is asked about is the name as a name server knows it.
        for (given, asked) in [
            ("EXAMPLE.Org", "example.org"),
            ("example.org.", "example.org"),
            ("\texample.org\r\n", "example.org"),
            ("xn--bcher-kva.example", "xn--bcher-kva.example"),
        ] {
            lookup_mx_at(&[&endpoint], given).unwrap();
            let request = server.request(server.asked() - 1);
            assert!(request.starts_with(&format!("GET /dns-query?name={asked}&type=MX HTTP/1.1\r\n")), "{given:?}: {request}");
        }
    }

    proptest! {
        #[test]
        fn arbitrary_answers_never_panic(status in 0u16..700, body in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = exchangers(&Answer { status, body });
        }

        #[test]
        fn record_shaped_noise_gives_host_names_or_nothing(
            status in prop_oneof![Just("0".to_string()), Just("3".to_string()), "[0-9]{1,3}", Just("\"0\"".to_string())],
            records in proptest::collection::vec(
                (
                    prop_oneof![Just("15".to_string()), Just("5".to_string()), Just("\"15\"".to_string()), "[0-9]{1,6}"],
                    "[0-9 ]{0,6}",
                    "[a-zA-Z0-9.-]{0,20}|[ -~&&[^\"\\\\]]{0,20}",
                ),
                0..30,
            ),
        ) {
            let records: Vec<String> = records.iter().map(|(kind, preference, host)| format!(r#"{{"type":{kind},"data":"{preference} {host}"}}"#)).collect();
            let body = format!(r#"{{"Status":{status},"Answer":[{}]}}"#, records.join(","));
            if let Ok(hosts) = read(&body) {
                prop_assert!(hosts.len() <= MAX_HOSTS);
                for (n, host) in hosts.iter().enumerate() {
                    let checked = domain_name(host);
                    prop_assert_eq!(checked.as_ref(), Some(host));
                    prop_assert!(!hosts[..n].contains(host));
                }
            }
        }

        #[test]
        fn any_text_is_a_domain_or_not(text in "\\PC{0,40}|[a-zA-Z0-9.-]{0,40}") {
            if let Some(name) = domain_name(&text) {
                prop_assert!(name.contains('.') && name.len() <= 253);
                prop_assert!(name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'));
                prop_assert_eq!(domain_name(&name), Some(name.clone()));
                // Written into an address, it is what it was.
                prop_assert_eq!(http::encode(&name), name);
            }
        }
    }
}
