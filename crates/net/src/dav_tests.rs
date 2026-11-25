// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The runner of a calendar server job against the strict stand-in of `mailcraft-dav`, and against
//! servers that lie.

use std::cell::RefCell;

use mailcraft_dav::http::{HttpExchange, HttpRequest, HttpResponse, TransportError};
use mailcraft_dav::standin::{Quirks, StandIn};
use mailcraft_model::{DavAsk, DavAskCollection, DavKnown, DavProblem, DavRead, DavReadState};

use super::*;

const ORIGIN: &str = "https://dav.example.org";
const USER: &str = "owner@example.org";
const PASSWORD: &str = "test-only-not-a-secret";
const NOW: i64 = 1_791_993_600;

fn event(uid: &str, start: &str) -> String {
    format!("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:{uid}\r\nDTSTART:{start}\r\nSUMMARY:{uid}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n")
}

fn server() -> StandIn {
    let s = StandIn::new(ORIGIN, USER, PASSWORD).unwrap();
    s.add_calendar("work", "Work", false);
    s.add_calendar("shared", "Shared", true);
    s.add_task_list("tasks", "Tasks");
    s.add_address_book("people", "People", false);
    s
}

fn home(_: &StandIn) -> String {
    format!("{ORIGIN}/dav/cal/owner/")
}

fn ask(s: &StandIn, list: bool, collections: Vec<DavAskCollection>) -> DavAsk {
    DavAsk { account: 7, email: USER.into(), username: USER.into(), homes: vec![home(s)], list_homes: list, collections, ..Default::default() }
}

/// The passwords of the tests, with the origins that were asked.
struct Held {
    asked: RefCell<Vec<String>>,
    password: Option<&'static str>,
}

impl Held {
    fn new(password: Option<&'static str>) -> Held {
        Held { asked: RefCell::new(Vec::new()), password }
    }
    fn lookup(&self) -> impl Fn(&str) -> Option<String> + '_ {
        move |origin| {
            self.asked.borrow_mut().push(origin.to_string());
            self.password.map(str::to_string)
        }
    }
}

fn read(s: &StandIn, ask: &DavAsk) -> DavDownload {
    let held = Held::new(Some(PASSWORD));
    let out = run(ask, &held.lookup(), s, NOW, &Limits::default());
    assert!(s.violations().is_empty(), "{:?}", s.violations());
    out
}

/// What the engine would ask next after `read`.
fn next(read: &DavRead, known: &[(&str, &str)]) -> DavAskCollection {
    DavAskCollection {
        href: read.href.clone(),
        sync_token: read.sync_token.clone(),
        ctag: read.ctag.clone(),
        known: known.iter().map(|(h, e)| DavKnown { href: (*h).into(), etag: (*e).into() }).collect(),
    }
}

fn names(read: &DavRead) -> Vec<String> {
    let mut names: Vec<String> = read.changes.iter().filter(|c| !c.gone).map(|c| c.href.rsplit('/').next().unwrap_or_default().to_string()).collect();
    names.sort();
    names
}

#[test]
fn the_class_cap_is_the_same_on_both_sides() {
    assert_eq!(crate::http::MAX_DAV, mailcraft_dav::status::MAX_DAV_CLASSES);
}

#[test]
fn a_first_sync_lists_the_homes_and_reads_every_calendar_by_listing_it() {
    let s = server();
    s.put_directly("work", "a.ics", &event("a", "20261201T100000Z")).unwrap();
    s.put_directly("work", "b.ics", &event("b", "20261202T100000Z")).unwrap();
    s.put_directly("shared", "c.ics", &event("c", "20261203T100000Z")).unwrap();
    let held = Held::new(Some(PASSWORD));
    let out = run(&ask(&s, true, Vec::new()), &held.lookup(), &s, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::None);
    let listing = out.listing.as_ref().unwrap();
    // (Calendars of events only: no task list, no address book.)
    let mut listed: Vec<(&str, bool)> = listing.calendars.iter().map(|c| (c.name.as_str(), c.read_only)).collect();
    listed.sort();
    assert_eq!(listed, vec![("Shared", true), ("Work", false)]);
    assert!(listing.complete && !listing.capped);
    let work = out.collections.iter().find(|c| c.href == s.collection_url("work")).unwrap();
    assert_eq!(names(work), vec!["a.ics", "b.ics"]);
    assert!(work.complete);
    assert_eq!(work.members.as_ref().map(Vec::len), Some(2), "a whole listing");
    assert_eq!(Some(work.sync_token.clone()), s.token("work"), "the token from before the listing");
    assert!(work.changes.iter().all(|c| c.raw.as_deref().is_some_and(|r| r.contains("BEGIN:VCALENDAR")) && !c.etag.is_empty()));
    assert_eq!(out.collections.len(), 2);
    assert_eq!(out.signed_in, vec![ORIGIN.to_string()]);
    // The password was asked for once, for this origin, after the server's challenge.
    assert_eq!(*held.asked.borrow(), vec![ORIGIN.to_string()]);
}

#[test]
fn nothing_carries_a_credential_until_the_server_has_challenged() {
    let s = server();
    s.put_directly("work", "a.ics", &event("a", "20261201T100000Z")).unwrap();
    let _ = read(&s, &ask(&s, true, Vec::new()));
    let seen = s.seen();
    assert!(!seen[0].credential, "the first request has no credential");
    assert!(seen[1].credential && seen[1].authorized, "the answer to the challenge has it");
    // And once challenged, the origin has it up front for the rest of the run.
    assert!(seen.iter().skip(1).all(|r| r.authorized));
    assert_eq!(seen.iter().filter(|r| !r.credential).count(), 1);
}

#[test]
fn a_delta_brings_what_changed_and_what_went() {
    let s = server();
    s.put_directly("work", "a.ics", &event("a", "20261201T100000Z")).unwrap();
    s.put_directly("work", "b.ics", &event("b", "20261202T100000Z")).unwrap();
    let first = read(&s, &ask(&s, true, Vec::new()));
    let work = first.collections.iter().find(|c| c.href == s.collection_url("work")).unwrap();
    let state = next(work, &[]);
    s.put_directly("work", "c.ics", &event("c", "20261203T100000Z")).unwrap();
    s.remove_directly("work", "a.ics");
    let second = read(&s, &ask(&s, false, vec![state.clone()]));
    let work = &second.collections[0];
    assert_eq!(work.state, DavReadState::Read);
    assert!(work.members.is_none() && work.complete, "changes only: nothing is taken to be gone but what the server said");
    assert_eq!(names(work), vec!["c.ics"]);
    assert_eq!(work.changes.iter().filter(|c| c.gone).count(), 1);
    assert!(work.changes.iter().find(|c| c.gone).unwrap().href.ends_with("a.ics"));
    assert_eq!(Some(work.sync_token.clone()), s.token("work"));
    // Nothing changed since: the cheap question answers it, and nothing else is asked.
    let state = next(work, &[]);
    let before = s.seen().len();
    let third = read(&s, &ask(&s, false, vec![state]));
    assert_eq!(third.collections[0].state, DavReadState::Unchanged);
    assert!(s.seen().len() - before <= 2, "one question (and its challenge, if it came again)");
}

#[test]
fn a_server_without_sync_collection_is_read_by_listing_and_multiget_then_by_change_tag() {
    let s = server();
    s.set_quirks(Quirks { no_sync_collection: true, ..Quirks::default() });
    s.put_directly("work", "a.ics", &event("a", "20261201T100000Z")).unwrap();
    s.put_directly("work", "b.ics", &event("b", "20261202T100000Z")).unwrap();
    let first = read(&s, &ask(&s, true, Vec::new()));
    let work = first.collections.iter().find(|c| c.href == s.collection_url("work")).unwrap();
    assert_eq!(names(work), vec!["a.ics", "b.ics"]);
    assert!(work.complete && work.sync_token.is_empty() && !work.ctag.is_empty());
    assert_eq!(work.members.as_ref().map(Vec::len), Some(2));
    // The next read fetches only what is new or different.
    let a = s.collection_url("work") + "a.ics";
    let etag_a = s.etag_of("work", "a.ics").unwrap();
    s.put_directly("work", "c.ics", &event("c", "20261203T100000Z")).unwrap();
    let second = read(&s, &ask(&s, false, vec![next(work, &[(a.as_str(), etag_a.as_str())])]));
    let work = &second.collections[0];
    assert!(work.complete);
    assert_eq!(work.members.as_ref().map(Vec::len), Some(3));
    assert_eq!(names(work), vec!["b.ics", "c.ics"]);
    // And then nothing changed: the change tag says so.
    let third = read(&s, &ask(&s, false, vec![next(work, &[])]));
    assert_eq!(third.collections[0].state, DavReadState::Unchanged);
}
#[test]
fn change_pages_cut_short_with_507_are_followed_to_the_end() {
    let s = server();
    let first = read(&s, &ask(&s, true, Vec::new()));
    let state = next(first.collections.iter().find(|c| c.href == s.collection_url("work")).unwrap(), &[]);
    s.set_quirks(Quirks { page_size: Some(2), ..Quirks::default() });
    for i in 0..5 {
        s.put_directly("work", &format!("{i}.ics"), &event(&i.to_string(), "20261201T100000Z")).unwrap();
    }
    let out = read(&s, &ask(&s, false, vec![state]));
    let work = &out.collections[0];
    assert_eq!(names(work).len(), 5);
    assert!(work.complete && work.members.is_none());
    assert_eq!(Some(work.sync_token.clone()), s.token("work"));
}
#[test]
fn a_first_read_that_stops_short_keeps_no_token_and_goes_on_at_the_next_run() {
    let s = server();
    for i in 0..5 {
        s.put_directly("work", &format!("{i}.ics"), &event(&i.to_string(), "20261201T100000Z")).unwrap();
    }
    let held = Held::new(Some(PASSWORD));
    // A run that may take in hardly any bytes still takes one document.
    let limits = Limits { raw_bytes: 1, ..Limits::default() };
    let mut known: Vec<(String, String)> = Vec::new();
    let mut state = DavAskCollection { href: s.collection_url("work"), ..Default::default() };
    for run_no in 0..5 {
        state.known = known.iter().map(|(h, e)| DavKnown { href: h.clone(), etag: e.clone() }).collect();
        let out = run(&ask(&s, false, vec![state.clone()]), &held.lookup(), &s, NOW, &limits);
        assert_eq!(out.problem, DavProblem::Limits);
        let work = &out.collections[0];
        assert_eq!(work.changes.len(), 1, "run {run_no}: one more each run");
        assert!(!work.complete && work.sync_token.is_empty() && work.ctag.is_empty(), "nothing is kept to go on from");
        assert_eq!(work.members.as_ref().map(Vec::len), Some(5), "but the listing was whole");
        known.extend(work.changes.iter().map(|c| (c.href.clone(), c.etag.clone())));
    }
    state.known = known.iter().map(|(h, e)| DavKnown { href: h.clone(), etag: e.clone() }).collect();
    let out = run(&ask(&s, false, vec![state]), &held.lookup(), &s, NOW, &limits);
    let work = &out.collections[0];
    assert!(work.complete && work.changes.is_empty() && out.problem == DavProblem::None, "{:?}", out.problem);
    assert_eq!(Some(work.sync_token.clone()), s.token("work"));
}
#[test]
fn a_change_pass_that_stops_short_keeps_the_token_of_the_last_page_it_took_in() {
    let s = server();
    s.put_directly("work", "z.ics", &event("z", "20261201T100000Z")).unwrap();
    let first = read(&s, &ask(&s, true, Vec::new()));
    let state = next(first.collections.iter().find(|c| c.href == s.collection_url("work")).unwrap(), &[]);
    s.set_quirks(Quirks { page_size: Some(2), ..Quirks::default() });
    for i in 0..5 {
        s.put_directly("work", &format!("{i}.ics"), &event(&i.to_string(), "20261201T100000Z")).unwrap();
    }
    let held = Held::new(Some(PASSWORD));
    let limits = Limits { pages: 2, ..Limits::default() };
    let out = run(&ask(&s, false, vec![state.clone()]), &held.lookup(), &s, NOW, &limits);
    let work = &out.collections[0];
    assert!(!work.complete && work.members.is_none());
    assert_ne!(work.sync_token, state.sync_token, "what was taken in is not asked for again");
    assert_eq!(work.changes.len(), 4, "two pages of two");
}

#[test]
fn a_token_the_server_forgot_is_read_afresh_and_matched_by_address() {
    let s = server();
    s.put_directly("work", "a.ics", &event("a", "20261201T100000Z")).unwrap();
    s.put_directly("work", "b.ics", &event("b", "20261202T100000Z")).unwrap();
    let first = read(&s, &ask(&s, true, Vec::new()));
    let state = next(first.collections.iter().find(|c| c.href == s.collection_url("work")).unwrap(), &[]);
    s.set_quirks(Quirks { token_expiry: true, ..Quirks::default() });
    s.put_directly("work", "c.ics", &event("c", "20261203T100000Z")).unwrap();
    let out = read(&s, &ask(&s, false, vec![state]));
    let work = &out.collections[0];
    assert!(work.complete, "read again, all of it");
    assert_eq!(names(work), vec!["a.ics", "b.ics", "c.ics"]);
    assert_eq!(work.members.as_ref().map(Vec::len), Some(3));
    assert_eq!(Some(work.sync_token.clone()), s.token("work"));
}

#[test]
fn a_resource_over_a_megabyte_is_read_whole_for_showing_and_one_over_two_is_never_asked_for_again() {
    let s = server();
    let pad = |n: usize| {
        format!(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:big\r\nDTSTART:20261201T100000Z\r\nSUMMARY:big\r\nDESCRIPTION:{}\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            "x".repeat(n)
        )
    };
    s.put_directly("work", "big.ics", &pad(1_500_000)).unwrap();
    s.put_directly("work", "huge.ics", &pad(2_200_000)).unwrap();
    s.put_directly("work", "a.ics", &event("a", "20261201T100000Z")).unwrap();
    let out = read(&s, &ask(&s, true, Vec::new()));
    let work = out.collections.iter().find(|c| c.href == s.collection_url("work")).unwrap();
    assert_eq!(names(work), vec!["a.ics", "big.ics", "huge.ics"]);
    assert!(work.changes.iter().find(|c| c.href.ends_with("huge.ics")).unwrap().raw.is_none(), "said, without its document");
    assert!(work.changes.iter().find(|c| c.href.ends_with("big.ics")).unwrap().raw.as_ref().unwrap().len() > 1_000_000);
    // (It can never be had: asking again brings the same, so it does not hold up the token.)
    assert!(work.complete);
    assert_eq!(work.members.as_ref().map(Vec::len), Some(3), "and it is still there");
}

#[test]
fn a_collection_with_more_than_one_run_takes_goes_on_at_the_next_run() {
    let s = server();
    let total = mailcraft_model::dav_down::MAX_CHANGES + 300;
    for i in 0..total {
        s.put_directly("shared", &format!("{i}.ics"), "BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n").unwrap();
    }
    let out = read(&s, &ask(&s, true, Vec::new()));
    let shared = out.collections.iter().find(|c| c.href == s.collection_url("shared")).unwrap();
    assert!(!shared.complete && shared.sync_token.is_empty());
    assert_eq!(shared.changes.len(), mailcraft_model::dav_down::MAX_CHANGES);
    // The next run is told what is here, and brings the rest.
    let known: Vec<(String, String)> = shared.changes.iter().map(|c| (c.href.clone(), c.etag.clone())).collect();
    let known: Vec<(&str, &str)> = known.iter().map(|(h, e)| (h.as_str(), e.as_str())).collect();
    let out = read(&s, &ask(&s, false, vec![next(shared, &known)]));
    let shared = &out.collections[0];
    assert!(shared.complete);
    assert_eq!(shared.changes.len(), 300);
    assert_eq!(shared.members.as_ref().map(Vec::len), Some(total));
    assert_eq!(Some(shared.sync_token.clone()), s.token("shared"));
}

#[test]
fn more_than_twenty_thousand_resources_are_capped_and_prove_nothing() {
    let many = mailcraft_model::dav_down::MAX_RESOURCES + 50;
    let x = Scripted::new(move |request| match kind(request) {
        "members" => multistatus(&(0..many).map(|i| member(&format!("/cal/work/{i}.ics"), &format!("\"{i}\""), None)).collect::<String>()),
        _ => multistatus(""),
    });
    let mut ask = collection_ask();
    // (Everything is here already: nothing needs fetching.)
    ask.collections[0].known = (0..many).map(|i| DavKnown { href: format!("{ORIGIN}/cal/work/{i}.ics"), etag: format!("\"{i}\"") }).collect();
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    assert!(!work.complete && work.members.is_none(), "a listing cut short says nothing of what is gone");
    assert!(work.sync_token.is_empty(), "nothing is kept to go on from");
}
/// A server whose answers are written by the test.
struct Scripted {
    answer: Box<dyn Fn(&HttpRequest) -> HttpResponse>,
    seen: RefCell<Vec<(String, bool)>>,
}

impl Scripted {
    fn new(answer: impl Fn(&HttpRequest) -> HttpResponse + 'static) -> Scripted {
        Scripted { answer: Box::new(answer), seen: RefCell::new(Vec::new()) }
    }
}

impl HttpExchange for Scripted {
    fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
        self.seen.borrow_mut().push((request.url().to_string(), request.field("authorization").is_some()));
        Ok((self.answer)(request))
    }
}

const XML: &str = "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\">";

fn multistatus(body: &str) -> HttpResponse {
    HttpResponse::new(207, format!("{XML}{body}</d:multistatus>").into_bytes())
}

/// A member, as a listing or a report names it (a plain resource: its `resourcetype` is empty).
fn member(href: &str, etag: &str, data: Option<&str>) -> String {
    let data = data.map(|d| format!("<c:calendar-data>{d}</c:calendar-data>")).unwrap_or_default();
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype/><d:getetag>{etag}</d:getetag>{data}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

/// What a request to a scripted server is.
fn kind(request: &HttpRequest) -> &'static str {
    let body = String::from_utf8_lossy(request.body()).to_string();
    if body.contains("sync-collection") {
        "sync"
    } else if body.contains("multiget") {
        "multiget"
    } else if body.contains("propfind") && body.contains("getetag") {
        "members"
    } else if body.contains("propfind") {
        "state"
    } else {
        "get"
    }
}

fn collection_ask() -> DavAsk {
    DavAsk {
        account: 7,
        email: USER.into(),
        username: USER.into(),
        homes: vec![format!("{ORIGIN}/cal/")],
        collections: vec![DavAskCollection { href: format!("{ORIGIN}/cal/work/"), ..Default::default() }],
        ..Default::default()
    }
}

/// A member whose ETag the server didn't give (a property that is 404 there).
fn bare(href: &str) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype/></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><d:getetag/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>"
    )
}

#[test]
fn a_member_without_a_usable_etag_is_still_there_and_is_asked_for_in_both_passes() {
    let doc = "BEGIN:VCALENDAR\nEND:VCALENDAR";
    let x = Scripted::new(move |request| match kind(request) {
        "members" => multistatus(&format!(
            "{}{}{}{}",
            base("/cal/work/", "200 OK"),
            member("/cal/work/a.ics", "\"1\"", None),
            bare("/cal/work/b.ics"),
            member("/cal/work/c.ics", "", None)
        )),
        "sync" => multistatus(&format!("{}{}<d:sync-token>t2</d:sync-token>", bare("/cal/work/b.ics"), member("/cal/work/c.ics", "", None))),
        _ => multistatus(&format!(
            "{}{}{}",
            member("/cal/work/a.ics", "\"1\"", Some(doc)),
            member("/cal/work/b.ics", "", Some(doc)),
            member("/cal/work/c.ics", "", Some(doc))
        )),
    });
    // By listing.
    let out = run(&collection_ask(), &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    assert!(work.complete, "{work:?} {:?} {:?}", out.problem, x.seen.borrow());
    assert_eq!(work.members.as_ref().map(Vec::len), Some(3), "none of them may look gone");
    assert_eq!(work.changes.len(), 3, "and each is read");
    // By token: both are named, and read unless the engine says they were read today.
    let mut ask = collection_ask();
    ask.collections[0].sync_token = "t1".into();
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.collections[0].changes.len(), 2);
    ask.collections[0].known =
        ["a.ics", "b.ics", "c.ics"].iter().map(|n| DavKnown { href: format!("{ORIGIN}/cal/work/{n}"), etag: String::new() }).collect();
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    assert!(out.collections[0].changes.is_empty());
}
/// The names of the resources of `/cal/work/` that a request body asks for.
fn asked(body: &str) -> Vec<String> {
    body.split("/cal/work/").skip(1).map(|rest| rest.split(['<', '"']).next().unwrap_or_default().to_string()).filter(|n| !n.is_empty()).collect()
}

const DOC: &str = "BEGIN:VCALENDAR\nEND:VCALENDAR";

fn listing_of(count: usize) -> String {
    (0..count).map(|i| member(&format!("/cal/work/{i}.ics"), "\"1\"", None)).collect()
}

#[test]
fn a_change_pass_with_more_deletions_than_a_read_holds_starts_over_by_listing_and_ends() {
    let gone = |count: usize| -> String {
        (0..count)
            .map(|i| format!("<d:response><d:href>/cal/work/{i}.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>"))
            .collect()
    };
    let server = |count: usize| {
        Scripted::new(move |request| match kind(request) {
            "sync" => multistatus(&format!("{}<d:sync-token>new</d:sync-token>", gone(count))),
            "members" => multistatus(&format!("{}{}", base("/cal/work/", "200 OK"), listing_of(3))),
            "state" => multistatus(
                "<d:response><d:href>/cal/work/</d:href><d:propstat><d:prop><d:sync-token>fresh</d:sync-token></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",
            ),
            _ => multistatus(
                &asked(&String::from_utf8_lossy(request.body()))
                    .iter()
                    .map(|n| member(&format!("/cal/work/{n}"), "\"1\"", Some(DOC)))
                    .collect::<String>(),
            ),
        })
    };
    let ask = || {
        let mut ask = collection_ask();
        ask.collections[0].sync_token = "old".into();
        ask
    };
    // Exactly the cap fits.
    let out = run(&ask(), &Held::new(Some(PASSWORD)).lookup(), &server(MAX_CHANGES), NOW, &Limits::default());
    let fits = &out.collections[0];
    assert!(fits.complete && fits.members.is_none() && fits.sync_token == "new" && fits.changes.len() == MAX_CHANGES);
    // One more does not: the page is not taken in part (its last deletion would be passed over
    // for good). The collection is listed afresh, and what is not in it is found by its absence.
    let out = run(&ask(), &Held::new(Some(PASSWORD)).lookup(), &server(MAX_CHANGES + 1), NOW, &Limits::default());
    let over = &out.collections[0];
    assert!(over.complete, "{:?}", over.sync_token);
    assert_eq!((over.sync_token.as_str(), over.members.as_ref().map(Vec::len)), ("fresh", Some(3)));
    assert!(over.changes.iter().all(|c| !c.gone), "nothing of the page that couldn't be taken whole");
}
#[test]
fn a_change_pass_with_more_changes_than_a_read_holds_goes_on_at_the_next_run() {
    let count = MAX_CHANGES + 1;
    let server = || {
        Scripted::new(move |request| match kind(request) {
            // (A server that sends all of them, whatever it was asked for.)
            "sync" => multistatus(&format!("{}<d:sync-token>new</d:sync-token>", listing_of(count))),
            "members" => multistatus(&format!("{}{}", base("/cal/work/", "200 OK"), listing_of(count))),
            "state" => multistatus(""),
            _ => multistatus(
                &asked(&String::from_utf8_lossy(request.body()))
                    .iter()
                    .map(|n| member(&format!("/cal/work/{n}"), "\"1\"", Some(DOC)))
                    .collect::<String>(),
            ),
        })
    };
    let mut ask = collection_ask();
    ask.collections[0].sync_token = "old".into();
    let first = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &server(), NOW, &Limits::default());
    let first = &first.collections[0];
    assert!(!first.complete && first.changes.len() == MAX_CHANGES);
    assert!(first.sync_token.is_empty(), "the token is of no use: the next run lists the collection");
    // The engine took them in, and says what is here.
    let mut ask = collection_ask();
    ask.collections[0].known = first.changes.iter().map(|c| DavKnown { href: c.href.clone(), etag: c.etag.clone() }).collect();
    let second = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &server(), NOW, &Limits::default());
    let second = &second.collections[0];
    assert_eq!(second.changes.len(), 1, "only what is not here yet");
    assert!(second.complete && second.members.as_ref().map(Vec::len) == Some(count), "so it is done now");
}

#[test]
fn a_member_the_server_gave_no_etag_for_is_not_read_again_at_every_full_read_but_is_when_it_changes() {
    let server = || {
        Scripted::new(move |request| match kind(request) {
            "sync" => multistatus(&format!("{}{}<d:sync-token>t</d:sync-token>", bare("/cal/work/a.ics"), bare("/cal/work/b.ics"))),
            "members" => multistatus(&format!("{}{}", bare("/cal/work/a.ics"), bare("/cal/work/b.ics"))),
            "state" => multistatus(""),
            _ => multistatus(
                &asked(&String::from_utf8_lossy(request.body())).iter().map(|n| member(&format!("/cal/work/{n}"), "", Some(DOC))).collect::<String>(),
            ),
        })
    };
    let mut ask = collection_ask();
    // Without a token: a is here (under no ETag), b is not.
    ask.collections[0].known = vec![DavKnown { href: format!("{ORIGIN}/cal/work/a.ics"), etag: String::new() }];
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &server(), NOW, &Limits::default());
    let hrefs: Vec<&str> = out.collections[0].changes.iter().map(|c| c.href.as_str()).collect();
    assert_eq!(hrefs, vec![format!("{ORIGIN}/cal/work/b.ics")], "{hrefs:?}");
    // A change pass names what changed: both are read.
    let mut ask = collection_ask();
    ask.collections[0].sync_token = "old".into();
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &server(), NOW, &Limits::default());
    assert_eq!(out.collections[0].changes.len(), 2);
}

#[test]
fn a_home_that_was_left_out_makes_the_listing_incomplete() {
    let s = server();
    let mut ask = ask(&s, true, Vec::new());
    ask.homes.push("https://outlook.office365.com/dav/cal/".into());
    let out = read(&s, &ask);
    let listing = out.listing.as_ref().unwrap();
    assert!(!listing.complete, "the calendars of a home that wasn't asked are not known to be gone");
    assert_eq!(listing.calendars.len(), 2, "what the others list is still there");
    assert!(out.skipped >= 1);
}

#[test]
fn an_address_off_the_origin_or_outside_the_collection_is_skipped_and_never_asked_for() {
    let evil = "https://evil.example.net/cal/work/x.ics";
    let x = Scripted::new(move |request| match kind(request) {
        "members" => multistatus(&format!(
            "{}{}{}{}{}",
            member("/cal/work/ok.ics", "\"1\"", None),
            member(evil, "\"2\"", None),
            member("/cal/other/steal.ics", "\"3\"", None),
            member("/cal/work/../other/up.ics", "\"4\"", None),
            member("/cal/work/%2e%2e/other/up2.ics", "\"5\"", None),
        )),
        "state" => multistatus(""),
        // A multiget answer that names what was not asked for, with data.
        _ => multistatus(&format!(
            "{}{}",
            member("/cal/work/ok.ics", "\"1\"", Some("BEGIN:VCALENDAR\nEND:VCALENDAR")),
            member("/cal/other/steal.ics", "\"3\"", Some("BEGIN:VCALENDAR\nEND:VCALENDAR")),
        )),
    });
    let held = Held::new(Some(PASSWORD));
    let out = run(&collection_ask(), &held.lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    let hrefs: Vec<&str> = work.changes.iter().map(|c| c.href.as_str()).collect();
    assert_eq!(hrefs, vec![format!("{ORIGIN}/cal/work/ok.ics")]);
    assert!(out.skipped >= 3);
    assert!(x.seen.borrow().iter().all(|(url, _)| url.starts_with(ORIGIN)), "nothing was asked of another server");
    assert!(x.seen.borrow().iter().all(|(url, _)| !url.contains("other") && !url.contains("..")));
    assert!(work.members.is_none(), "a listing with members left out is no proof of what is gone");
}

#[test]
fn a_redirect_to_another_origin_goes_nowhere_and_takes_no_password() {
    let x = Scripted::new(|_| HttpResponse::new(301, Vec::new()).with_field("location", "https://evil.example.net/cal/work/"));
    let held = Held::new(Some(PASSWORD));
    let out = run(&collection_ask(), &held.lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::Redirected);
    assert_eq!(x.seen.borrow().len(), 1, "no request was made to the other origin");
    assert!(x.seen.borrow().iter().all(|(_, credential)| !credential));
    assert!(held.asked.borrow().is_empty(), "the password was not even looked up");
}

#[test]
fn a_redirect_on_the_same_origin_is_followed_one_step_at_a_time_up_to_five() {
    let hops = RefCell::new(0usize);
    let x = Scripted::new(move |request| {
        if request.url().ends_with("/moved/") {
            multistatus("")
        } else {
            *hops.borrow_mut() += 1;
            HttpResponse::new(307, Vec::new()).with_field("location", "/cal/moved/")
        }
    });
    let held = Held::new(Some(PASSWORD));
    let out = run(&collection_ask(), &held.lookup(), &x, NOW, &Limits::default());
    // (Followed, and the run goes on; but a collection answered at another address is not read,
    // and that is said.)
    assert_eq!(out.problem, DavProblem::ServerErrors);
    // Endless redirects stop after five.
    let endless = Scripted::new(|request| HttpResponse::new(302, Vec::new()).with_field("location", &format!("{}x/", request.url())));
    let out = run(&collection_ask(), &held.lookup(), &endless, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::Redirected);
    assert_eq!(endless.seen.borrow().len(), MAX_REDIRECTS + 1);
    // And a redirect to plain http is another origin.
    let downgrade = Scripted::new(|_| HttpResponse::new(302, Vec::new()).with_field("location", "http://dav.example.org/cal/work/"));
    let out = run(&collection_ask(), &held.lookup(), &downgrade, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::Redirected);
    assert_eq!(downgrade.seen.borrow().len(), 1);
}

fn challenge(scheme: &'static str) -> Scripted {
    // (Whatever it is sent, it asks again: the credential is never good enough.)
    Scripted::new(move |_| HttpResponse::new(401, Vec::new()).with_field("www-authenticate", scheme))
}

#[test]
fn a_wrong_password_stops_the_run_after_one_try() {
    let x = challenge("Basic realm=\"x\"");
    let held = Held::new(Some("wrong"));
    let out = run(&collection_ask(), &held.lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::WrongPassword);
    assert_eq!(x.seen.borrow().len(), 2, "one without, one with the credential: no more (lockouts)");
    assert!(out.signed_in.is_empty());
}

#[test]
fn no_password_is_asked_for_and_none_is_sent() {
    let x = challenge("Basic realm=\"x\"");
    let held = Held::new(None);
    let out = run(&collection_ask(), &held.lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::NeedsPassword);
    assert!(x.seen.borrow().iter().all(|(_, credential)| !credential));
    assert_eq!(held.asked.borrow().len(), 1);
}

#[test]
fn a_challenge_that_isnt_basic_gets_no_password() {
    let x = challenge("Digest realm=\"x\", nonce=\"y\"");
    let held = Held::new(Some(PASSWORD));
    let out = run(&collection_ask(), &held.lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::UnsupportedSignIn);
    assert!(held.asked.borrow().is_empty());
}

#[test]
fn a_password_is_asked_for_only_by_the_origin_that_challenged() {
    // Two homes on two origins; only the second challenges.
    let x = Scripted::new(|request| {
        if request.url().starts_with("https://cal.example.org") && request.field("authorization").is_none() {
            HttpResponse::new(401, Vec::new()).with_field("www-authenticate", "Basic realm=\"x\"")
        } else {
            multistatus("")
        }
    });
    let mut ask = collection_ask();
    ask.homes = vec![format!("{ORIGIN}/cal/"), "https://cal.example.org/cal/".into()];
    ask.list_homes = true;
    ask.collections.clear();
    let held = Held::new(Some(PASSWORD));
    let out = run(&ask, &held.lookup(), &x, NOW, &Limits::default());
    assert_eq!(*held.asked.borrow(), vec!["https://cal.example.org".to_string()]);
    assert_eq!(out.signed_in, vec!["https://cal.example.org".to_string()]);
    assert!(x.seen.borrow().iter().filter(|(url, _)| url.starts_with(ORIGIN)).all(|(_, credential)| !credential));
}

#[test]
fn a_collection_on_an_origin_that_is_no_home_is_never_asked() {
    let x = Scripted::new(|_| multistatus(""));
    let mut ask = collection_ask();
    ask.collections = vec![DavAskCollection { href: "https://elsewhere.example.net/cal/x/".into(), ..Default::default() }];
    let held = Held::new(Some(PASSWORD));
    let out = run(&ask, &held.lookup(), &x, NOW, &Limits::default());
    assert!(x.seen.borrow().is_empty());
    assert!(out.collections.is_empty() && out.skipped >= 1);
}

#[test]
fn microsoft_and_google_hosts_are_never_contacted() {
    for home in [
        "https://outlook.office365.com/cal/",
        "https://apidata.googleusercontent.com/caldav/v2/x/",
        "http://dav.example.org/cal/",
        "https://user:pw@dav.example.org/cal/",
    ] {
        let x = Scripted::new(|_| multistatus(""));
        let mut ask = collection_ask();
        ask.homes = vec![home.into()];
        ask.collections = vec![DavAskCollection { href: home.into(), ..Default::default() }];
        let held = Held::new(Some(PASSWORD));
        let out = run(&ask, &held.lookup(), &x, NOW, &Limits::default());
        assert_eq!(out.problem, DavProblem::NotCalendarServer, "{home}");
        assert!(x.seen.borrow().is_empty(), "{home}");
        assert!(held.asked.borrow().is_empty());
    }
}

#[test]
fn a_server_that_says_not_now_is_left_alone_for_as_long_as_it_asks() {
    let x = Scripted::new(|_| HttpResponse::new(503, Vec::new()).with_field("retry-after", "120"));
    let held = Held::new(Some(PASSWORD));
    let out = run(&collection_ask(), &held.lookup(), &x, NOW, &Limits::default());
    assert_eq!((out.problem, out.wait), (DavProblem::Busy, 120));
    let x = Scripted::new(|_| HttpResponse::new(429, Vec::new()).with_field("retry-after", "999999"));
    let out = run(&collection_ask(), &held.lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.wait, 600, "at most ten minutes");
}

#[test]
fn a_run_stops_at_its_limits_and_a_certificate_or_connection_failure_ends_it() {
    let s = server();
    s.put_directly("work", "a.ics", &event("a", "20261201T100000Z")).unwrap();
    let held = Held::new(Some(PASSWORD));
    let out = run(&ask(&s, true, Vec::new()), &held.lookup(), &s, NOW, &Limits { requests: 2, ..Limits::default() });
    assert_eq!(out.problem, DavProblem::Limits);
    let out = run(&ask(&s, true, Vec::new()), &held.lookup(), &s, NOW, &Limits { raw_bytes: 10, ..Limits::default() });
    assert_eq!(out.problem, DavProblem::Limits);
    struct Down(TransportError);
    impl HttpExchange for Down {
        fn exchange(&self, _: &HttpRequest) -> Result<HttpResponse, TransportError> {
            Err(self.0)
        }
    }
    let out = run(&collection_ask(), &held.lookup(), &Down(TransportError::Certificate), NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::Certificate);
    let out = run(&collection_ask(), &held.lookup(), &Down(TransportError::Unreachable), NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::Unreachable);
}

#[test]
fn hostile_answers_never_panic_and_never_hold_more_than_the_caps() {
    let junk: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"<not xml".to_vec(),
        b"<?xml version=\"1.0\"?><!DOCTYPE x [<!ENTITY a \"b\">]><d:multistatus xmlns:d=\"DAV:\">&a;</d:multistatus>".to_vec(),
        vec![0xff; 4096],
        format!("{XML}{}</d:multistatus>", "<d:response><d:href>/cal/work/a.ics</d:href></d:response>".repeat(50)).into_bytes(),
    ];
    for body in junk {
        let status = [200u16, 207, 404, 500][body.len() % 4];
        let x = Scripted::new(move |_| HttpResponse::new(status, body.clone()));
        let held = Held::new(Some(PASSWORD));
        let mut ask = collection_ask();
        ask.list_homes = true;
        let out = run(&ask, &held.lookup(), &x, NOW, &Limits::default());
        assert!(out.collections.iter().all(|c| c.changes.len() <= mailcraft_model::dav_down::MAX_CHANGES));
    }
}

#[test]
fn a_response_comes_through_the_wire_with_its_fields_and_its_classes() {
    let answer = https::Response {
        status: 401,
        body: b"x".to_vec(),
        location: Some("/x".into()),
        challenges: vec!["Basic realm=\"a\"".into(), "Digest realm=\"b\"".into()],
        retry_after: Some("5".into()),
        dav: vec!["1".into(), "calendar-access".into(), "<http://example.org/ns,1>".into()],
        etag: Some("\"e\"".into()),
        ..Default::default()
    };
    let response = response_of(answer);
    assert_eq!(status::challenges(&response), vec!["basic", "digest"]);
    assert_eq!(status::dav_classes(&response), vec!["1", "calendar-access", "<http://example.org/ns,1>"]);
    assert_eq!(response.header("etag"), Some("\"e\""));
    assert_eq!(response.header("location"), Some("/x"));
}

#[test]
fn nothing_of_a_run_shows_in_the_debug_output_of_its_parts() {
    let ask = collection_ask();
    let shown = format!("{ask:?} {:?}", Limits::default());
    assert!(!shown.contains("example.org") && !shown.contains(USER));
}

/// A calendar server on this machine, in plain `http` (the one case a password may travel so),
/// that answers from a script: no credential is a 401, a PROPFIND lists one calendar, a sync
/// report names one resource, a multiget has it.
fn local_server() -> (u16, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        // (Until the client has been quiet for two seconds.)
        let _ = listener.set_nonblocking(true);
        let mut quiet = std::time::Instant::now();
        while quiet.elapsed() < std::time::Duration::from_secs(2) {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            quiet = std::time::Instant::now();
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text
                        .to_ascii_lowercase()
                        .split("content-length: ")
                        .nth(1)
                        .and_then(|l| l.split("\r\n").next()?.parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&request).to_string();
            seen.push(text.lines().next().unwrap_or_default().to_string());
            let authorized = text.to_ascii_lowercase().contains("authorization: basic");
            let body = if text.contains("calendar-multiget") {
                member("/cal/work/a.ics", "\"1\"", Some("BEGIN:VCALENDAR\nEND:VCALENDAR"))
            } else if text.starts_with("PROPFIND") && text.contains("getetag") {
                member("/cal/work/a.ics", "\"1\"", None)
            } else if text.contains("calendar-multiget-never") {
                member("/cal/work/a.ics", "\"1\"", Some("BEGIN:VCALENDAR\nEND:VCALENDAR"))
            } else {
                "<d:response><d:href>/cal/work/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>Work</d:displayname></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>".to_string()
            };
            let reply = if !authorized {
                "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
            } else {
                let xml = format!("{XML}{body}<d:sync-token>t1</d:sync-token></d:multistatus>");
                format!(
                    "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                    xml.len()
                )
            };
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
        seen
    });
    (port, handle)
}

#[test]
fn the_wire_carries_a_job_to_a_server_on_this_machine_and_the_password_only_after_the_challenge() {
    let (port, server) = local_server();
    let ask = DavAsk {
        account: 7,
        email: USER.into(),
        username: USER.into(),
        homes: vec![format!("http://127.0.0.1:{port}/cal/")],
        list_homes: true,
        ..Default::default()
    };
    let asked = RefCell::new(Vec::new());
    let passwords = |origin: &str| {
        asked.borrow_mut().push(origin.to_string());
        Some(PASSWORD.to_string())
    };
    let wire = Wire::new("");
    let out = run(&ask, &passwords, &wire, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::None, "{out:?}");
    assert_eq!(out.listing.as_ref().map(|l| l.calendars.len()), Some(1));
    assert_eq!(out.collections.len(), 1);
    assert_eq!(out.collections[0].changes.len(), 1);
    assert_eq!(*asked.borrow(), vec![format!("http://127.0.0.1:{port}")]);
    drop(wire);
    let seen = server.join().unwrap();
    assert!(seen[0].starts_with("PROPFIND "), "{seen:?}");
    assert!(seen.iter().any(|line| line.starts_with("REPORT ")));
}

#[test]
fn a_certificate_the_system_does_not_trust_is_kept_for_the_window_and_ends_the_run() {
    // Nothing listens there: this is only the connection failing, which is not a certificate.
    let wire = Wire::new("");
    assert!(wire.certificate().is_none());
    let ask = DavAsk {
        homes: vec!["http://127.0.0.1:9/cal/".into()],
        collections: vec![DavAskCollection { href: "http://127.0.0.1:9/cal/work/".into(), ..Default::default() }],
        ..collection_ask()
    };
    let out = run(&ask, &|_| Some(PASSWORD.into()), &wire, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::Unreachable);
}

mod property {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// Whatever a server answers, with whatever status and headers, a run ends without a panic,
        /// holds to its caps, takes nothing from outside the collection and sends no password to an
        /// origin that wasn't asked about.
        #[test]
        fn arbitrary_answers_never_panic_and_stay_inside_the_caps(
            bytes in proptest::collection::vec(any::<u8>(), 0..2_000),
            status in prop::sample::select(vec![200u16, 207, 301, 302, 401, 403, 404, 409, 412, 429, 500, 503, 507]),
            location in "[ -~]{0,60}",
            challenge in "[ -~]{0,40}",
            list in any::<bool>(),
        ) {
            let x = Scripted::new(move |_| {
                HttpResponse::new(status, bytes.clone()).with_field("location", &location).with_field("www-authenticate", &challenge).with_field("retry-after", "1")
            });
            let mut ask = collection_ask();
            ask.list_homes = list;
            let held = Held::new(Some(PASSWORD));
            let out = run(&ask, &held.lookup(), &x, NOW, &Limits::default());
            prop_assert!(out.collections.iter().all(|c| c.changes.len() <= mailcraft_model::dav_down::MAX_CHANGES));
            prop_assert!(out.wait <= 600);
            prop_assert!(x.seen.borrow().iter().all(|(url, _)| url.starts_with(ORIGIN)));
            prop_assert!(held.asked.borrow().iter().all(|origin| origin == ORIGIN));
        }
    }
}

// ---- the invariants, held by design (round 5) -------------------------------------------------

/// A home listing: the home, and one child per `children` entry (a whole `<d:response>`).
fn home_listing(children: &str) -> HttpResponse {
    multistatus(&format!(
        "<d:response><d:href>/cal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>{children}"
    ))
}

fn calendar_child(href: &str) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>W</d:displayname></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    )
}

#[test]
fn i1_a_calendar_whose_properties_failed_makes_the_listing_incomplete() {
    let x = Scripted::new(|request| {
        if request.field("depth") == Some("1") && request.url().ends_with("/cal/") {
            home_listing(&format!(
                "{}<d:response><d:href>/cal/other/</d:href><d:propstat><d:prop><d:resourcetype/><d:displayname/></d:prop><d:status>HTTP/1.1 503 Service Unavailable</d:status></d:propstat></d:response>",
                calendar_child("/cal/work/")
            ))
        } else {
            multistatus("")
        }
    });
    let mut ask = collection_ask();
    ask.list_homes = true;
    ask.collections.clear();
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let listing = out.listing.as_ref().unwrap();
    assert!(!listing.complete, "a child that couldn't be told apart may be a calendar: nothing is known to be gone");
}

#[test]
fn i1_a_collection_that_was_redirected_is_no_proof_that_anything_is_gone() {
    let x = Scripted::new(|request| {
        let url = request.url().to_string();
        if url.ends_with("/cal/work/") {
            HttpResponse::new(301, Vec::new()).with_field("location", "/cal/moved/")
        } else {
            // The moved collection, with its members under its new address.
            let body = String::from_utf8_lossy(request.body()).to_string();
            if body.contains("multiget") {
                multistatus(&member("/cal/moved/a.ics", "\"1\"", Some(DOC)))
            } else {
                multistatus(&format!("{}<d:sync-token>t</d:sync-token>", member("/cal/moved/a.ics", "\"1\"", None)))
            }
        }
    });
    let out = run(&collection_ask(), &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    assert!(work.members.is_none() && !work.complete, "answered at another address: nothing is read, nothing is gone: {work:?}");
}

#[test]
fn i2_a_multiget_too_large_to_hold_is_asked_for_one_by_one() {
    struct Big;
    impl HttpExchange for Big {
        fn exchange(&self, request: &HttpRequest) -> Result<HttpResponse, TransportError> {
            let body = String::from_utf8_lossy(request.body()).to_string();
            if body.contains("multiget") {
                return Err(TransportError::BodyTooLarge);
            }
            if request.method().as_str() == "GET" {
                return Ok(HttpResponse::new(200, DOC.as_bytes().to_vec()).with_field("etag", "\"1\""));
            }
            Ok(multistatus(&format!(
                "{}{}<d:sync-token>t</d:sync-token>",
                member("/cal/work/a.ics", "\"1\"", None),
                member("/cal/work/b.ics", "\"1\"", None)
            )))
        }
    }
    let out = run(&collection_ask(), &Held::new(Some(PASSWORD)).lookup(), &Big, NOW, &Limits::default());
    assert_eq!(out.collections[0].changes.len(), 2, "each came by itself");
}

#[test]
fn i2_a_change_pass_the_server_refuses_reads_the_collection_afresh_instead_of_stalling() {
    let x = Scripted::new(|request| {
        let body = String::from_utf8_lossy(request.body()).to_string();
        if body.contains("sync-collection") {
            HttpResponse::new(501, Vec::new())
        } else if body.contains("multiget") {
            multistatus(&member("/cal/work/a.ics", "\"1\"", Some(DOC)))
        } else {
            multistatus(&format!("{}{}", base("/cal/work/", "200 OK"), member("/cal/work/a.ics", "\"1\"", None)))
        }
    });
    let mut ask = collection_ask();
    ask.collections[0].sync_token = "old".into();
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    assert!(work.complete, "read another way: {work:?}");
    assert_eq!(work.changes.len(), 1);
}

// ---- found in review ---------------------------------------------------------------------------

#[test]
fn r_a_home_answered_at_another_address_or_listing_calendars_outside_it_is_no_whole_listing() {
    let held = Held::new(Some(PASSWORD));
    let mut ask = collection_ask();
    ask.list_homes = true;
    ask.collections.clear();
    let moved = Scripted::new(|request| {
        if request.url().ends_with("/cal/") {
            HttpResponse::new(301, Vec::new()).with_field("location", "/cal2/")
        } else {
            home_listing(&calendar_child("/cal2/work/"))
        }
    });
    let out = run(&ask, &held.lookup(), &moved, NOW, &Limits::default());
    assert!(!out.listing.as_ref().unwrap().complete, "redirected");
    let outside = Scripted::new(|_| home_listing(&format!("{}{}", calendar_child("/cal/work/"), calendar_child("/other/x/"))));
    let out = run(&ask, &held.lookup(), &outside, NOW, &Limits::default());
    let listing = out.listing.as_ref().unwrap();
    assert!(!listing.complete);
    assert_eq!(listing.calendars.len(), 1, "only what is inside the home");
}

#[test]
fn r_a_resource_the_server_refuses_for_good_does_not_hold_the_read_up() {
    let x = Scripted::new(|request| match kind(request) {
        "members" => multistatus(&format!("{}{}", base("/cal/work/", "200 OK"), member("/cal/work/a.ics", "\"1\"", None))),
        "state" => multistatus(""),
        "multiget" => HttpResponse::new(501, Vec::new()),
        _ => HttpResponse::new(403, Vec::new()),
    });
    let out = run(&collection_ask(), &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    assert!(out.collections[0].complete, "{:?}", out.collections[0]);
}

#[test]
fn r_a_member_the_server_could_not_describe_keeps_the_read_from_being_all() {
    let x = Scripted::new(|request| match kind(request) {
        "members" => multistatus(&format!(
            "{}<d:response><d:href>/cal/work/b.ics</d:href><d:status>HTTP/1.1 503 Service Unavailable</d:status></d:response>",
            member("/cal/work/a.ics", "\"1\"", None)
        )),
        "state" => multistatus(""),
        _ => multistatus(&member("/cal/work/a.ics", "\"1\"", Some(DOC))),
    });
    let out = run(&collection_ask(), &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    assert!(!work.complete && work.members.is_none(), "{work:?}");
}

#[test]
fn r_a_change_page_of_members_without_an_etag_cut_short_by_the_bytes_goes_on_at_the_next_run() {
    let x = Scripted::new(|request| match kind(request) {
        "sync" => multistatus(&format!("{}{}<d:sync-token>t2</d:sync-token>", bare("/cal/work/a.ics"), bare("/cal/work/b.ics"))),
        _ => multistatus(
            &asked(&String::from_utf8_lossy(request.body())).iter().map(|n| member(&format!("/cal/work/{n}"), "", Some(DOC))).collect::<String>(),
        ),
    });
    let limits = Limits { raw_bytes: 1, ..Limits::default() };
    let mut ask = collection_ask();
    ask.collections[0].sync_token = "t1".into();
    let mut took: Vec<String> = Vec::new();
    let mut done = false;
    for _ in 0..3 {
        ask.collections[0].known = took.iter().map(|h| DavKnown { href: h.clone(), etag: String::new() }).collect();
        let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &limits);
        let work = &out.collections[0];
        took.extend(work.changes.iter().map(|c| c.href.clone()));
        if work.complete {
            assert_eq!(work.sync_token, "t2");
            done = true;
            break;
        }
    }
    let mut each = took.clone();
    each.sort();
    each.dedup();
    assert!(done && each.len() == 2 && took.len() == 2, "each came once, and the read got past them: {took:?}");
}

#[test]
fn r_a_member_that_can_never_be_had_is_said_without_its_document() {
    let x = Scripted::new(|request| match kind(request) {
        "members" => multistatus(&format!("{}{}", base("/cal/work/", "200 OK"), member("/cal/work/a.ics", "\"1\"", None))),
        "state" => multistatus(""),
        "multiget" => HttpResponse::new(501, Vec::new()),
        _ => HttpResponse::new(403, Vec::new()),
    });
    let out = run(&collection_ask(), &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    assert_eq!(work.changes.len(), 1);
    assert!(work.changes[0].raw.is_none() && work.changes[0].etag == "\"1\"", "so that it is not asked for at every run");
}

#[test]
fn r_a_change_the_server_could_not_describe_or_gave_no_etag_for_holds_the_token_until_it_is_read() {
    // A change page names a.ics with a failed status and b.ics, here under an ETag, without one:
    // neither can be passed over as unchanged.
    let page = format!(
        "<d:response><d:href>/cal/work/a.ics</d:href><d:status>HTTP/1.1 503 Service Unavailable</d:status></d:response>{}<d:sync-token>t2</d:sync-token>",
        bare("/cal/work/b.ics")
    );
    let ask = || {
        let mut ask = collection_ask();
        ask.collections[0].sync_token = "t1".into();
        ask.collections[0].known = vec![
            DavKnown { href: format!("{ORIGIN}/cal/work/a.ics"), etag: String::new() },
            DavKnown { href: format!("{ORIGIN}/cal/work/b.ics"), etag: "\"1\"".into() },
        ];
        ask
    };
    // The server can't give a.ics now: the token stays, so the next run asks again.
    let busy = page.clone();
    let x = Scripted::new(move |request| match kind(request) {
        "sync" => multistatus(&busy),
        "state" => multistatus(""),
        "multiget" => multistatus(&member("/cal/work/b.ics", "\"2\"", Some(DOC))),
        // (Not a 503: that is a busy server, and ends the run.)
        _ => HttpResponse::new(500, Vec::new()),
    });
    let out = run(&ask(), &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    assert!(!work.complete && work.sync_token == "t1", "{work:?}");
    assert!(work.changes.iter().any(|c| c.href.ends_with("b.ics")), "b.ics is read: {work:?}");
    // Once it can, both are read and the token moves past them.
    let x = Scripted::new(move |request| match kind(request) {
        "sync" => multistatus(&page),
        _ => multistatus(
            &asked(&String::from_utf8_lossy(request.body()))
                .iter()
                .map(|n| member(&format!("/cal/work/{n}"), "\"2\"", Some(DOC)))
                .collect::<String>(),
        ),
    });
    let out = run(&ask(), &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    let work = &out.collections[0];
    assert!(work.complete && work.sync_token == "t2", "{work:?}");
    assert_eq!(names(work), vec!["a.ics".to_string(), "b.ics".to_string()], "{work:?}");
}

// ---- found in review, round 6 ------------------------------------------------------------------

/// The collection itself, as a Depth 1 listing names it first, with `status`.
fn base(href: &str, status: &str) -> String {
    format!(
        "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop><d:status>HTTP/1.1 {status}</d:status></d:propstat></d:response>"
    )
}

#[test]
fn r6_an_empty_answer_or_one_that_does_not_describe_the_collection_is_no_whole_listing() {
    let listed = |members: String| {
        let x = Scripted::new(move |request| match kind(request) {
            "members" => multistatus(&members),
            _ => multistatus(""),
        });
        let out = run(&collection_ask(), &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
        out.collections.first().map(|c| (c.members.clone(), c.complete)).unwrap_or((None, false))
    };
    assert_eq!(listed(String::new()), (None, false), "<d:multistatus/>: nothing is gone, and the change tag is not kept");
    let gone = "<d:response><d:href>/cal/work/</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>".to_string();
    assert_eq!(listed(gone), (None, false), "the collection itself not found");
    assert_eq!(listed(member("/cal/work/a.ics", "\"1\"", None)).0, None, "members without the collection itself");
    // The collection itself, and nothing in it: an empty calendar (RFC 4918), which is whole.
    assert_eq!(listed(base("/cal/work/", "200 OK")), (Some(Vec::new()), true));
}

#[test]
fn r6_an_empty_home_listing_is_not_whole() {
    let mut ask = collection_ask();
    ask.list_homes = true;
    ask.collections.clear();
    let x = Scripted::new(|_| multistatus(""));
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    assert!(!out.listing.as_ref().unwrap().complete, "no calendar is gone on an answer that says nothing of the home");
    let x = Scripted::new(|_| home_listing(""));
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    assert!(out.listing.as_ref().unwrap().complete, "a home with no calendars, which says so");
}

#[test]
fn r6_a_document_or_a_home_that_keeps_failing_is_said_as_a_problem() {
    // A change pass whose one document the server fails to give.
    let x = Scripted::new(|request| match kind(request) {
        "sync" => multistatus(&format!("{}<d:sync-token>t2</d:sync-token>", member("/cal/work/a.ics", "\"2\"", None))),
        "state" => multistatus(""),
        _ => HttpResponse::new(500, Vec::new()),
    });
    let mut ask = collection_ask();
    ask.collections[0].sync_token = "t1".into();
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::ServerErrors, "{:?}", out.collections);
    assert_eq!(out.collections[0].sync_token, "t1", "the token stays");
    // A home whose listing fails.
    let mut ask = collection_ask();
    ask.list_homes = true;
    ask.collections.clear();
    let x = Scripted::new(|_| HttpResponse::new(500, Vec::new()));
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.problem, DavProblem::ServerErrors);
    assert!(!out.listing.as_ref().unwrap().complete);
}

#[test]
fn r6_a_listed_calendar_the_engine_has_no_room_for_is_not_read() {
    let x = Scripted::new(|request| {
        if request.field("depth") == Some("1") && request.url().ends_with("/cal/") {
            home_listing(&format!("{}{}", calendar_child("/cal/work/"), calendar_child("/cal/new/")))
        } else {
            multistatus("")
        }
    });
    let mut ask = collection_ask();
    ask.list_homes = true;
    ask.no_room = true;
    let out = run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default());
    assert_eq!(out.listing.as_ref().unwrap().calendars.len(), 2, "both are said to the engine");
    assert!(x.seen.borrow().iter().all(|(url, _)| !url.contains("/cal/new/")), "but the new one is never asked: {:?}", x.seen.borrow());
}

#[test]
fn r6_the_generation_asked_under_comes_back_with_the_download() {
    let mut ask = collection_ask();
    ask.generation = 3;
    let x = Scripted::new(|_| multistatus(""));
    assert_eq!(run(&ask, &Held::new(Some(PASSWORD)).lookup(), &x, NOW, &Limits::default()).generation, 3);
}
