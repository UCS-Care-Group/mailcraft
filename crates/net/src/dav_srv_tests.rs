// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The records of RFC 6764: what is asked, what is taken, and hostile answers.

use proptest::prelude::*;

use super::*;
use crate::fake::{FakeHttp, json_answer};
use crate::http::Answer;

const SRV: &str = r#"{"Status":0,"Answer":[
    {"type":33,"data":"10 0 0 nothing.example.org."},
    {"type":33,"data":"20 0 8443 Backup.Example.ORG."},
    {"type":33,"data":"10 0 443 Dav.Example.ORG."}
]}"#;
const NONE: &str = r#"{"Status":3}"#;

/// A name server that answers the SRV question with `srv` and the TXT question with `txt`.
fn answering(srv: &'static str, txt: &'static str) -> FakeHttp {
    FakeHttp::start(move |_, request| json_answer("200 OK", if request.contains("type=TXT") { txt } else { srv }))
}

fn txt(data: &[&str]) -> String {
    let records: Vec<String> = data.iter().map(|data| format!(r#"{{"type":16,"data":{}}}"#, serde_json::Value::from(*data))).collect();
    format!(r#"{{"Status":0,"Answer":[{}]}}"#, records.join(","))
}

fn path_of(data: &[&str]) -> Option<String> {
    let answer = Answer { status: 200, body: txt(data).into_bytes() };
    path(&records(&answer).unwrap())
}

#[test]
fn the_place_is_the_best_target_with_the_txt_records_path_and_only_the_domain_is_asked() {
    let server = answering(SRV, r#"{"Status":0,"Answer":[{"type":16,"data":"\"path=/SOGo/dav/\""}]}"#);
    let endpoint = server.url("/dns-query");
    let found = lookup_dav_at(&[&endpoint], " Example.ORG. ", DavService::Calendars).unwrap().unwrap();
    // Every place with a port, best first: the ones behind the first are for when it can't be reached.
    let targets: Vec<(&str, u16)> = found.targets.iter().map(|target| (target.host.as_str(), target.port)).collect();
    assert_eq!(targets, [("dav.example.org", 443), ("backup.example.org", 8443)]);
    assert_eq!((found.domain.as_str(), found.path.as_deref()), ("example.org", Some("/SOGo/dav/")));
    assert!(!format!("{found:?}").contains("example") && !format!("{found:?}").contains("SOGo"));
    lookup_dav_at(&[&endpoint], "example.org", DavService::Contacts).unwrap().unwrap();
    let asked: Vec<String> = (0..server.asked()).map(|n| server.request(n)).collect();
    let lines: Vec<&str> = asked.iter().filter_map(|request| request.lines().next()).collect();
    assert_eq!(
        lines,
        [
            "GET /dns-query?name=_caldavs._tcp.example.org&type=SRV HTTP/1.1",
            "GET /dns-query?name=_caldavs._tcp.example.org&type=TXT HTTP/1.1",
            "GET /dns-query?name=_carddavs._tcp.example.org&type=SRV HTTP/1.1",
            "GET /dns-query?name=_carddavs._tcp.example.org&type=TXT HTTP/1.1",
        ]
    );
    // Questions and nothing else: no body, no cookie, nothing of who asks.
    assert!(asked.iter().all(|q| q.ends_with("\r\n\r\n") && !q.contains('@') && !q.to_lowercase().contains("cookie")), "{asked:?}");
}

#[test]
fn a_domain_without_the_record_has_no_place_and_is_asked_nothing_more() {
    for none in [
        NONE,
        r#"{"Status":0}"#,
        r#"{"Status":0,"Answer":[{"type":33,"data":"0 0 0 ."}]}"#,
        r#"{"Status":0,"Answer":[{"type":33,"data":"0 0 0 dav.example.org."}]}"#,
    ] {
        let server = answering(none, r#"{"Status":0,"Answer":[{"type":16,"data":"path=/dav"}]}"#);
        assert_eq!(lookup_dav_at(&[&server.url("/dns-query")], "example.org", DavService::Calendars), Ok(None), "{none}");
        assert_eq!(server.asked(), 1, "{none}");
    }
}

#[test]
fn a_place_without_a_path_is_still_a_place() {
    // No TXT record, one without a path, and a name server that fails at that question.
    for text in [NONE, r#"{"Status":0,"Answer":[{"type":16,"data":"\"v=spf1 -all\""}]}"#, "not json"] {
        let server = answering(SRV, text);
        let found = lookup_dav_at(&[&server.url("/dns-query")], "example.org", DavService::Contacts).unwrap().unwrap();
        assert_eq!((found.targets[0].host.as_str(), found.path), ("dav.example.org", None), "{text}");
    }
}

#[test]
fn the_next_name_server_is_asked_when_one_fails_and_a_failed_lookup_is_an_error() {
    let good = answering(SRV, r#"{"Status":0,"Answer":[{"type":16,"data":"path=/dav"}]}"#);
    let broken = FakeHttp::answering(json_answer("500 Internal Server Error", "{}"));
    let found = lookup_dav_at(&[&broken.url("/a"), &good.url("/b")], "example.org", DavService::Calendars).unwrap().unwrap();
    assert_eq!(found.path.as_deref(), Some("/dav"));
    assert!(lookup_dav_at(&[&broken.url("/a")], "example.org", DavService::Calendars).is_err());
    assert!(matches!(lookup_dav_at(&[], "example.org", DavService::Calendars), Err(NetError::Config(_))));
}

#[test]
fn only_a_domain_is_ever_asked_about() {
    let server = answering(SRV, NONE);
    let endpoint = server.url("/dns-query");
    for bad in [
        "",
        "localhost",
        "ada@example.org",
        "example.org/path",
        "example.org&type=A",
        "example.org\r\nHost: evil",
        "_x._tcp.example.org",
        "*.example.org",
    ] {
        for service in [DavService::Calendars, DavService::Contacts] {
            assert!(matches!(lookup_dav_at(&[&endpoint], bad, service), Err(NetError::Config(_))), "{bad:?}");
            assert!(matches!(lookup_dav(bad, service), Err(NetError::Config(_))), "{bad:?}");
        }
    }
    assert_eq!(server.asked(), 0);
}

#[test]
fn a_path_is_read_from_quoted_and_bare_strings() {
    assert_eq!(path_of(&[r#""path=/dav""#]).as_deref(), Some("/dav"));
    assert_eq!(path_of(&["path=/dav"]).as_deref(), Some("/dav"));
    assert_eq!(path_of(&[r#""txtvers=1" "PATH=/remote.php/dav/""#]).as_deref(), Some("/remote.php/dav/"));
    assert_eq!(path_of(&[r#""v=spf1 -all""#, r#"  "path=/cal?x=1"  "#]).as_deref(), Some("/cal?x=1"));
    // The first `path` is the one that counts (RFC 6763 §6.4), a path or not.
    assert_eq!(path_of(&[r#""path=/a" "path=/b""#, r#""path=/c""#]).as_deref(), Some("/a"));
    assert_eq!(path_of(&[r#""path=nowhere" "path=/a""#, r#""path=/b""#]), None);
    assert_eq!(path_of(&[r#""path" "path=/a""#]), None);
    assert_eq!(path_of(&[r#""Path=" "path=/a""#]), None);
    // Records of another type are not read.
    let other = Answer { status: 200, body: br#"{"Status":0,"Answer":[{"type":33,"data":"path=/dav"},{"type":16,"data":7}]}"#.to_vec() };
    assert_eq!(path(&records(&other).unwrap()), None);
}

#[test]
fn what_is_no_path_is_not_taken() {
    let long = format!("path=/{}", "a".repeat(MAX_PATH));
    for bad in [
        "",
        "path",
        "path=",
        "path=dav",
        "path=//other.example/dav",
        "path=https://other.example/dav",
        "path=/dav dir",
        "path=/dav#x",
        "path=/dav\\x",
        "path=/d\u{e4}v",
        "path=/dav\r\nHost: other.example",
        r#""path=/dav\"x""#,
        r#""path=/dav"#,
        r#""" "" "=/dav""#,
        "xpath=/dav",
        long.as_str(),
    ] {
        assert_eq!(path_of(&[bad]), None, "{bad:?}");
    }
    assert_eq!(path_of(&[&format!("path=/{}", "a".repeat(MAX_PATH - 1))]).map(|path| path.len()), Some(MAX_PATH));
}

#[test]
fn the_strings_of_a_record_are_capped() {
    let many = r#""a=b" "#.repeat(1_000);
    assert_eq!(strings(&many).len(), MAX_STRINGS);
    assert_eq!(path_of(&[&format!(r#"{many}"path=/dav""#)]), None);
}

proptest! {
    #[test]
    fn a_path_from_any_record_is_a_short_plain_one(data in prop_oneof![".{0,80}", "\"?(path|PATH|x)=[ -~]{0,300}\"?( \"[ -~]{0,20}\")?"]) {
        if let Some(path) = path_of(&[&data]) {
            prop_assert!(path.starts_with('/') && !path.starts_with("//") && path.len() <= MAX_PATH);
            prop_assert!(path.bytes().all(|b| b.is_ascii_graphic() && b != b'\\' && b != b'#' && b != b'"'), "{path}");
        }
        prop_assert!(strings(&data).len() <= MAX_STRINGS);
    }
}
