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
        ("/a/./b//c/../d", "https://dav.example.org/a/b//d"),
        ("/a//b/", "https://dav.example.org/a//b/"),
        ("/a%2db/%7euser/%41", "https://dav.example.org/a-b/~user/A"),
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

#[test]
fn encoded_separators_and_dot_segments_with_parameters_are_refused() {
    let o = o();
    for href in [
        "/cal/owner/work/..%2F..%2Fother/a.ics",
        "/cal/owner/work/..%2f..%2fother/a.ics",
        "/cal/owner/work/a%5Cb",
        "/cal/owner/work/a%5cb",
        "/cal/owner/work/a%00b",
        "/cal/owner/work/%2e%2e%2Fx",
    ] {
        assert_eq!(o.resolve(BASE, href), Err(HrefError::Malformed), "{href}");
        assert!(!o.is_within(BASE, href), "{href}");
    }
    for href in ["/cal/owner/work/..;/..;/other/a.ics", "/cal/owner/work/.;x/a.ics", "/cal/owner/work/..;a=b"] {
        assert_eq!(o.resolve(BASE, href), Err(HrefError::Escapes), "{href}");
        assert!(!o.is_within(BASE, href), "{href}");
    }
    // A semicolon elsewhere is an ordinary character, and an encoded slash in a query is left alone.
    assert!(o.resolve(BASE, "a;b.ics").is_ok());
    assert!(o.resolve(BASE, "a.ics?x=%2F").is_ok());
}

#[test]
fn a_scheme_without_slashes_is_not_a_path() {
    let o = o();
    for href in ["mailto:x@example.org", "https:dav.example.org/x", "javascript:alert(1)", "a:b.ics", "tel:+1"] {
        assert_eq!(o.resolve(BASE, href), Err(HrefError::Malformed), "{href}");
    }
    assert!(o.resolve(BASE, "./a:b.ics").is_ok());
    assert!(o.resolve(BASE, "a.ics?x=a:b").is_ok());
}

#[test]
fn foreign_addresses_are_data_only() {
    let o = o();
    assert_eq!(o.foreign("https://P12-Caldav.example.net:443/a/../b/").as_deref(), Some("https://p12-caldav.example.net/b/"));
    for no in [
        "https://dav.example.org/x",
        "http://other.example.net/",
        "/relative",
        "ftp://x.example.net/",
        "https://u:p@other.example.net/",
        "https://other.example.net/%2F..;/",
    ] {
        assert_eq!(o.foreign(no), None, "{no}");
    }
    assert!(TrustedOrigin::from_account_settings("http://dav.example.org").is_err());
    assert!(TrustedOrigin::from_account_settings("http://127.0.0.1:8080").is_ok());
    assert!(TrustedOrigin::from_account_settings("https://u@dav.example.org").is_err());
}
