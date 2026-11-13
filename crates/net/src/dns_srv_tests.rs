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

/// A name server that answers the SRV question with `srv` and the alias question with `alias`.
fn answering(srv: &'static str, alias: &'static str) -> FakeHttp {
    FakeHttp::start(move |_, request| json_answer("200 OK", if request.contains("type=CNAME") { alias } else { srv }))
}

#[test]
fn the_hosts_for_autodiscover_are_the_srv_targets_then_the_alias_and_only_the_domain_is_asked() {
    let server = answering(
        r#"{"Status":0,"Answer":[{"type":33,"data":"0 0 443 autodiscover.de2.hoster.example."},{"type":33,"data":"0 0 80 autodiscover.de2.hoster.example."}]}"#,
        r#"{"Status":0,"Answer":[{"type":5,"data":"Autodiscover.Hoster.Example."}]}"#,
    );
    let hosts = autodiscover_hosts_at(&[&server.url("/dns-query")], "Example.ORG.").unwrap();
    assert_eq!(hosts, ["autodiscover.de2.hoster.example", "autodiscover.hoster.example"]);
    assert_eq!(server.asked(), 2);
    let questions = [server.request(0), server.request(1)];
    assert!(questions.iter().any(|q| q.starts_with("GET /dns-query?name=_autodiscover._tcp.example.org&type=SRV HTTP/1.1\r\n")), "{questions:?}");
    assert!(questions.iter().any(|q| q.starts_with("GET /dns-query?name=autodiscover.example.org&type=CNAME HTTP/1.1\r\n")), "{questions:?}");
    assert!(questions.iter().all(|q| q.ends_with("\r\n\r\n") && !q.contains('@') && !q.to_lowercase().contains("cookie")), "{questions:?}");
}

#[test]
fn the_search_s_targets_keep_the_srv_ports_and_take_the_alias_at_443() {
    let server = answering(
        r#"{"Status":0,"Answer":[{"type":33,"data":"0 0 8443 autodiscover.de2.hoster.example."}]}"#,
        r#"{"Status":0,"Answer":[{"type":5,"data":"autodiscover.de2.hoster.example."}]}"#,
    );
    let targets = autodiscover_targets_at(&[&server.url("/q")], "example.org").unwrap();
    let srv: Vec<(String, u16)> = targets.srv.into_iter().map(|t| (t.host, t.port)).collect();
    assert_eq!(srv, [("autodiscover.de2.hoster.example".to_string(), 8443)]);
    // The alias is kept apart: it is no SRV record's word.
    assert_eq!(targets.aliases, ["autodiscover.de2.hoster.example"]);
    // A split DNS: the public resolvers don't know the SRV record, the alias is public.
    let split = answering(r#"{"Status":3}"#, r#"{"Status":0,"Answer":[{"type":5,"data":"autodiscover.de2.hoster.example."}]}"#);
    let targets = autodiscover_targets_at(&[&split.url("/q")], "example.org").unwrap();
    assert!(targets.srv.is_empty());
    assert_eq!(targets.aliases, ["autodiscover.de2.hoster.example"]);
}

#[test]
fn an_srv_record_that_names_autodiscover_of_the_domain_itself_is_kept_and_an_alias_to_itself_is_not() {
    let server = answering(
        r#"{"Status":0,"Answer":[{"type":33,"data":"0 0 443 autodiscover.example.org."}]}"#,
        r#"{"Status":0,"Answer":[{"type":5,"data":"autodiscover.example.org."}]}"#,
    );
    let url = server.url("/q");
    let targets = autodiscover_targets_at(&[&url], "example.org").unwrap();
    assert_eq!(targets.srv.into_iter().map(|t| t.host).collect::<Vec<_>>(), ["autodiscover.example.org"]);
    assert!(targets.aliases.is_empty());
    assert_eq!(autodiscover_hosts_at(&[&url], "example.org").unwrap(), ["autodiscover.example.org"]);
}

#[test]
fn an_alias_counts_only_when_it_is_called_autodiscover_too() {
    let alias =
        |data: &str| aliases(&Answer { status: 200, body: format!(r#"{{"Status":0,"Answer":[{{"type":5,"data":"{data}"}}]}}"#).into_bytes() });
    assert_eq!(alias("autodiscover.outlook.com."), Ok(vec!["autodiscover.outlook.com".to_string()]));
    // What a wildcard gives every name, a web host, or something that isn't a name says nothing.
    for nothing in
        ["example.org.", "www.example.org.", "webhost.example.net.", "autodiscoverx.example.net.", "autodiscover", ".", "autodiscover..example.net."]
    {
        assert_eq!(alias(nothing), Ok(Vec::new()), "{nothing}");
    }
    // Only aliases are read.
    let address = aliases(&Answer { status: 200, body: br#"{"Status":0,"Answer":[{"type":1,"data":"autodiscover.example.net"}]}"#.to_vec() });
    assert_eq!(address, Ok(Vec::new()));
}

#[test]
fn a_domain_without_records_has_no_hosts_and_one_answer_is_enough() {
    let none = answering(r#"{"Status":3}"#, r#"{"Status":0}"#);
    assert_eq!(autodiscover_hosts_at(&[&none.url("/q")], "example.org"), Ok(Vec::new()));
    // The alias pointing back at the host asked about names nothing new.
    let itself = answering(r#"{"Status":0}"#, r#"{"Status":0,"Answer":[{"type":5,"data":"autodiscover.example.org."}]}"#);
    assert_eq!(autodiscover_hosts_at(&[&itself.url("/q")], "example.org"), Ok(Vec::new()));
    // The SRV question fails, the alias question is answered.
    let half = FakeHttp::start(|_, request| {
        if request.contains("type=CNAME") {
            json_answer("200 OK", r#"{"Status":0,"Answer":[{"type":5,"data":"autodiscover.hoster.example."}]}"#)
        } else {
            json_answer("500 Internal Server Error", "{}")
        }
    });
    assert_eq!(autodiscover_hosts_at(&[&half.url("/q")], "example.org"), Ok(vec!["autodiscover.hoster.example".to_string()]));
    // Neither: an error.
    let broken = FakeHttp::answering(json_answer("500 Internal Server Error", "{}"));
    assert!(autodiscover_hosts_at(&[&broken.url("/q")], "example.org").is_err());
    // Not a domain: nothing is asked.
    assert!(matches!(autodiscover_hosts_at(&[&broken.url("/q")], "ada@example.org"), Err(NetError::Config(_))));
    assert_eq!(broken.asked(), 2);
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
fn an_alias_keeps_its_room_however_many_hosts_the_srv_record_names() {
    let server = answering(
        r#"{"Status":0,"Answer":[{"type":33,"data":"1 0 443 a1.example.org."},{"type":33,"data":"2 0 443 a2.example.org."},
            {"type":33,"data":"3 0 443 a3.example.org."},{"type":33,"data":"4 0 443 a4.example.org."},
            {"type":33,"data":"5 0 443 a5.example.org."},{"type":33,"data":"6 0 443 a6.example.org."},
            {"type":33,"data":"7 0 443 a7.example.org."},{"type":33,"data":"8 0 443 a8.example.org."},
            {"type":33,"data":"9 0 443 a9.example.org."}]}"#,
        r#"{"Status":0,"Answer":[{"type":5,"data":"autodiscover.outlook.com."}]}"#,
    );
    let hosts = autodiscover_hosts_at(&[&server.url("/q")], "example.org").unwrap();
    assert_eq!(hosts.len(), MAX_HOSTS);
    assert_eq!(hosts.last().map(String::as_str), Some("autodiscover.outlook.com"), "{hosts:?}");
    assert_eq!(hosts[..MAX_HOSTS - 1], (1..MAX_HOSTS).map(|n| format!("a{n}.example.org")).collect::<Vec<_>>()[..]);
    // Without an alias the SRV record's targets have all the room.
    let srv_only = answering(
        r#"{"Status":0,"Answer":[{"type":33,"data":"1 0 443 a1.example.org."},{"type":33,"data":"2 0 443 a2.example.org."},
            {"type":33,"data":"3 0 443 a3.example.org."},{"type":33,"data":"4 0 443 a4.example.org."},
            {"type":33,"data":"5 0 443 a5.example.org."},{"type":33,"data":"6 0 443 a6.example.org."},
            {"type":33,"data":"7 0 443 a7.example.org."},{"type":33,"data":"8 0 443 a8.example.org."},
            {"type":33,"data":"9 0 443 a9.example.org."}]}"#,
        r#"{"Status":3}"#,
    );
    assert_eq!(autodiscover_hosts_at(&[&srv_only.url("/q")], "example.org").unwrap().len(), MAX_HOSTS);
}

#[test]
fn an_alias_the_srv_record_names_already_takes_no_room_from_it() {
    // Eight hosts in the record, Microsoft's last, and an alias of the first of them: all eight
    // are kept, so Microsoft's is still seen.
    let server = answering(
        r#"{"Status":0,"Answer":[{"type":33,"data":"1 0 443 autodiscover.hoster.example."},{"type":33,"data":"2 0 443 a2.example.org."},
            {"type":33,"data":"3 0 443 a3.example.org."},{"type":33,"data":"4 0 443 a4.example.org."},
            {"type":33,"data":"5 0 443 a5.example.org."},{"type":33,"data":"6 0 443 a6.example.org."},
            {"type":33,"data":"7 0 443 a7.example.org."},{"type":33,"data":"8 0 443 autodiscover.outlook.com."}]}"#,
        r#"{"Status":0,"Answer":[{"type":5,"data":"autodiscover.hoster.example."}]}"#,
    );
    let hosts = autodiscover_hosts_at(&[&server.url("/q")], "example.org").unwrap();
    assert_eq!(hosts.len(), MAX_HOSTS, "{hosts:?}");
    assert_eq!(hosts.first().map(String::as_str), Some("autodiscover.hoster.example"), "{hosts:?}");
    assert_eq!(hosts.last().map(String::as_str), Some("autodiscover.outlook.com"), "{hosts:?}");
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
    fn arbitrary_alias_answers_never_panic(status in 0u16..700, body in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = aliases(&Answer { status, body });
    }

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
