// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Property tests: every reader and builder takes arbitrary input without panicking, and what
//! comes out holds its invariants.
use crate::answer::{Collections, Multistatus, color, status_code};
use crate::href::Origin;
use crate::http::{HttpResponse, Secret};
use crate::request::{Endpoint, Kind, Stamp};
use crate::status::{challenges, classify, dav_classes, http_date, is_dav_evidence, retry_after};
use proptest::prelude::*;

fn origin() -> Origin {
    Origin::parse("https://dav.example.org").unwrap()
}

const NS: &str = "xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:card=\"urn:ietf:params:xml:ns:carddav\" xmlns:a=\"http://apple.com/ns/ical/\" xmlns:cs=\"http://calendarserver.org/ns/\"";

fn junk_text() -> impl Strategy<Value = String> {
    prop_oneof![".{0,40}", "[a-zA-Z0-9/:%._~@#?& \\\\-]{0,60}", Just("../".repeat(30)), Just("%".to_owned()), Just(String::new())]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn resolve_never_panics_and_stays_on_origin(href in junk_text(), base in junk_text()) {
        let o = origin();
        if let Ok(url) = o.resolve(&format!("https://dav.example.org/{base}"), &href) {
            prop_assert!(url.len() <= 2_048);
            prop_assert!(Origin::parse(&url).is_ok_and(|p| p == o), "{url}");
            prop_assert!(!url.contains("/../") && !url.contains("/./") && !url.ends_with("/.."));
            let again = o.resolve(&o.base(), &url);
            prop_assert_eq!(again.as_deref(), Ok(url.as_str()), "idempotent");
        }
        let _ = o.resolve(&base, &href);
        let _ = o.is_within(&base, &href);
        let _ = o.path(&href);
    }

    #[test]
    fn origin_parse_never_panics(url in ".{0,80}") {
        let _ = Origin::parse(&url);
        let _ = crate::http::check_endpoint(&url);
    }

    #[test]
    fn builders_never_panic_and_never_leave_the_origin(url in junk_text(), a in junk_text(), b in junk_text()) {
        let e = Endpoint::new(origin(), Some(Secret::basic("u", "p")));
        let coll = "https://dav.example.org/cal/c/";
        let stamp = Stamp::parse("20260101T000000Z").unwrap();
        let end = Stamp::parse("20270101T000000Z").unwrap();
        let built = [
            e.propfind_principal(&url), e.propfind_homes(&url), e.propfind_collections(&url), e.propfind_state(&url),
            e.options(&url), e.get(&url), e.calendar_query(&url, &stamp, &end),
            e.multiget(Kind::Calendar, coll, &[&url, &a]), e.sync_collection(coll, Some(&a), Some(7)),
            e.put_change(Kind::Calendar, coll, &url, &a, &b), e.delete(coll, &url, &a),
            e.put_create(Kind::AddressBook, coll, &a, &b).map(|(_, r)| r),
        ];
        for r in built.into_iter().flatten() {
            prop_assert!(Origin::parse(r.url()).is_ok_and(|p| p == origin()));
            prop_assert!(r.fields().all(|(_, v)| !v.chars().any(char::is_control)));
            prop_assert!(r.url().len() <= 2_048);
        }
        let _ = Stamp::parse(&url);
    }

    #[test]
    fn multistatus_parse_never_panics_on_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let o = origin();
        if let Ok(m) = Multistatus::parse(&bytes, &o, "https://dav.example.org/") {
            let _ = Collections::from_listing(m);
        }
    }

    #[test]
    fn multistatus_parse_holds_its_invariants_on_plausible_documents(
        hrefs in proptest::collection::vec(junk_text(), 0..6),
        etag in junk_text(), name in junk_text(), colour in junk_text(), status in junk_text(), token in junk_text(),
    ) {
        let esc = |t: &str| crate::request::escape(t);
        let mut inner = String::new();
        for h in &hrefs {
            inner.push_str(&format!(
                "<d:response><d:href>{}</d:href><d:propstat><d:prop><d:getetag>{}</d:getetag><d:displayname>{}</d:displayname><a:calendar-color>{}</a:calendar-color>\
                 <d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:current-user-principal><d:href>{}</d:href></d:current-user-principal></d:prop><d:status>{}</d:status></d:propstat></d:response>",
                esc(h), esc(&etag), esc(&name), esc(&colour), esc(h), esc(&status)
            ));
            inner.push_str(&format!("<d:response><d:href>{}</d:href><d:status>{}</d:status></d:response>", esc(h), esc(&status)));
        }
        let doc = format!("<d:multistatus {NS}>{inner}<d:sync-token>{}</d:sync-token></d:multistatus>", esc(&token));
        let o = origin();
        if let Ok(m) = Multistatus::parse(doc.as_bytes(), &o, "https://dav.example.org/cal/") {
            prop_assert!(m.entries.len() <= crate::answer::MAX_ENTRIES);
            if let Some(t) = &m.sync_token { prop_assert!(t.len() <= crate::request::MAX_TOKEN); }
            for e in &m.entries {
                prop_assert!(Origin::parse(&e.href).is_ok_and(|p| p == o));
                prop_assert!(e.etag.as_ref().is_none_or(|t| t.len() <= crate::request::MAX_TOKEN && !t.chars().any(char::is_control)));
                prop_assert!(e.display_name.as_ref().is_none_or(|n| n.chars().count() <= crate::answer::MAX_NAME));
                prop_assert!(e.principal.as_ref().is_none_or(|p| Origin::parse(p).is_ok_and(|p| p == o)));
            }
            let c = Collections::from_listing(m);
            prop_assert!(c.calendars.len() + c.address_books.len() <= crate::answer::MAX_COLLECTIONS);
        }
    }

    #[test]
    fn small_readers_never_panic(text in ".{0,60}", now in any::<i64>(), code in any::<u16>()) {
        let _ = color(&text);
        let _ = status_code(&text);
        let _ = http_date(&text);
        let w = retry_after(Some(&text), now);
        prop_assert!(w <= crate::status::MAX_WAIT || text.trim().is_empty() || w == 60);
        let r = HttpResponse::new(code, text.clone().into_bytes()).with_field("Retry-After", &text).with_field("DAV", &text).with_field("WWW-Authenticate", &text);
        let _ = (classify(&r, now), challenges(&r), dav_classes(&r), is_dav_evidence(&r, &origin(), "https://dav.example.org/"));
    }

    #[test]
    fn colours_roundtrip(r in any::<u8>(), g in any::<u8>(), b in any::<u8>(), a in any::<u8>()) {
        prop_assert_eq!(color(&format!("#{r:02X}{g:02x}{b:02X}")), Some([r, g, b]));
        prop_assert_eq!(color(&format!("#{r:02X}{g:02x}{b:02X}{a:02x}")), Some([r, g, b]));
    }

    #[test]
    fn stand_in_takes_any_request_body(body in proptest::collection::vec(any::<u8>(), 0..200), which in 0usize..5, depth in "[01x]") {
        use crate::http::{HttpExchange, HttpRequest, Method};
        let s = crate::standin::StandIn::new("https://dav.example.org").unwrap();
        s.add_calendar("work", "Work", false);
        let method = [Method::Propfind, Method::Report, Method::Put, Method::Delete, Method::Get][which];
        let auth = s.credential();
        let url = s.collection_url("work");
        let req = HttpRequest::new(method, &url, &[("Authorization", auth.expose()), ("Depth", &depth), ("Content-Type", "application/xml")], body).unwrap();
        let _ = s.exchange(&req);
    }
}
