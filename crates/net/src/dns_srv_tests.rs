// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The SRV lookup for Autodiscover: what is asked, what is taken, and hostile answers.

use proptest::prelude::*;

use super::*;
use crate::fake::{FakeHttp, json_answer};

const ANSWER: &str = r#"{"Status":0,"Question":[{"name":"_autodiscover._tcp.example.org","type":33}],
    "Answer":[
        {"name":"_autodiscover._tcp.example.org","type":33,"TTL":300,"data":"20 10 443 Backup.Example.ORG."},
        {"name":"_autodiscover._tcp.example.org","type":33,"TTL":300,"data":"10 10 443 low.example.org."},
        {"name":"_autodiscover._tcp.example.org","type":33,"TTL":300,"data":"10 60 443 high.example.org."},
        {"name":"_autodiscover._tcp.example.org","type":5,"TTL":300,"data":"alias.example.net."},
        {"name":"_autodiscover._tcp.example.org","type":33,"TTL":300,"data":"30 0 443 high.example.org."}
    ]}"#;

fn read(body: &str) -> Result<Vec<String>, NetError> {
    targets(&Answer { status: 200, body: body.as_bytes().to_vec() }).map(|all| all.into_iter().map(|target| target.host).collect())
}

fn hosts_of(result: Result<Vec<SrvTarget>, NetError>) -> Result<Vec<String>, NetError> {
    result.map(|all| all.into_iter().map(|target| target.host).collect())
}

fn records(data: &[&str]) -> String {
    let records: Vec<String> = data.iter().map(|data| format!(r#"{{"type":33,"data":{data}}}"#)).collect();
    format!(r#"{{"Status":0,"Answer":[{}]}}"#, records.join(","))
}

#[test]
fn targets_come_back_best_first_and_only_the_domain_is_asked() {
    let server = FakeHttp::answering(json_answer("200 OK", ANSWER));
    let found = lookup_autodiscover_at(&[&server.url("/dns-query")], " Example.ORG. ").unwrap();
    // The port comes with the host.
    assert_eq!(found.iter().map(|target| target.port).collect::<Vec<_>>(), [443, 443, 443]);
    assert!(!format!("{found:?}").contains("example"));
    let hosts = found.into_iter().map(|target| target.host).collect::<Vec<_>>();
    assert_eq!(hosts, ["high.example.org", "low.example.org", "backup.example.org"]);
    let request = server.request(0);
    assert!(request.starts_with("GET /dns-query?name=_autodiscover._tcp.example.org&type=SRV HTTP/1.1\r\n"), "{request}");
    // A question and nothing else: no body, no cookie, nothing of who asks.
    assert!(request.ends_with("\r\n\r\n") && !request.to_lowercase().contains("cookie") && !request.contains("@"), "{request}");
}

#[test]
fn no_record_and_no_service_are_not_errors() {
    for none in [
        r#"{"Status":3}"#,
        r#"{"Status":0}"#,
        r#"{"Status":0,"Answer":[]}"#,
        r#"{"Status":0,"Answer":{"type":33,"data":"1 1 443 a.example.org."}}"#,
        // "The service is not offered here" (RFC 2782).
        r#"{"Status":0,"Answer":[{"type":33,"data":"0 0 0 ."}]}"#,
    ] {
        assert_eq!(read(none), Ok(Vec::new()), "{none}");
    }
}

#[test]
fn the_next_name_server_is_asked_when_one_fails() {
    let good = FakeHttp::answering(json_answer("200 OK", ANSWER));
    let broken = FakeHttp::answering(json_answer("500 Internal Server Error", "{}"));
    assert_eq!(hosts_of(lookup_autodiscover_at(&[&broken.url("/a"), &good.url("/b")], "example.org")).map(|hosts| hosts.len()), Ok(3));
    let failed = lookup_autodiscover_at(&[&broken.url("/a")], "example.org");
    assert_eq!(failed, Err(NetError::Proto(ProtoError::Protocol("the name server answered with status 500".into()))));
    assert_eq!(lookup_autodiscover_at(&[], "example.org"), Err(NetError::Config("there is no name server to ask".into())));
}

#[test]
fn only_a_domain_is_ever_asked_about() {
    let server = FakeHttp::answering(json_answer("200 OK", ANSWER));
    let endpoint = server.url("/dns-query");
    for bad in [
        "",
        "localhost",
        "ada@example.org",
        "example.org/path",
        "example.org&type=A",
        "example.org\r\nHost: evil",
        "exa mple.org",
        "example.org:53",
        "exämple.org",
        "_x._tcp.example.org",
        "*.example.org",
        ".example.org",
    ] {
        assert!(matches!(lookup_autodiscover_at(&[&endpoint], bad), Err(NetError::Config(_))), "{bad:?}");
        assert!(matches!(lookup_autodiscover(bad), Err(NetError::Config(_))), "{bad:?}");
    }
    assert_eq!(server.asked(), 0);
}

#[test]
fn hostile_answers_give_host_names_or_nothing() {
    let hostile = records(&[
        r#""10 10 443 evil host.example.org.""#,
        r#""10 10 443 evil.example.org/path""#,
        r#""10 10 443 http://evil.example.org""#,
        r#""10 10 443 user@evil.example.org""#,
        r#""10 10 443 evil.example.org\r\nRCPT TO:<eve@example.org>""#,
        r#""10 10 443 évil.example.org""#,
        r#""10 10 443 localhost.""#,
        r#""10 10 443""#,
        r#""10 10 mail.example.org.""#,
        r#""10 443 mail.example.org.""#,
        r#""ten 10 443 mail.example.org.""#,
        r#""-1 10 443 mail.example.org.""#,
        r#""10 70000 443 mail.example.org.""#,
        r#""10 10 70000 mail.example.org.""#,
        r#""10 10 443 mail.example.org. extra""#,
        r#""""#,
        "10",
        "null",
        r#"["10 10 443 mail.example.org."]"#,
        r#""5   1  443   Good.Example.ORG.  ""#,
    ]);
    assert_eq!(read(&hostile), Ok(vec!["good.example.org".to_string()]));
    for garbage in ["", "not json", "[]", "null", "42", "{}", r#"{"Status":"0"}"#, r#"{"Status":-1}"#, "{\"Status\":0"] {
        assert!(matches!(read(garbage), Err(NetError::Proto(ProtoError::Protocol(_)))), "{garbage:?}");
    }
    for failed in [1, 2, 4, 5, 65_535] {
        assert!(matches!(read(&format!(r#"{{"Status":{failed}}}"#)), Err(NetError::Proto(ProtoError::Rejected(_)))), "{failed}");
    }
    let nested = format!(r#"{{"Status":0,"Answer":{}{}}}"#, "[".repeat(5_000), "]".repeat(5_000));
    assert!(matches!(read(&nested), Err(NetError::Proto(ProtoError::Protocol(_)))));
}

#[test]
fn a_port_is_kept_and_a_host_at_two_ports_is_two_targets() {
    let body =
        records(&[r#""1 1 8443 a.example.org.""#, r#""1 1 443 a.example.org.""#, r#""1 1 8443 a.example.org.""#, r#""1 1 99999 b.example.org.""#]);
    let found = targets(&Answer { status: 200, body: body.into_bytes() }).unwrap();
    assert_eq!(found.iter().map(|target| (target.host.as_str(), target.port)).collect::<Vec<_>>(), [("a.example.org", 8443), ("a.example.org", 443)]);
}

#[test]
fn the_hosts_handed_back_are_capped() {
    let data: Vec<String> = (1..=700).rev().map(|n| format!("\"{n} 1 443 srv{n}.example.org.\"")).collect();
    let many = records(&data.iter().map(String::as_str).collect::<Vec<_>>());
    let hosts = read(&many).unwrap();
    assert_eq!(hosts, (1..=MAX_HOSTS).map(|n| format!("srv{n}.example.org")).collect::<Vec<_>>());
    let same = format!(r#"{{"Status":0,"Answer":[{}]}}"#, vec![r#"{"type":33,"data":"1 1 443 a.b."}"#; 2_000].join(","));
    assert_eq!(read(&same), Ok(vec!["a.b".to_string()]));
}

proptest! {
    #[test]
    fn arbitrary_answers_never_panic(status in 0u16..700, body in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = targets(&Answer { status, body });
    }

    #[test]
    fn record_shaped_noise_gives_host_names_or_nothing(
        records in proptest::collection::vec(
            (prop_oneof![Just("33".to_string()), Just("5".to_string()), "[0-9]{1,6}"], "[0-9 ]{0,12}", "[a-zA-Z0-9.-]{0,20}|[ -~&&[^\"\\\\]]{0,20}"),
            0..30,
        ),
    ) {
        let records: Vec<String> = records.iter().map(|(kind, numbers, host)| format!(r#"{{"type":{kind},"data":"{numbers} {host}"}}"#)).collect();
        let body = format!(r#"{{"Status":0,"Answer":[{}]}}"#, records.join(","));
        if let Ok(hosts) = read(&body) {
            prop_assert!(hosts.len() <= MAX_HOSTS);
            for (n, host) in hosts.iter().enumerate() {
                let checked = domain_name(host);
                prop_assert_eq!(checked.as_ref(), Some(host));
                prop_assert!(!hosts[..n].contains(host));
            }
        }
    }
}
