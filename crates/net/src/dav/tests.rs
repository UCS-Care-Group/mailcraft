// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The CalDAV and CardDAV client: URLs, listings, events and cards, against a stand-in.

use super::*;
use crate::fake::FakeHttp;

#[test]
fn a_dav_service_is_found_at_the_well_known_address_without_a_password() {
    // A server answers /.well-known/caldav by leading to its service root, which asks for a
    // sign-in. The realm needn't name the product or the service: any DAV server will do.
    let server = FakeHttp::start(|n, _| {
        if n == 0 {
            b"HTTP/1.1 301 Moved Permanently\r\nLocation: /dav/caldav/\r\nContent-Length: 0\r\n\r\n".to_vec()
        } else {
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"example\"\r\nContent-Length: 0\r\n\r\n".to_vec()
        }
    });
    let (answer, final_url) = send_following(&server.url("/.well-known/caldav"), "PROPFIND", Some("0"), None, "", XML, &[], 16 * 1024).unwrap();
    assert!(is_dav_root(&answer));
    assert_eq!(service_url(&final_url), server.url("/dav/caldav/"));
    let request = server.request(0);
    assert!(request.starts_with("PROPFIND /.well-known/caldav HTTP/1.1\r\n"));
    assert!(!request.to_ascii_lowercase().contains("authorization"), "{request}");

    // A collection read without a sign-in is a service too; a web page is not one.
    let open = FakeHttp::answering(b"HTTP/1.1 207 Multi-Status\r\nContent-Length: 0\r\n\r\n".to_vec());
    let (answer, _) = send_following(&open.url("/.well-known/carddav"), "PROPFIND", Some("0"), None, "", XML, &[], 16 * 1024).unwrap();
    assert!(is_dav_root(&answer));
    let page = FakeHttp::answering(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec());
    let (answer, _) = send_following(&page.url("/.well-known/carddav"), "PROPFIND", Some("0"), None, "", XML, &[], 16 * 1024).unwrap();
    assert!(!is_dav_root(&answer));
}

#[test]
fn a_host_is_only_a_host() {
    assert_eq!(origin(" Mail.Example.org. "), Some("https://mail.example.org/".into()));
    assert_eq!(origin("[2001:db8::1]"), Some("https://[2001:db8::1]/".into()));
    assert_eq!(origin("2001:db8::1"), Some("https://[2001:db8::1]/".into()));
    for bad in ["", "not a host", "mail.example.org/../x", "mail.example.org?x"] {
        assert_eq!(origin(bad), None, "{bad}");
    }
}

#[test]
fn a_multistatus_names_its_collections() {
    let body = br#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
      <D:response><D:href>/caldav/ada@example.org/</D:href><D:propstat><D:prop>
        <D:resourcetype><D:collection/><C:calendar/></D:resourcetype><D:displayname>Calendar</D:displayname>
        <D:current-user-privilege-set><D:privilege><D:all/></D:privilege></D:current-user-privilege-set>
      </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
    </D:multistatus>"#;
    let responses = multistatus(body).unwrap();
    assert_eq!(responses.len(), 1);
    assert!(responses[0].has_resource_type("calendar"));
    assert_eq!(responses[0].prop_text("displayname").as_deref(), Some("Calendar"));
    assert_eq!(responses[0].rights(), Rights::Edit);
}

#[test]
fn a_team_calendar_is_owned_by_its_group() {
    let body = br#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
      <D:response><D:href>/caldav/workgroup/7/Rota/</D:href><D:propstat><D:prop>
        <D:resourcetype><D:collection/><C:calendar/></D:resourcetype>
        <D:owner><D:href>/caldav/workgroup/7/</D:href></D:owner>
      </D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
    </D:multistatus>"#;
    let responses = multistatus(body).unwrap();
    let owner = calendar_owner(&responses[0]).expect("the group owns it");
    assert_eq!((owner.address.as_str(), owner.name.as_str()), ("workgroup-7", "Workgroup"));
}

#[test]
fn an_absolute_address_must_be_on_the_service_and_never_keeps_a_fragment() {
    let base = "https://mail.example.org/caldav/";
    assert_eq!(
        resolve(base, "https://mail.example.org/caldav/ada/calendar/x.ics#day").as_deref(),
        Some("https://mail.example.org/caldav/ada/calendar/x.ics")
    );
    assert_eq!(resolve(base, "https://elsewhere.example/x.ics"), None);
    assert_eq!(resolve(base, "http://mail.example.org/caldav/x.ics"), None);
    assert_eq!(request_url(base, "/caldav/ada/x.ics#day").as_deref(), Some("https://mail.example.org/caldav/ada/x.ics"));
    assert_eq!(request_url(base, "https://elsewhere.example/x.ics"), None);
}

#[test]
fn addresses_resolve_within_the_service() {
    let base = "https://mail.example.org/caldav/";
    assert_eq!(resolve(base, "/caldav/ada/").as_deref(), Some("https://mail.example.org/caldav/ada/"));
    assert_eq!(resolve(base, "ada/").as_deref(), Some("https://mail.example.org/caldav/ada/"));
    assert_eq!(resolve(base, "https://mail.example.org/caldav/ada/").as_deref(), Some("https://mail.example.org/caldav/ada/"));
}

#[test]
fn a_change_made_here_goes_up_as_it_is_the_servers() {
    use mailcraft_model::remote::{EventOp, EventTime, EventsUpload};
    let server = FakeHttp::answering(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n".to_vec());
    let upload = EventsUpload {
        account: 7,
        email: "ada@example.org".into(),
        ops: vec![EventOp {
            kind: EventChangeKind::Create,
            event: 42,
            token: "made-here".into(),
            fields: EventFields {
                title: Some("Stand-up".into()),
                time: Some(EventTime { start: 1_800_000_000, end: 1_800_003_600, all_day: false }),
                ..Default::default()
            },
            ..Default::default()
        }],
        zone: "UTC".into(),
    };
    let calendar = server.url("/caldav/ada@example.org/calendar/");
    let uploaded = upload_events("", "ada@example.org", "invented", &upload, &calendar);
    assert_eq!(uploaded.outcomes.len(), 1);
    assert_eq!(uploaded.outcomes[0].result, EventResult::Done);
    assert_eq!(uploaded.outcomes[0].uid, "made-here");
    let request = server.request(0);
    assert!(request.starts_with("PUT /caldav/ada@example.org/calendar/made-here.ics HTTP/1.1\r\n"), "{request}");
    assert!(request.contains("Authorization: Basic "), "{request}");
    assert!(request.contains("Content-Type: text/calendar; charset=utf-8\r\n"), "{request}");
    assert!(request.contains("If-None-Match: *\r\n"), "{request}");
    assert!(request.contains("SUMMARY:Stand-up") && request.contains("UID:made-here\r\n"), "{request}");
}

#[test]
fn hostile_xml_gives_nothing_and_never_panics() {
    for body in [b"".as_slice(), b"<a>", b"<a><b></a>", b"<!DOCTYPE x [<!ENTITY e 'x'>]><a>&e;</a>", b"<a>&nope;</a>"] {
        let _ = multistatus(body);
        let _ = parse_xml(body);
    }
}

#[test]
fn a_change_to_an_event_named_only_by_its_path_reaches_the_server() {
    use mailcraft_model::remote::{EventChangeKind, EventFields, EventOp, EventsUpload};
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\nBEGIN:VEVENT\r\nUID:e-1\r\nDTSTAMP:20260101T000000Z\r\nDTSTART:20261012T130000Z\r\nDTEND:20261012T140000Z\r\nSUMMARY:Before\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let server = FakeHttp::start(move |n, _| {
        if n == 0 {
            format!("HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\n\r\n{}", ics.len(), ics).into_bytes()
        } else {
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec()
        }
    });
    let upload = EventsUpload {
        account: 7,
        email: "ada@example.org".into(),
        zone: "UTC".into(),
        ops: vec![EventOp {
            kind: EventChangeKind::Update,
            event: 42,
            remote: "/caldav/ada%40example.org/Calendar/e-1.ics".into(),
            fields: EventFields { title: Some("After".into()), ..Default::default() },
            ..Default::default()
        }],
    };
    let uploaded = upload_events(&server.url("/caldav/"), "ada@example.org", "invented", &upload, "");
    assert_eq!(uploaded.outcomes[0].result, EventResult::Done);
    assert_eq!(uploaded.outcomes[0].remote, "/caldav/ada%40example.org/Calendar/e-1.ics");
    assert!(server.request(0).starts_with("GET /caldav/ada%40example.org/Calendar/e-1.ics HTTP/1.1\r\n"), "{}", server.request(0));
    assert!(server.request(1).starts_with("PUT /caldav/ada%40example.org/Calendar/e-1.ics HTTP/1.1\r\n"), "{}", server.request(1));
    assert!(server.request(1).contains("SUMMARY:After\r\n"), "{}", server.request(1));
}

#[test]
fn a_delete_of_an_event_named_by_its_path_is_conditional_on_the_version_read() {
    use mailcraft_model::remote::{EventChangeKind, EventOp, EventsUpload};
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e-1\r\nDTSTART:20261012T130000Z\r\nDTEND:20261012T140000Z\r\nSUMMARY:Before\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let server = FakeHttp::start(move |n, _| {
        if n == 0 {
            format!("HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\n\r\n{}", ics.len(), ics).into_bytes()
        } else {
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec()
        }
    });
    let upload = EventsUpload {
        account: 7,
        email: "ada@example.org".into(),
        ops: vec![EventOp {
            kind: EventChangeKind::Delete,
            event: 42,
            remote: "/caldav/ada%40example.org/Calendar/e-1.ics".into(),
            ..Default::default()
        }],
        zone: "UTC".into(),
    };
    let uploaded = upload_events(&server.url("/caldav/"), "ada@example.org", "invented", &upload, "");
    assert_eq!(uploaded.outcomes[0].result, EventResult::Done);
    assert!(server.request(1).starts_with("DELETE /caldav/ada%40example.org/Calendar/e-1.ics HTTP/1.1\r\n"), "{}", server.request(1));
    assert!(server.request(1).contains("If-Match: \"v1\"\r\n"), "{}", server.request(1));
}

#[test]
fn a_card_says_which_book_it_came_from() {
    let card = mailcraft_vcard::parse("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:one\r\nFN:Ada Lovelace\r\nEMAIL:ada@example.org\r\nEND:VCARD\r\n")
        .into_iter()
        .next()
        .unwrap();
    let remote = remote_contact(card, "/carddav/ada/contacts/~book-7/one.vcf".into(), "\"1\"".into(), "Suppliers");
    assert_eq!((remote.book.as_str(), remote.uid.as_str(), remote.display_name.as_str()), ("Suppliers", "one", "Ada Lovelace"));
}

#[test]
fn an_edit_keeps_unknown_ics_bytes_and_the_events_zone() {
    use mailcraft_model::remote::{EventChangeKind, EventFields, EventOp, EventsUpload};
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//test//EN\r\nBEGIN:VEVENT\r\nUID:e-1\r\nDTSTAMP:20260101T000000Z\r\nDTSTART;TZID=Europe/Stockholm:20261024T090000\r\nDTEND;TZID=Europe/Stockholm:20261024T100000\r\nRRULE:FREQ=WEEKLY;COUNT=3\r\nSUMMARY:Before\r\nX-PRIVATE:keep\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Reminder\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let server = FakeHttp::start(move |n, _| {
        if n == 0 {
            format!("HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Length: {}\r\n\r\n{}", ics.len(), ics).into_bytes()
        } else {
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec()
        }
    });
    let upload = EventsUpload {
        account: 7,
        email: "ada@example.org".into(),
        zone: "UTC".into(),
        ops: vec![EventOp {
            kind: EventChangeKind::Update,
            event: 42,
            remote: "/caldav/ada%40example.org/Calendar/e-1.ics".into(),
            fields: EventFields { title: Some("After".into()), ..Default::default() },
            ..Default::default()
        }],
    };
    let uploaded = upload_events(&server.url("/caldav/"), "ada@example.org", "invented", &upload, "");
    assert_eq!(uploaded.outcomes[0].result, EventResult::Done);
    let put = server.request(1);
    assert!(put.contains("SUMMARY:After\r\n"), "{put}");
    assert!(put.contains("X-PRIVATE:keep\r\n") && put.contains("BEGIN:VALARM") && put.contains("TZID=Europe/Stockholm"), "{put}");
    assert!(put.contains("If-Match: \"v1\"\r\n"), "{put}");
}

#[test]
fn an_event_changed_on_the_server_is_not_overwritten() {
    use mailcraft_model::remote::{EventChangeKind, EventFields, EventOp, EventsUpload};
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e-1\r\nDTSTART:20261012T130000Z\r\nDTEND:20261012T140000Z\r\nSUMMARY:Before\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let server = FakeHttp::start(move |n, _| {
        if n == 0 {
            format!("HTTP/1.1 200 OK\r\nETag: \"v2\"\r\nContent-Length: {}\r\n\r\n{}", ics.len(), ics).into_bytes()
        } else {
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec()
        }
    });
    let upload = EventsUpload {
        account: 7,
        email: "ada@example.org".into(),
        zone: "UTC".into(),
        ops: vec![EventOp {
            kind: EventChangeKind::Update,
            event: 42,
            remote: "/caldav/ada%40example.org/Calendar/e-1.ics".into(),
            etag: "\"v1\"".into(),
            fields: EventFields { title: Some("After".into()), ..Default::default() },
            ..Default::default()
        }],
    };
    let uploaded = upload_events(&server.url("/caldav/"), "ada@example.org", "invented", &upload, "");
    assert_eq!(uploaded.outcomes[0].result, EventResult::Refused);
    assert_eq!(server.asked(), 1, "only the read that found the newer version");
}

#[test]
fn a_meeting_is_not_written_through_caldav() {
    use mailcraft_model::remote::{EventChangeKind, EventFields, EventOp, EventsUpload};
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:e-1\r\nDTSTART:20261012T130000Z\r\nDTEND:20261012T140000Z\r\nSUMMARY:Meeting\r\nORGANIZER:mailto:ada@example.org\r\nATTENDEE:mailto:bo@example.org\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let server = FakeHttp::start(move |n, _| {
        if n == 0 {
            format!("HTTP/1.1 200 OK\r\nETag: \"v1\"\r\nContent-Length: {}\r\n\r\n{}", ics.len(), ics).into_bytes()
        } else {
            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec()
        }
    });
    let upload = EventsUpload {
        account: 7,
        email: "ada@example.org".into(),
        zone: "UTC".into(),
        ops: vec![EventOp {
            kind: EventChangeKind::Update,
            event: 42,
            remote: "/caldav/ada%40example.org/Calendar/e-1.ics".into(),
            fields: EventFields { title: Some("Changed".into()), ..Default::default() },
            ..Default::default()
        }],
    };
    let uploaded = upload_events(&server.url("/caldav/"), "ada@example.org", "invented", &upload, "");
    assert_eq!(uploaded.outcomes[0].result, EventResult::Refused);
    assert_eq!(server.asked(), 1, "a meeting is refused before any PUT");
}

#[test]
fn an_occurrence_is_not_deleted_or_rewritten_as_its_series() {
    use mailcraft_model::remote::{EventChangeKind, EventOp, EventsUpload};
    let server = FakeHttp::answering(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec());
    let upload = EventsUpload {
        account: 7,
        email: "ada@example.org".into(),
        zone: "UTC".into(),
        ops: vec![EventOp {
            kind: EventChangeKind::Delete,
            event: 42,
            remote: "/caldav/ada%40example.org/Calendar/e-1.ics#e-1#20261026T130000Z".into(),
            ..Default::default()
        }],
    };
    let uploaded = upload_events(&server.url("/caldav/"), "ada@example.org", "invented", &upload, "");
    assert_eq!(uploaded.outcomes[0].result, EventResult::Refused);
    assert_eq!(server.asked(), 0);
}

#[test]
fn apply_fields_keeps_a_recurring_events_zone() {
    use mailcraft_model::Event;
    use mailcraft_model::remote::{EventFields, EventTime};
    let mut event = Event {
        start_zone: "Europe/Stockholm".into(),
        end_zone: "Europe/Stockholm".into(),
        recurrence: Some(mailcraft_model::Recurrence::default()),
        ..Default::default()
    };
    apply_fields(
        &mut event,
        &EventFields { time: Some(EventTime { start: 1_800_000_000, end: 1_800_003_600, all_day: false }), ..Default::default() },
    );
    assert_eq!((event.start_zone.as_str(), event.end_zone.as_str()), ("Europe/Stockholm", "Europe/Stockholm"));
}

#[test]
fn a_login_without_the_domain_names_the_main_calendar_too() {
    let owner = CalendarOwner { address: "ada".into(), name: String::new() };
    assert!(same_principal(&owner.address, "ada@example.org"));
    assert!(same_principal("ada@example.org", "ada@example.org"));
    assert!(!same_principal("bo@example.org", "ada@example.org"));
}

#[test]
fn a_redirect_is_followed_but_never_to_another_server() {
    let server = FakeHttp::start(|n, _| {
        if n == 0 {
            b"HTTP/1.1 301 Moved Permanently\r\nLocation: /caldav/next/\r\nContent-Length: 0\r\n\r\n".to_vec()
        } else {
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"example\"\r\nContent-Length: 0\r\n\r\n".to_vec()
        }
    });
    let (reply, final_url) = send_following(&server.url("/caldav/"), "PROPFIND", Some("0"), None, "", XML, &[], 16 * 1024).unwrap();
    assert_eq!((reply.status, final_url), (401, server.url("/caldav/next/")));
    let elsewhere =
        FakeHttp::answering(b"HTTP/1.1 301 Moved Permanently\r\nLocation: https://elsewhere.example/caldav/\r\nContent-Length: 0\r\n\r\n".to_vec());
    assert!(
        send_following(&elsewhere.url("/caldav/"), "PROPFIND", Some("0"), None, "", XML, &[], 16 * 1024)
            .unwrap_err()
            .to_string()
            .contains("another server")
    );
}
