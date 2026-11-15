// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in held against the client side of this crate, and the stand-in's own strictness.
use super::*;

const USER: &str = "owner@example.org";
use crate::answer::{Collections, Multistatus};
use crate::request::{Endpoint, Kind, Stamp};
use crate::status::{Verdict, classify};

fn event(uid: &str, start: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:{start}\r\nSUMMARY:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n")
}

fn server() -> (StandIn, Endpoint) {
    let s = StandIn::new("https://dav.example.org", USER, "test-only-not-a-secret").unwrap();
    s.add_calendar("work", "Work", false);
    s.add_calendar("shared", "Shared", true);
    s.add_task_list("tasks", "Tasks");
    s.add_address_book("people", "People", false);
    let e = Endpoint::new(s.origin().clone(), Some(s.credential()));
    (s, e)
}

fn run(s: &StandIn, r: crate::http::HttpRequest) -> HttpResponse {
    s.exchange(&r).unwrap()
}

#[test]
fn discovery_walk_finds_the_collections() {
    let (s, e) = server();
    s.set_quirks(Quirks { sogo_plain_in_home: true, ..Quirks::default() });
    let root = s.root_url();
    let r = run(&s, e.propfind_principal(&root).unwrap());
    assert_eq!(classify(&r, 0), Verdict::Multistatus);
    let m = Multistatus::parse(r.body(), s.origin(), &root).unwrap();
    let principal = m.entries[0].principal.clone().unwrap();
    let r = run(&s, e.propfind_homes(&principal).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &principal).unwrap();
    let (cal_home, card_home) = (m.entries[0].calendar_homes[0].clone(), m.entries[0].addressbook_homes[0].clone());
    let r = run(&s, e.propfind_collections(&cal_home).unwrap());
    let c = Collections::from_listing(Multistatus::parse(r.body(), s.origin(), &cal_home).unwrap());
    let names: Vec<_> = c.calendars.iter().map(|c| c.display_name.clone().unwrap()).collect();
    assert_eq!(names, ["Work", "Shared"]);
    assert_eq!(c.calendars[0].read_only, Some(false));
    assert_eq!(c.calendars[1].read_only, Some(true));
    assert_eq!(c.calendars[0].color, Some([0x33, 0x66, 0xcc]));
    assert!(c.calendars[0].supports_sync() && c.calendars[0].ctag.is_some());
    assert_eq!(c.passed_over, 3, "the home itself, the task list and personal.ics");
    let r = run(&s, e.propfind_collections(&card_home).unwrap());
    let c = Collections::from_listing(Multistatus::parse(r.body(), s.origin(), &card_home).unwrap());
    assert_eq!(c.address_books.len(), 1);
    assert_eq!(c.address_books[0].address_data, ["text/vcard;3.0", "text/vcard;4.0"]);
    assert!(s.violations().is_empty());
}

#[test]
fn probes_without_credentials_get_a_401_whose_dav_header_is_the_evidence() {
    let (s, _) = server();
    let anon = Endpoint::anonymous(s.origin().clone());
    let r = run(&s, anon.propfind_principal(&s.root_url()).unwrap());
    assert_eq!(r.status(), 401);
    assert!(crate::status::is_dav_evidence(&r, s.origin(), &s.root_url()));
    s.set_quirks(Quirks { bare_401: true, ..Quirks::default() });
    let r = run(&s, anon.propfind_principal(&s.root_url()).unwrap());
    assert_eq!(r.status(), 401);
    assert!(!crate::status::is_dav_evidence(&r, s.origin(), &s.root_url()));
    let wrong = Endpoint::new(s.origin().clone(), Some(Secret::basic(USER, "wrong")));
    assert_eq!(run(&s, wrong.options(&s.root_url()).unwrap()).status(), 401);
    assert!(s.seen().iter().all(|x| !x.authorized));
}

#[test]
fn well_known_redirects_to_the_root() {
    let (s, e) = server();
    let r = run(&s, e.propfind_principal("/.well-known/caldav").unwrap());
    assert_eq!(classify(&r, 0), Verdict::Redirect(301));
    assert_eq!(r.header("location"), Some("/dav/"));
}

#[test]
fn a_calendar_syncs_from_nothing_then_by_token_with_removals() {
    let (s, e) = server();
    let c = s.collection_url("work");
    s.put_directly("work", "a.ics", &event("a", "20260301T100000Z")).unwrap();
    s.put_directly("work", "b.ics", &event("b", "20260302T100000Z")).unwrap();
    let r = run(&s, e.sync_collection(&c, None, None).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &c).unwrap();
    assert_eq!(m.members().count(), 2);
    let token = m.sync_token.clone().unwrap();
    assert!(!m.truncated);
    s.put_directly("work", "c.ics", &event("c", "20260303T100000Z")).unwrap();
    s.remove_directly("work", "a.ics");
    let r = run(&s, e.sync_collection(&c, Some(&token), None).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &c).unwrap();
    assert_eq!(m.entries.len(), 2);
    assert_eq!(m.members().count(), 1);
    assert_eq!(m.entries.iter().filter(|x| x.is_gone()).count(), 1);
    assert_eq!(m.sync_token, s.token("work"));
    assert!(s.violations().is_empty());
}

#[test]
fn pages_are_cut_with_507_and_followed_to_the_end() {
    let (s, e) = server();
    let c = s.collection_url("work");
    for i in 0..5 {
        s.put_directly("work", &format!("{i}.ics"), &event(&i.to_string(), "20260301T100000Z")).unwrap();
    }
    s.set_quirks(Quirks { page_size: Some(2), ..Quirks::default() });
    let (mut token, mut seen, mut pages) = (None::<String>, 0, 0);
    loop {
        let r = run(&s, e.sync_collection(&c, token.as_deref(), None).unwrap());
        let m = Multistatus::parse(r.body(), s.origin(), &c).unwrap();
        seen += m.members().count();
        token = m.sync_token.clone();
        pages += 1;
        if !m.truncated || pages > 10 {
            break;
        }
    }
    assert_eq!((seen, pages), (5, 3));
}

#[test]
fn the_limit_in_the_request_is_obeyed() {
    let (s, e) = server();
    let c = s.collection_url("work");
    for i in 0..3 {
        s.put_directly("work", &format!("{i}.ics"), &event(&i.to_string(), "20260301T100000Z")).unwrap();
    }
    let r = run(&s, e.sync_collection(&c, None, Some(1)).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &c).unwrap();
    assert!(m.truncated);
    assert_eq!(m.members().count(), 1);
}

#[test]
fn unknown_tokens_are_403_valid_sync_token() {
    let (s, e) = server();
    let c = s.collection_url("work");
    s.put_directly("work", "a.ics", &event("a", "20260301T100000Z")).unwrap();
    for t in ["junk", "http://example.org/ns/sync/999"] {
        let r = run(&s, e.sync_collection(&c, Some(t), None).unwrap());
        assert_eq!(classify(&r, 0), Verdict::SyncTokenInvalid, "{t}");
    }
    let good = s.token("work").unwrap();
    s.set_quirks(Quirks { token_expiry: true, ..Quirks::default() });
    assert_eq!(classify(&run(&s, e.sync_collection(&c, Some(&good), None).unwrap()), 0), Verdict::SyncTokenInvalid);
    assert_eq!(classify(&run(&s, e.sync_collection(&c, None, None).unwrap()), 0), Verdict::Multistatus);
}

#[test]
fn without_sync_collection_the_report_is_refused_and_not_offered() {
    let (s, e) = server();
    s.set_quirks(Quirks { no_sync_collection: true, ..Quirks::default() });
    let c = s.collection_url("work");
    assert_eq!(classify(&run(&s, e.sync_collection(&c, None, None).unwrap()), 0), Verdict::Forbidden);
    let home = format!("{}/dav/cal/owner/", s.origin().base());
    let r = run(&s, e.propfind_collections(&home).unwrap());
    let col = Collections::from_listing(Multistatus::parse(r.body(), s.origin(), &home).unwrap());
    assert!(!col.calendars[0].supports_sync() && col.calendars[0].sync_token.is_none());
}

#[test]
fn the_calendar_query_then_multiget_path() {
    let (s, e) = server();
    let c = s.collection_url("work");
    s.put_directly("work", "in.ics", &event("in", "20260601T100000Z")).unwrap();
    s.put_directly("work", "out.ics", &event("out", "20300601T100000Z")).unwrap();
    let r = run(&s, e.calendar_query(&c, &Stamp::parse("20260101T000000Z").unwrap(), &Stamp::parse("20270101T000000Z").unwrap()).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &c).unwrap();
    assert_eq!(m.members().count(), 1);
    assert!(m.entries[0].data.is_none(), "getetag only");
    let href = m.entries[0].href.clone();
    let r = run(&s, e.multiget(Kind::Calendar, &c, &[&href]).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &c).unwrap();
    assert!(m.entries[0].data.as_deref().unwrap().contains("UID:in"));
    assert!(m.entries[0].etag.is_some());
    assert!(s.violations().is_empty());
}

#[test]
fn contacts_multiget_returns_address_data() {
    let (s, e) = server();
    let c = s.collection_url("people");
    s.put_directly("people", "p.vcf", "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:A\r\nEND:VCARD\r\n").unwrap();
    let r = run(&s, e.propfind_state(&c).unwrap());
    assert_eq!(classify(&r, 0), Verdict::Multistatus);
    let r = run(&s, e.multiget(Kind::AddressBook, &c, &[&format!("{c}p.vcf")]).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &c).unwrap();
    assert!(m.entries[0].data.as_deref().unwrap().contains("FN:A"));
    // A calendar report on an address book is not supported.
    let r = run(&s, e.calendar_query(&c, &Stamp::parse("20260101T000000Z").unwrap(), &Stamp::parse("20270101T000000Z").unwrap()).unwrap());
    assert_eq!(classify(&r, 0), Verdict::Forbidden);
}

#[test]
fn creates_changes_and_deletes_with_the_right_preconditions() {
    let (s, e) = server();
    let c = s.collection_url("work");
    let (url, put) = e.put_create(Kind::Calendar, &c, "new1", &event("n", "20260301T100000Z")).unwrap();
    let r = run(&s, put.clone());
    assert_eq!(classify(&r, 0), Verdict::Done);
    let etag = r.header("etag").unwrap().to_owned();
    assert_eq!(s.etag_of("work", "new1.ics"), Some(etag.clone()));
    // The name exists: 412, nothing overwritten.
    assert_eq!(classify(&run(&s, put), 0), Verdict::PreconditionFailed);
    // A change with the right ETag, then the stale one.
    let ch = e.put_change(Kind::Calendar, &c, &url, &etag, &event("n", "20260401T100000Z")).unwrap();
    assert_eq!(run(&s, ch.clone()).status(), 204);
    assert_eq!(classify(&run(&s, ch), 0), Verdict::PreconditionFailed);
    let current = s.etag_of("work", "new1.ics").unwrap();
    assert_eq!(classify(&run(&s, e.delete(&c, &url, &etag).unwrap()), 0), Verdict::PreconditionFailed);
    assert_eq!(classify(&run(&s, e.delete(&c, &url, &current).unwrap()), 0), Verdict::Done);
    assert_eq!(classify(&run(&s, e.delete(&c, &url, &current).unwrap()), 0), Verdict::Gone);
    assert_eq!(s.writes(), 3);
    assert!(s.violations().is_empty());
}

#[test]
fn a_read_only_collection_refuses_writes() {
    let (s, e) = server();
    let c = s.collection_url("shared");
    let (_, put) = e.put_create(Kind::Calendar, &c, "x", &event("x", "20260301T100000Z")).unwrap();
    assert_eq!(classify(&run(&s, put), 0), Verdict::Forbidden);
    assert_eq!(s.writes(), 0);
}

#[test]
fn data_that_is_not_a_calendar_object_is_refused() {
    let (s, e) = server();
    let c = s.collection_url("work");
    let (_, put) = e.put_create(Kind::Calendar, &c, "x", "hello").unwrap();
    assert_eq!(classify(&run(&s, put), 0), Verdict::Forbidden);
}

fn raw(s: &StandIn, method: Method, url: &str, fields: &[(&str, &str)], body: &str) -> HttpResponse {
    let mut all = vec![("Authorization", s.credential().expose().to_owned())];
    all.extend(fields.iter().map(|(n, v)| (*n, (*v).to_owned())));
    let all: Vec<(&str, &str)> = all.iter().map(|(n, v)| (*n, v.as_str())).collect();
    s.exchange(&HttpRequest::new(method, url, &all, body.as_bytes().to_vec()).unwrap()).unwrap()
}

#[test]
fn the_stand_in_rejects_what_the_rfcs_forbid() {
    let (s, _) = server();
    let c = s.collection_url("work");
    let ics = event("z", "20260301T100000Z");
    let ct = ("Content-Type", "text/calendar; charset=utf-8");
    // Create without If-None-Match: *.
    assert_eq!(raw(&s, Method::Put, &format!("{c}z.ics"), &[ct], &ics).status(), 428);
    assert_eq!(s.violations().len(), 1);
    s.put_directly("work", "z.ics", &ics).unwrap();
    // Change without If-Match.
    assert_eq!(raw(&s, Method::Put, &format!("{c}z.ics"), &[ct], &ics).status(), 428);
    // Delete without If-Match.
    assert_eq!(raw(&s, Method::Delete, &format!("{c}z.ics"), &[], "").status(), 428);
    assert_eq!(s.violations().len(), 3);
    assert_eq!(s.writes(), 0);
    // Infinite depth.
    let xml = [("Content-Type", "application/xml")];
    let body = "<d:propfind xmlns:d=\"DAV:\"><d:prop/></d:propfind>";
    assert_eq!(raw(&s, Method::Propfind, &c, &xml, body).status(), 403);
    assert_eq!(raw(&s, Method::Propfind, &c, &[xml[0], ("Depth", "infinity")], body).status(), 403);
    // Not XML / wrong root.
    assert_eq!(raw(&s, Method::Propfind, &c, &[xml[0], ("Depth", "0")], "junk").status(), 400);
    assert_eq!(raw(&s, Method::Propfind, &c, &[("Depth", "0")], body).status(), 415);
    // sync-collection with Depth 1.
    let sync = "<d:sync-collection xmlns:d=\"DAV:\"><d:sync-token/><d:sync-level>1</d:sync-level><d:prop><d:getetag/></d:prop></d:sync-collection>";
    assert_eq!(raw(&s, Method::Report, &c, &[xml[0], ("Depth", "1")], sync).status(), 400);
    assert_eq!(raw(&s, Method::Report, &c, &[xml[0], ("Depth", "0")], sync).status(), 207);
}

#[test]
fn off_collection_multiget_hrefs_are_a_violation() {
    let (s, _) = server();
    let c = s.collection_url("work");
    s.put_directly("shared", "x.ics", &event("x", "20260301T100000Z")).unwrap();
    let body = "<c:calendar-multiget xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\"><d:prop><d:getetag/></d:prop><d:href>/dav/cal/owner/shared/x.ics</d:href></c:calendar-multiget>";
    let r = raw(&s, Method::Report, &c, &[("Content-Type", "application/xml"), ("Depth", "1")], body);
    assert_eq!(r.status(), 207);
    assert!(!String::from_utf8_lossy(r.body()).contains("200 OK"));
    assert_eq!(s.violations().len(), 1);
}

#[test]
fn google_mode_changes_the_layout_and_hrefs() {
    let (s, e) = server();
    s.set_quirks(Quirks { google: true, ..Quirks::default() });
    let root = s.root_url();
    assert!(root.ends_with("/caldav/v2/"));
    let r = run(&s, e.propfind_principal(&root).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &root).unwrap();
    let principal = m.entries[0].principal.clone().unwrap();
    assert!(principal.contains("owner%40example.org"));
    let r = run(&s, e.propfind_homes(&principal).unwrap());
    let m = Multistatus::parse(r.body(), s.origin(), &principal).unwrap();
    assert!(m.entries[0].addressbook_homes.is_empty());
    let home = m.entries[0].calendar_homes[0].clone();
    let r = run(&s, e.propfind_collections(&home).unwrap());
    assert!(String::from_utf8_lossy(r.body()).contains("<d:href>https://dav.example.org/caldav/v2/"));
    let c = Collections::from_listing(Multistatus::parse(r.body(), s.origin(), &home).unwrap());
    assert_eq!(c.calendars.len(), 2);
    assert!(c.calendars.iter().all(|c| c.ctag.is_none()));
}

#[test]
fn another_origin_is_not_served() {
    let (s, _) = server();
    let r = s.exchange(&HttpRequest::new(Method::Options, "https://other.example.org/", &[], Vec::new()).unwrap()).unwrap();
    assert_eq!(r.status(), 421);
}
