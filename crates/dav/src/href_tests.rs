// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn o() -> Origin {
    Origin::parse("https://dav.example.org").unwrap()
}
const BASE: &str = "https://dav.example.org/cal/owner/work/";

#[test]
fn resolves_the_forms_of_an_href() {
    let o = o();
    for (href, want) in [
        ("/cal/owner/work/a.ics", "https://dav.example.org/cal/owner/work/a.ics"),
        ("a.ics", "https://dav.example.org/cal/owner/work/a.ics"),
        ("../home/", "https://dav.example.org/cal/owner/home/"),
        ("https://DAV.example.org:443/x/y", "https://dav.example.org/x/y"),
        ("//dav.example.org/x", "https://dav.example.org/x"),
        ("/a/./b//c/../d", "https://dav.example.org/a/b/d"),
        ("/a%2fb/%7euser/%41", "https://dav.example.org/a%2Fb/~user/A"),
        ("/o%40example.org/", "https://dav.example.org/o%40example.org/"),
        ("/caf\u{e9}", "https://dav.example.org/caf%C3%A9"),
    ] {
        assert_eq!(o.resolve(BASE, href).as_deref(), Ok(want), "{href}");
    }
}

#[test]
fn refuses_what_is_off_origin() {
    let o = o();
    for href in [
        "https://evil.example.net/cal/",
        "http://dav.example.org/cal/",
        "https://dav.example.org:8443/cal/",
        "https://dav.example.org.evil.net/",
        "//evil.example.net/",
        "https://owner:pw@dav.example.org/",
        "https://dav.example.org@evil.net/",
        "ftp://dav.example.org/",
    ] {
        assert!(o.resolve(BASE, href).is_err(), "{href}");
    }
    assert_eq!(o.resolve(BASE, "https://evil.example.net/"), Err(HrefError::OffOrigin));
    assert_eq!(o.resolve("https://evil.example.net/", "x"), Err(HrefError::OffOrigin));
}

#[test]
fn refuses_malformed_hrefs() {
    let o = o();
    for href in ["", "   ", "/a b", "/a\\b", "/a\nb", "/a%zz", "/a%", "/a#frag", "/../etc", "/a/../../b"] {
        assert!(o.resolve(BASE, href).is_err(), "{href:?}");
    }
    assert_eq!(o.resolve(BASE, "/a/../../b"), Err(HrefError::Escapes));
    let long = format!("/{}", "a".repeat(2_048));
    assert_eq!(o.resolve(BASE, &long), Err(HrefError::Malformed));
    let ok = format!("/{}", "a".repeat(2_000));
    assert!(o.resolve(BASE, &ok).is_ok());
}

#[test]
fn origin_parse_rules() {
    assert!(Origin::parse("https://user@dav.example.org/").is_err());
    assert!(Origin::parse("https://dav.example.org:0/").is_err());
    assert!(Origin::parse("https://dav.example.org:99999/").is_err());
    assert!(Origin::parse("gopher://x/").is_err());
    assert!(Origin::parse("https:///x").is_err());
    assert!(Origin::parse("https://[::1]:8443/x").unwrap().is_loopback());
    assert!(Origin::parse("http://127.0.0.1:8080").unwrap().is_loopback());
    assert!(!Origin::parse("http://127.0.0.1.evil.net").unwrap().is_loopback());
    assert_eq!(Origin::parse("HTTPS://Dav.Example.ORG:443/x").unwrap(), o());
    assert_eq!(format!("{:?}", o()), "Origin(..)");
}

#[test]
fn within_means_below_and_never_equal() {
    let o = o();
    assert!(o.is_within(BASE, "/cal/owner/work/a.ics"));
    assert!(o.is_within(BASE, "https://dav.example.org/cal/owner/work/sub/a.ics"));
    assert!(!o.is_within(BASE, BASE));
    assert!(!o.is_within(BASE, "/cal/owner/work2/a.ics"));
    assert!(!o.is_within(BASE, "/cal/owner/work/../other/a.ics"));
    assert!(!o.is_within(BASE, "https://evil.example.net/cal/owner/work/a.ics"));
}
