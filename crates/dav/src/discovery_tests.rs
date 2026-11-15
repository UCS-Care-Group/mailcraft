// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The search for a server: where it starts, what it takes for one, and what it never does
//! (send a credential, leave its origin, follow a server for ever).
use super::*;
use crate::http::{HttpRequest, HttpResponse, Method};
use crate::standin::{DAV_HEADER, Quirks, StandIn};
use proptest::prelude::*;
use std::sync::Mutex;

const USER: &str = "owner@example.org";

fn stand_in() -> StandIn {
    let s = StandIn::new("https://dav.example.org", USER, "test-only-not-a-secret").unwrap();
    s.add_calendar("work", "Work", false);
    s
}

/// A server that answers by the address asked, and keeps what it was asked.
struct Script<F: Fn(&str) -> Result<HttpResponse, TransportError>> {
    answer: F,
    asked: Mutex<Vec<HttpRequest>>,
}

impl<F: Fn(&str) -> Result<HttpResponse, TransportError>> Script<F> {
    fn new(answer: F) -> Self {
        Self { answer, asked: Mutex::new(Vec::new()) }
    }
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().iter().map(|r| r.url().to_owned()).collect()
    }
}

impl<F: Fn(&str) -> Result<HttpResponse, TransportError>> HttpExchange for Script<F> {
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.asked.lock().unwrap().push(request.clone());
        (self.answer)(request.url())
    }
}

fn redirect(to: &str) -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse::new(301, Vec::new()).with_field("Location", to))
}

fn dav_401() -> Result<HttpResponse, TransportError> {
    Ok(HttpResponse::new(401, Vec::new()).with_field("WWW-Authenticate", "Basic realm=\"dav\"").with_field("DAV", DAV_HEADER))
}

fn domain() -> Start {
    Start::at_domain("example.org", Kind::Calendar).unwrap()
}

#[test]
fn the_start_is_the_records_place_or_the_domains_well_known_address() {
    assert_eq!(Start::at_domain("Example.ORG", Kind::Calendar).unwrap().url(), "https://example.org/.well-known/caldav");
    assert_eq!(Start::at_domain("example.org", Kind::AddressBook).unwrap().url(), "https://example.org/.well-known/carddav");
    let srv = Start::from_srv("dav.example.org", 8443, None, Kind::AddressBook).unwrap();
    assert_eq!(srv.url(), "https://dav.example.org:8443/.well-known/carddav");
    assert_eq!((srv.origin().host(), srv.origin().port(), srv.origin().secure()), ("dav.example.org", 8443, true));
    // The TXT record's path takes the well-known address's place, and is tidied like any address.
    let path = Start::from_srv("dav.example.org", 443, Some("/SOGo/./dav/"), Kind::Calendar).unwrap();
    assert_eq!(path.url(), "https://dav.example.org/SOGo/dav/");
    assert_eq!(Start::at("https://Cloud.Example.org/remote.php/dav").unwrap().url(), "https://cloud.example.org/remote.php/dav");
    assert!(Start::at("http://127.0.0.1:5232/").is_ok());
    // Nothing of an address is in a log.
    assert_eq!(format!("{srv:?}"), "Start(..)");
}

#[test]
fn a_start_that_is_no_place_over_tls_is_refused() {
    for host in [
        "",
        ".",
        "dav..example.org",
        "dav.example.org.",
        "dav.example.org/x",
        "owner@example.org",
        "dav.example.org:80",
        "[::1]",
        "dav example.org",
        "dáv.example.org",
    ] {
        assert!(Start::from_srv(host, 443, None, Kind::Calendar).is_err(), "{host:?}");
    }
    assert!(Start::from_srv(&format!("{}.example.org", "a".repeat(64)), 443, None, Kind::Calendar).is_err());
    assert!(Start::from_srv(&["a"; 130].join("."), 443, None, Kind::Calendar).is_err());
    assert!(Start::from_srv("dav.example.org", 0, None, Kind::Calendar).is_err());
    for path in ["", "dav", "//other.example/dav", "/dav#x", "/dav\r\nHost: other.example", "/../../x"] {
        assert!(Start::from_srv("dav.example.org", 443, Some(path), Kind::Calendar).is_err(), "{path:?}");
    }
    assert_eq!(Start::at("http://dav.example.org/"), Err(EndpointError::NotEncrypted));
    assert_eq!(Start::at("https://owner:pw@dav.example.org/"), Err(EndpointError::Credentials));
    assert!(Start::at("ftp://dav.example.org/").is_err() && Start::at("dav.example.org").is_err());
}

#[test]
fn a_server_is_found_behind_its_well_known_address_and_no_credential_goes_out() {
    let s = stand_in();
    for kind in [Kind::Calendar, Kind::AddressBook] {
        let start = Start::from_srv("dav.example.org", 443, None, kind).unwrap();
        let found = probe(&s, &start, 0).unwrap();
        assert_eq!(found, Found::Server { root: s.root_url(), schemes: vec!["basic".to_owned()] });
        assert_eq!(format!("{found:?}"), "Server(..)");
    }
    let seen = s.seen();
    assert_eq!(seen.len(), 4, "the well-known address and the root, for each kind");
    assert!(seen.iter().all(|r| r.method == "PROPFIND" && !r.credential), "{seen:?}");
    assert_eq!(seen[0].path, "/.well-known/caldav");
    assert_eq!(seen[2].path, "/.well-known/carddav");
    assert!(s.violations().is_empty());
}

#[test]
fn a_bare_401_is_no_server() {
    let s = stand_in();
    s.set_quirks(Quirks { bare_401: true, ..Quirks::default() });
    assert_eq!(probe(&s, &Start::at(&s.root_url()).unwrap(), 0), Ok(Found::NotDav));
}

#[test]
fn a_server_that_asks_nobody_who_they_are_is_found_by_its_multistatus() {
    let body = "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\"><d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>";
    let server = Script::new(|_| Ok(HttpResponse::new(207, body.as_bytes().to_vec())));
    let found = probe(&server, &Start::from_srv("dav.example.org", 443, Some("/dav/"), Kind::Calendar).unwrap(), 0).unwrap();
    assert_eq!(found, Found::Server { root: "https://dav.example.org/dav/".to_owned(), schemes: Vec::new() });
    // A 207 that is no multistatus is somebody else's.
    let other = Script::new(|_| Ok(HttpResponse::new(207, b"<html/>".to_vec())));
    assert_eq!(probe(&other, &domain(), 0), Ok(Found::NotDav));
}

#[test]
fn what_is_asked_is_a_propfind_for_the_principal_with_nothing_of_the_owner_in_it() {
    let server = Script::new(|_| dav_401());
    probe(&server, &domain(), 0).unwrap();
    let asked = server.asked.lock().unwrap();
    let [request] = asked.as_slice() else { panic!("one request, not {}", asked.len()) };
    assert_eq!((request.method(), request.url()), (Method::Propfind, "https://example.org/.well-known/caldav"));
    assert_eq!(request.field("Depth"), Some("0"));
    let names: Vec<String> = request.fields().map(|(name, _)| name.to_ascii_lowercase()).collect();
    assert!(!names.iter().any(|name| name == "authorization" || name == "cookie"), "{names:?}");
    let body = String::from_utf8_lossy(request.body());
    assert!(body.contains("current-user-principal") && !body.contains('@'), "{body}");
}

#[test]
fn a_redirect_to_another_server_is_handed_back_and_not_followed() {
    for (location, shown) in [
        ("https://dav.hoster.example/caldav/", Some("https://dav.hoster.example/caldav/")),
        ("//dav.hoster.example/caldav/", Some("https://dav.hoster.example/caldav/")),
        // The same host at another port is another server.
        ("https://example.org:8443/dav/", Some("https://example.org:8443/dav/")),
        // Never out of TLS, never to anything that is no web address.
        ("http://example.org/dav/", None),
        ("http://dav.hoster.example/", None),
        ("ftp://example.org/dav/", None),
        ("https://owner:pw@dav.hoster.example/", None),
        ("", None),
        ("https://", None),
    ] {
        let server = Script::new(|_| redirect(location));
        let found = probe(&server, &domain(), 0).unwrap();
        assert_eq!(found, shown.map_or(Found::NotDav, |address| Found::Elsewhere(address.to_owned())), "{location:?}");
        assert_eq!(server.asked(), ["https://example.org/.well-known/caldav"], "{location:?}");
    }
    assert_eq!(format!("{:?}", Found::Elsewhere("https://dav.hoster.example/".to_owned())), "Elsewhere(..)");
}

#[test]
fn the_search_goes_on_at_the_other_server_once_the_owner_agreed() {
    let s = stand_in();
    let first = Script::new(|_| redirect("https://dav.example.org/.well-known/caldav"));
    let Found::Elsewhere(address) = probe(&first, &domain(), 0).unwrap() else { panic!("not sent on") };
    assert_eq!(probe(&s, &Start::at(&address).unwrap(), 0), Ok(Found::Server { root: s.root_url(), schemes: vec!["basic".to_owned()] }));
}

#[test]
fn redirects_are_followed_a_step_at_a_time_and_not_for_ever() {
    // Five steps are taken, and the place they lead to is asked.
    let patient = Script::new(|url| match url.rsplit('/').next().and_then(|n| n.parse::<usize>().ok()) {
        Some(MAX_STEPS) => dav_401(),
        Some(n) => redirect(&format!("/hop/{}", n + 1)),
        None => redirect("/hop/1"),
    });
    assert_eq!(
        probe(&patient, &domain(), 0),
        Ok(Found::Server { root: format!("https://example.org/hop/{MAX_STEPS}"), schemes: vec!["basic".to_owned()] })
    );
    assert_eq!(patient.asked().len(), MAX_STEPS + 1);
    // A sixth is not.
    let endless = Script::new(|url| redirect(&format!("{url}/x")));
    assert_eq!(probe(&endless, &domain(), 0), Ok(Found::NotDav));
    assert_eq!(endless.asked().len(), MAX_STEPS + 1);
    // A circle ends where it closes.
    let circle = Script::new(|url| redirect(if url.ends_with("/a") { "/b" } else { "/a" }));
    assert_eq!(probe(&circle, &domain(), 0), Ok(Found::NotDav));
    assert_eq!(circle.asked().len(), 3);
    let itself = Script::new(|_| redirect("/.well-known/caldav"));
    assert_eq!(probe(&itself, &domain(), 0), Ok(Found::NotDav));
    assert_eq!(itself.asked().len(), 1);
}

#[test]
fn a_relative_redirect_is_read_against_the_address_asked() {
    let server = Script::new(|url| if url.ends_with("/caldav") { redirect("../remote.php/dav/") } else { dav_401() });
    let found = probe(&server, &domain(), 0).unwrap();
    assert_eq!(found, Found::Server { root: "https://example.org/remote.php/dav/".to_owned(), schemes: vec!["basic".to_owned()] });
}

#[test]
fn anything_else_is_no_server_and_a_failed_exchange_is_an_error() {
    for status in [200, 204, 301, 403, 404, 405, 429, 500, 503] {
        let server = Script::new(|_| Ok(HttpResponse::new(status, b"<html>hello</html>".to_vec()).with_field("DAV", DAV_HEADER)));
        assert_eq!(probe(&server, &domain(), 0), Ok(Found::NotDav), "{status}");
        assert_eq!(server.asked().len(), 1, "{status}");
    }
    for error in [TransportError::Unreachable, TransportError::Failed, TransportError::BodyTooLarge, TransportError::Certificate] {
        assert_eq!(probe(&Script::new(|_| Err(error)), &domain(), 0), Err(error));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn a_hostile_server_never_gets_a_request_off_the_origin_or_more_than_six(
        answers in prop::collection::vec((prop_oneof![Just(207u16), Just(301), Just(302), Just(307), Just(308), Just(401), 100u16..600], ".{0,60}", any::<bool>()), 1..8),
        steps in prop_oneof!["[a-z/.:%?#@ -]{0,40}", "(https?:)?//[a-z.]{1,20}(:[0-9]{1,5})?/[a-z/.]{0,20}", Just("//".to_owned())],
    ) {
        let count = Mutex::new(0usize);
        let server = Script::new(|_| {
            let mut n = count.lock().unwrap();
            let (status, body, plain) = &answers[*n % answers.len()];
            *n += 1;
            Ok(HttpResponse::new(*status, body.clone().into_bytes()).with_field("Location", if *plain { &steps } else { body }).with_field("DAV", body))
        });
        let start = domain();
        let found = probe(&server, &start, 0).unwrap();
        let asked = server.asked();
        prop_assert!(!asked.is_empty() && asked.len() <= MAX_STEPS + 1);
        for request in server.asked.lock().unwrap().iter() {
            prop_assert!(Origin::parse(request.url()).is_ok_and(|o| &o == start.origin()), "{}", request.url());
            prop_assert!(request.field("authorization").is_none());
        }
        match found {
            Found::Server { root, .. } => prop_assert!(Origin::parse(&root).is_ok_and(|o| &o == start.origin())),
            Found::Elsewhere(address) => prop_assert!(Origin::parse(&address).is_ok_and(|o| o.secure() && &o != start.origin())),
            Found::NotDav => {}
        }
    }

    #[test]
    fn a_start_from_any_record_is_over_tls_or_refused(host in ".{0,40}", port in any::<u16>(), path in prop::option::of(".{0,40}")) {
        if let Ok(start) = Start::from_srv(&host, port, path.as_deref(), Kind::Calendar) {
            prop_assert!(start.origin().secure() && start.origin().port() == port && port != 0);
            prop_assert!(Origin::parse(start.url()).is_ok_and(|o| &o == start.origin()));
        }
    }
}
