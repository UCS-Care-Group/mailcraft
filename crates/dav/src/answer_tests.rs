// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use super::*;

fn o() -> Origin {
    Origin::parse("https://dav.example.org").unwrap()
}
const BASE: &str = "https://dav.example.org/cal/owner/";
const NS: &str = "xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:caldav\" xmlns:card=\"urn:ietf:params:xml:ns:carddav\" xmlns:a=\"http://apple.com/ns/ical/\" xmlns:cs=\"http://calendarserver.org/ns/\"";

fn ms(inner: &str) -> Multistatus {
    Multistatus::parse(format!("<d:multistatus {NS}>{inner}</d:multistatus>").as_bytes(), &o(), BASE).unwrap()
}
fn prop(href: &str, props: &str) -> String {
    format!("<d:response><d:href>{href}</d:href><d:propstat><d:prop>{props}</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>")
}

#[test]
fn reads_a_calendar_collection() {
    let m = ms(&prop(
        "/cal/owner/work/",
        "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype><d:displayname>  Work </d:displayname><a:calendar-color>#FF8800FF</a:calendar-color>\
         <c:supported-calendar-component-set><c:comp name=\"vevent\"/><c:comp name=\"VTODO\"/></c:supported-calendar-component-set>\
         <d:supported-report-set><d:supported-report><d:report><d:sync-collection/></d:report></d:supported-report><d:supported-report><d:report><c:calendar-multiget/></d:report></d:supported-report></d:supported-report-set>\
         <d:current-user-privilege-set><d:privilege><d:read/></d:privilege><d:privilege><d:write-content/></d:privilege></d:current-user-privilege-set>\
         <cs:getctag>ct-1</cs:getctag><d:sync-token>tok-9</d:sync-token>",
    ));
    let e = m.entries.first().unwrap();
    assert_eq!(e.href, "https://dav.example.org/cal/owner/work/");
    assert!(e.is_collection && e.is_calendar && !e.is_addressbook && e.holds_events() && e.supports_sync());
    assert_eq!((e.display_name.as_deref(), e.color), (Some("Work"), Some([255, 136, 0])));
    assert_eq!(e.components.as_deref(), Some(&["VEVENT".to_owned(), "VTODO".to_owned()][..]));
    assert_eq!((e.read_only, e.ctag.as_deref(), e.sync_token.as_deref()), (Some(false), Some("ct-1"), Some("tok-9")));
}

#[test]
fn read_only_follows_the_privileges() {
    let only_read = "<d:current-user-privilege-set><d:privilege><d:read/></d:privilege></d:current-user-privilege-set>";
    assert_eq!(ms(&prop("/a/", only_read)).entries[0].read_only, Some(true));
    let all = "<d:current-user-privilege-set><d:privilege><d:all/></d:privilege></d:current-user-privilege-set>";
    assert_eq!(ms(&prop("/a/", all)).entries[0].read_only, Some(false));
    assert_eq!(ms(&prop("/a/", "<d:displayname>x</d:displayname>")).entries[0].read_only, None);
}

#[test]
fn a_calendar_without_a_component_set_is_taken_as_events_and_a_task_list_is_not() {
    let rt = "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>";
    assert!(ms(&prop("/a/", rt)).entries[0].holds_events());
    let tasks = format!("{rt}<c:supported-calendar-component-set><c:comp name=\"VTODO\"/></c:supported-calendar-component-set>");
    assert!(!ms(&prop("/a/", &tasks)).entries[0].holds_events());
}

#[test]
fn principal_and_homes_resolve_on_origin_and_off_origin_ones_are_dropped() {
    let m = ms(&prop(
        "/",
        "<d:current-user-principal><d:href>/p/owner/</d:href></d:current-user-principal>\
         <c:calendar-home-set><d:href>/cal/owner/</d:href><d:href>https://evil.example.net/cal/</d:href><d:href>/cal/owner/</d:href></c:calendar-home-set>\
         <card:addressbook-home-set><d:href>https://dav.example.org/card/owner/</d:href></card:addressbook-home-set>",
    ));
    let e = &m.entries[0];
    assert_eq!(e.principal.as_deref(), Some("https://dav.example.org/p/owner/"));
    assert_eq!(e.calendar_homes, ["https://dav.example.org/cal/owner/"]);
    assert_eq!(e.addressbook_homes, ["https://dav.example.org/card/owner/"]);
    assert_eq!(m.skipped, 1);
    let off = ms(&prop("/", "<d:current-user-principal><d:href>https://evil.example.net/p/</d:href></d:current-user-principal>"));
    assert!(off.entries[0].principal.is_none());
}

#[test]
fn off_origin_responses_are_skipped_and_counted() {
    let m = ms(&format!(
        "{}{}{}",
        prop("https://evil.example.net/cal/owner/x.ics", "<d:getetag>\"1\"</d:getetag>"),
        prop("/cal/owner/a.ics", "<d:getetag>\"2\"</d:getetag>"),
        prop("/../x", "<d:getetag>\"3\"</d:getetag>")
    ));
    assert_eq!(m.entries.len(), 1);
    assert_eq!(m.skipped, 2);
    assert_eq!(m.entries[0].etag.as_deref(), Some("\"2\""));
}

#[test]
fn a_sync_answer_has_members_removals_token_and_truncation() {
    let removed = "<d:response><d:href>/cal/owner/gone.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>";
    let cut = "<d:response><d:href>/cal/owner/</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>";
    let m = ms(&format!("{}{removed}{cut}<d:sync-token>t-5</d:sync-token>", prop("/cal/owner/a.ics", "<d:getetag>\"1\"</d:getetag>")));
    assert_eq!(m.sync_token.as_deref(), Some("t-5"));
    assert!(m.truncated);
    assert_eq!(m.entries.len(), 2);
    assert!(m.entries[1].is_gone());
    assert_eq!(m.members().count(), 1);
}

#[test]
fn missing_properties_are_not_deletions() {
    let m = ms(
        "<d:response><d:href>/cal/owner/a.ics</d:href><d:propstat><d:prop><d:getetag/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response>",
    );
    let e = &m.entries[0];
    assert!(!e.is_gone() && e.etag.is_none());
}

#[test]
fn tokens_etags_names_colours_and_data_are_bounded() {
    let long = "x".repeat(crate::request::MAX_TOKEN + 1);
    assert_eq!(
        Multistatus::parse(format!("<d:multistatus {NS}><d:sync-token>{long}</d:sync-token></d:multistatus>").as_bytes(), &o(), BASE),
        Err(AnswerError::Token)
    );
    let m = ms(&format!(
        "{}{}",
        prop("/cal/owner/a.ics", &format!("<d:getetag>{long}</d:getetag>")),
        prop("/cal/owner/b.ics", "<d:getetag></d:getetag>")
    ));
    assert!(m.entries.is_empty());
    assert_eq!(m.skipped, 2);
    let name = "é".repeat(500);
    let e = ms(&prop("/a/", &format!("<d:displayname>{name}</d:displayname><cs:getctag>{long}</cs:getctag>"))).entries.remove(0);
    assert_eq!(e.display_name.unwrap().chars().count(), MAX_NAME);
    assert!(e.ctag.is_none());
    for bad in ["red", "#12", "#GGGGGG", "#1234567", "FF8800", "#ff88000", "# ff8800", "#ff88\u{e9}0"] {
        assert_eq!(color(bad), None, "{bad:?}");
    }
    assert_eq!(color("#aBcDeF"), Some([0xab, 0xcd, 0xef]));
    let big = "x".repeat(MAX_DATA + 1);
    let e = ms(&prop("/cal/owner/a.ics", &format!("<c:calendar-data>{big}</c:calendar-data>"))).entries.remove(0);
    assert!(e.data.is_none() && e.data_too_large);
    let e = ms(&prop("/cal/owner/a.ics", "<c:calendar-data>BEGIN:VCALENDAR</c:calendar-data>")).entries.remove(0);
    assert_eq!(e.data.as_deref(), Some("BEGIN:VCALENDAR"));
}

#[test]
fn entries_are_capped() {
    let one = "<d:response><d:href>/cal/owner/a.ics</d:href><d:status>HTTP/1.1 404 Not Found</d:status></d:response>";
    let m = ms(&one.repeat(MAX_ENTRIES + 5));
    assert_eq!(m.entries.len(), MAX_ENTRIES);
    assert!(m.capped);
}

#[test]
fn collections_are_sorted_passed_over_and_capped() {
    let cal = "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>";
    let book = "<d:resourcetype><d:collection/><card:addressbook/></d:resourcetype>";
    let tasks = format!("{cal}<c:supported-calendar-component-set><c:comp name=\"VTODO\"/></c:supported-calendar-component-set>");
    let inner = format!(
        "{}{}{}{}{}{}",
        prop("/cal/owner/", "<d:resourcetype><d:collection/></d:resourcetype>"),
        prop("/cal/owner/work/", cal),
        prop("/cal/owner/todo/", &tasks),
        prop("/cal/owner/personal.ics", "<d:resourcetype/>"),
        prop("/cal/owner/people/", book),
        prop("/cal/owner/inbox/", "<d:resourcetype><d:collection/></d:resourcetype>"),
    );
    let c = Collections::from_listing(ms(&inner));
    assert_eq!((c.calendars.len(), c.address_books.len(), c.passed_over, c.capped), (1, 1, 4, false));
    let many: String = (0..60).map(|i| prop(&format!("/cal/owner/c{i}/"), cal)).collect();
    let c = Collections::from_listing(ms(&many));
    assert_eq!(c.calendars.len(), MAX_COLLECTIONS);
    assert!(c.capped);
}

#[test]
fn address_data_types_are_read() {
    let m = ms(&prop(
        "/card/owner/b/",
        "<card:supported-address-data><card:address-data-type content-type=\"text/vcard\" version=\"4.0\"/></card:supported-address-data>",
    ));
    assert_eq!(m.entries[0].address_data, ["text/vcard;4.0"]);
}

#[test]
fn hostile_documents_are_refused_whole() {
    let o = o();
    for doc in [
        "",
        "not xml",
        "<d:multistatus xmlns:d=\"DAV:\"><d:response>",
        "<!DOCTYPE x [<!ENTITY a \"b\">]><d:multistatus xmlns:d=\"DAV:\"/>",
        "<d:propfind xmlns:d=\"DAV:\"/>",
    ] {
        assert!(Multistatus::parse(doc.as_bytes(), &o, BASE).is_err(), "{doc:?}");
    }
    assert!(Multistatus::parse(b"<d:multistatus xmlns:d=\"DAV:\"/>", &o, "https://evil.example.net/").is_err());
    assert!(status_code("HTTP/1.1 700 x").is_none() && status_code("200").is_none());
}

#[test]
fn only_the_asked_addresses_507_means_truncated() {
    let other = "<d:response><d:href>/cal/owner/member.ics</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>";
    let m = ms(other);
    assert!(!m.truncated);
    assert_eq!(m.entries.len(), 1);
    assert_eq!(m.entries[0].status, 507);
    let own = "<d:response><d:href>/cal/owner/</d:href><d:status>HTTP/1.1 507 Insufficient Storage</d:status></d:response>";
    assert!(ms(own).truncated);
    assert!(ms(own).entries.is_empty());
}

#[test]
fn empty_and_later_invalid_tokens_never_replace_a_valid_one() {
    assert_eq!(ms("<d:sync-token/>").sync_token, None);
    assert_eq!(ms("<d:sync-token></d:sync-token>").sync_token, None);
    let long = "x".repeat(crate::request::MAX_TOKEN + 1);
    let e = ms(&format!(
        "<d:response><d:href>/a/</d:href><d:propstat><d:prop><d:sync-token>good</d:sync-token><cs:getctag>c1</cs:getctag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>\
         <d:propstat><d:prop><d:sync-token>{long}</d:sync-token><cs:getctag></cs:getctag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>"
    ))
    .entries
    .remove(0);
    assert_eq!((e.sync_token.as_deref(), e.ctag.as_deref()), (Some("good"), Some("c1")));
}

#[test]
fn reports_must_be_in_their_own_namespace() {
    let set = "<d:supported-report-set>\
        <d:supported-report><d:report><c:sync-collection/></d:report></d:supported-report>\
        <d:supported-report><d:report><d:calendar-multiget/></d:report></d:supported-report>\
        <d:supported-report><d:report><d:sync-collection/></d:report></d:supported-report>\
        <d:supported-report><d:report><c:calendar-multiget/></d:report></d:supported-report></d:supported-report-set>";
    assert_eq!(ms(&prop("/a/", set)).entries[0].reports, ["sync-collection", "calendar-multiget"]);
    let wrong = "<d:supported-report-set><d:supported-report><d:report><c:sync-collection/></d:report></d:supported-report></d:supported-report-set>";
    assert!(!ms(&prop("/a/", wrong)).entries[0].supports_sync());
}

#[test]
fn debug_shows_counts_and_flags_only() {
    let m = ms(&prop(
        "/cal/owner/secret-name/",
        "<d:displayname>Private Name</d:displayname><d:getetag>\"etag-secret\"</d:getetag><c:calendar-data>BEGIN:VCALENDAR SECRETDATA</c:calendar-data>\
         <d:current-user-principal><d:href>/principal-secret/</d:href></d:current-user-principal>",
    ));
    let shown = format!("{m:?} {:?} {:?}", m.entries[0], Collections::from_listing(m.clone()));
    for secret in ["secret-name", "Private", "etag-secret", "SECRETDATA", "principal-secret"] {
        assert!(!shown.contains(secret), "{secret}");
    }
}
