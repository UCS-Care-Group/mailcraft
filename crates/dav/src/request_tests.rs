// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;
use crate::href::TrustedOrigin;
use crate::http::Method;

const COLL: &str = "https://dav.example.org/cal/owner/work/";

fn ep() -> Endpoint {
    Endpoint::new(TrustedOrigin::from_account_settings("https://dav.example.org").unwrap(), Some(Secret::basic("u", "p").unwrap()))
}
fn body(r: &HttpRequest) -> String {
    String::from_utf8(r.body().to_vec()).unwrap()
}

#[test]
fn every_body_is_well_formed_xml_in_the_right_shape() {
    let e = ep();
    let start = Stamp::parse("20260101T000000Z").unwrap();
    let end = Stamp::parse("20270101T000000Z").unwrap();
    let all = [
        (e.propfind_principal("/").unwrap(), "propfind", "0"),
        (e.propfind_homes("/p/").unwrap(), "propfind", "0"),
        (e.propfind_collections("/cal/owner/").unwrap(), "propfind", "1"),
        (e.propfind_state(COLL).unwrap(), "propfind", "0"),
        (e.calendar_query(COLL, &start, &end).unwrap(), "calendar-query", "1"),
        (e.multiget(Kind::Calendar, COLL, &["/cal/owner/work/a.ics"]).unwrap(), "calendar-multiget", ""),
        (e.multiget(Kind::AddressBook, COLL, &["/cal/owner/work/a.vcf"]).unwrap(), "addressbook-multiget", ""),
        (e.sync_collection(COLL, None, Some(50)).unwrap(), "sync-collection", "0"),
    ];
    for (r, root, depth) in &all {
        let doc = crate::xml::parse_document(r.body()).unwrap();
        assert_eq!(doc.name, *root);
        assert_eq!(r.field("depth"), if depth.is_empty() { None } else { Some(*depth) }, "a multiget leaves Depth out");
        assert_eq!(r.field("authorization"), Some(Secret::basic("u", "p").unwrap().expose()));
        assert!(r.is_repeatable());
    }
    assert_eq!(all[4].0.method(), Method::Report);
    assert_eq!(all[0].0.method(), Method::Propfind);
}

#[test]
fn calendar_query_asks_for_etags_only_in_a_time_range() {
    let e = ep();
    let r = e.calendar_query(COLL, &Stamp::parse("20260101T000000Z").unwrap(), &Stamp::parse("20270101T000000Z").unwrap()).unwrap();
    let b = body(&r);
    assert!(b.contains("<d:getetag/>") && !b.contains("calendar-data"));
    assert!(b.contains("start=\"20260101T000000Z\" end=\"20270101T000000Z\""));
    assert_eq!(
        e.calendar_query(COLL, &Stamp::parse("20270101T000000Z").unwrap(), &Stamp::parse("20260101T000000Z").unwrap()).unwrap_err(),
        RequestError::Invalid
    );
}

#[test]
fn stamps_are_strict() {
    for bad in [
        "",
        "2026-01-01T00:00:00Z",
        "20260101T000000",
        "20261301T000000Z",
        "20260132T000000Z",
        "20260101T240000Z",
        "20260101T006000Z",
        "2026010AT000000Z",
        "20260101T000000Z ",
    ] {
        assert!(Stamp::parse(bad).is_err(), "{bad:?}");
    }
    assert!(Stamp::parse("20261231T235960Z").is_ok());
}

#[test]
fn multiget_is_capped_and_held_to_the_collection() {
    let e = ep();
    let many: Vec<String> = (0..=MAX_MULTIGET).map(|i| format!("/cal/owner/work/{i}.ics")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(e.multiget(Kind::Calendar, COLL, &refs).unwrap_err(), RequestError::Count);
    assert!(e.multiget(Kind::Calendar, COLL, refs.get(..MAX_MULTIGET).unwrap()).is_ok());
    assert_eq!(e.multiget(Kind::Calendar, COLL, &[]).unwrap_err(), RequestError::Count);
    for bad in ["https://evil.example.net/cal/owner/work/a.ics", "/cal/owner/other/a.ics", "/cal/owner/work/../other/a.ics", COLL] {
        assert!(e.multiget(Kind::Calendar, COLL, &[bad]).is_err(), "{bad}");
    }
}

#[test]
fn no_request_is_built_for_another_origin_so_no_credential_leaves() {
    let e = ep();
    for url in ["https://evil.example.net/", "http://dav.example.org/", "https://dav.example.org:8443/"] {
        assert!(e.propfind_principal(url).is_err(), "{url}");
        assert!(e.get(url).is_err());
        assert!(e.options(url).is_err());
    }
    assert!(e.put_change(Kind::Calendar, COLL, "https://evil.example.net/cal/owner/work/a.ics", "\"1\"", "x").is_err());
    assert!(e.delete(COLL, "/cal/owner/other/a.ics", "\"1\"").is_err());
    assert!(e.put_create(Kind::Calendar, "https://evil.example.net/cal/owner/work/", "a", "x").is_err());
}

#[test]
fn creates_name_one_segment_and_never_overwrite() {
    let e = ep();
    let (url, r) = e.put_create(Kind::Calendar, COLL, "0a1b-2c.3d", "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
    assert_eq!(url, "https://dav.example.org/cal/owner/work/0a1b-2c.3d.ics");
    assert_eq!(r.url(), url);
    assert_eq!(r.field("if-none-match"), Some("*"));
    assert!(r.field("if-match").is_none());
    assert!(!r.is_repeatable());
    assert_eq!(r.field("content-type"), Some("text/calendar; charset=utf-8"));
    for bad in ["", "..", ".x", "a/b", "a%2Fb", "a b", "a\r\nb", &"a".repeat(MAX_NAME + 1)] {
        assert_eq!(e.put_create(Kind::Calendar, COLL, bad, "x").unwrap_err(), RequestError::Invalid, "{bad:?}");
    }
    let (url, _) = e.put_create(Kind::AddressBook, "https://dav.example.org/card/owner/book/", "c1", "BEGIN:VCARD").unwrap();
    assert!(url.ends_with("/c1.vcf"));
}

#[test]
fn changes_and_deletes_carry_if_match_and_checked_etags() {
    let e = ep();
    let r = e.put_change(Kind::Calendar, COLL, "/cal/owner/work/a.ics", "W/\"abc\"", "BEGIN:VCALENDAR").unwrap();
    assert_eq!(r.field("if-match"), Some("W/\"abc\""));
    assert!(r.field("if-none-match").is_none());
    let d = e.delete(COLL, "/cal/owner/work/a.ics", "\"abc\"").unwrap();
    assert_eq!((d.method(), d.field("if-match")), (Method::Delete, Some("\"abc\"")));
    for bad in ["", &"x".repeat(MAX_TOKEN + 1), "a\r\nb"] {
        assert_eq!(e.delete(COLL, "/cal/owner/work/a.ics", bad).unwrap_err(), RequestError::Invalid);
        assert_eq!(e.put_change(Kind::Calendar, COLL, "/cal/owner/work/a.ics", bad, "x").unwrap_err(), RequestError::Invalid);
    }
    let big = "a".repeat(MAX_RESOURCE + 1);
    assert_eq!(e.put_change(Kind::Calendar, COLL, "/cal/owner/work/a.ics", "\"1\"", &big).unwrap_err(), RequestError::TooLarge);
}

#[test]
fn sync_tokens_are_escaped_and_checked_and_limits_clamped() {
    let e = ep();
    let r = e.sync_collection(COLL, Some("a<b>&\"c"), Some(u32::MAX)).unwrap();
    let doc = crate::xml::parse_document(r.body()).unwrap();
    assert_eq!(doc.value(DAV, "sync-token"), Some("a<b>&\"c"));
    assert_eq!(doc.child(DAV, "limit").unwrap().value(DAV, "nresults"), Some("1000"));
    assert!(e.sync_collection(COLL, Some(&"t".repeat(MAX_TOKEN + 1)), None).is_err());
    assert!(e.sync_collection(COLL, Some("a\u{0}b"), None).is_err());
    let first = e.sync_collection(COLL, None, None).unwrap();
    assert_eq!(crate::xml::parse_document(first.body()).unwrap().value(DAV, "sync-token"), Some(""));
}

#[test]
fn anonymous_probes_carry_no_credential_and_debug_hides_it() {
    let e = Endpoint::anonymous(Origin::parse("https://dav.example.org").unwrap());
    assert!(e.propfind_principal("/").unwrap().field("authorization").is_none());
    assert!(ep().without_credential().options("/").unwrap().field("authorization").is_none());
    assert!(!format!("{:?}", ep()).contains("Basic"));
}

#[test]
fn escape_covers_markup() {
    assert_eq!(escape("a&b<c>\"d'\u{0}\u{7}"), "a&amp;b&lt;c&gt;&quot;d&apos;");
}

#[test]
fn a_user_name_with_a_colon_makes_no_credential_and_authorization_is_endpoint_only() {
    assert_eq!(Secret::basic("a:b", "pw"), Err(crate::http::CredentialError));
    // `HttpRequest::new` is crate-private: a credential reaches a request only through an Endpoint,
    // which has checked the address. (Checked by the crate not compiling otherwise.)
    let anon = Endpoint::anonymous(Origin::parse("https://dav.example.org").unwrap());
    assert!(anon.get("/x").unwrap().field("authorization").is_none());
}
