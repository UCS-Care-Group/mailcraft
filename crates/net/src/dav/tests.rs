// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The CalDAV and CardDAV client: URLs, listings, events and cards, against a stand-in.

use super::*;
use crate::fake::FakeHttp;

#[test]
fn an_mx_host_is_asked_whether_it_is_linchange_without_a_password() {
    let cal = FakeHttp::answering(
        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"linchange CalDAV\", charset=\"UTF-8\"\r\nContent-Length: 0\r\n\r\n".to_vec(),
    );
    let card = FakeHttp::answering(
        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"linchange CardDAV\", charset=\"UTF-8\"\r\nContent-Length: 0\r\n\r\n".to_vec(),
    );
    // The test asks the two stand-ins instead of a real host: each is reached by the same
    // paths the service roots would be.
    assert!(names_linchange(&cal.url("/caldav/"), "CalDAV").unwrap());
    assert!(names_linchange(&card.url("/carddav/"), "CardDAV").unwrap());
    let request = cal.request(0);
    assert!(request.starts_with("PROPFIND /caldav/ HTTP/1.1\r\n"));
    assert!(!request.to_ascii_lowercase().contains("authorization"), "{request}");
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
fn a_new_contact_goes_to_the_book_it_was_given() {
    let server = FakeHttp::answering(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n".to_vec());
    let book = server.url("/carddav/ada@example.org/contacts/personal/");
    let upload = ContactsUpload {
        account: 1,
        email: "ada@example.org".into(),
        ops: vec![ContactOp {
            kind: ContactChangeKind::Create,
            contact: 7,
            book: book.clone(),
            uid: "one-uid".into(),
            card: "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:one-uid\r\nFN:Ada Lovelace\r\nEND:VCARD\r\n".into(),
            ..Default::default()
        }],
    };
    let out = upload_contacts("", "ada@example.org", "invented", &upload);
    assert_eq!(out.outcomes[0].result, EventResult::Done);
    assert_eq!(out.outcomes[0].remote, format!("{book}one-uid.vcf"));
    let request = server.request(0);
    assert!(request.starts_with("PUT /carddav/ada@example.org/contacts/personal/one-uid.vcf HTTP/1.1\r\n"), "{request}");
    assert!(request.contains("Content-Type: text/vcard; charset=utf-8\r\n"), "{request}");
    assert!(request.contains("UID:one-uid\r\n") && request.contains("FN:Ada Lovelace\r\n"), "{request}");
}

#[test]
fn a_card_changed_or_deleted_here_reaches_the_server() {
    let server = FakeHttp::answering(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n".to_vec());
    let upload = ContactsUpload {
        account: 1,
        email: "ada@example.org".into(),
        ops: vec![
            ContactOp {
                kind: ContactChangeKind::Update,
                contact: 7,
                remote: "/carddav/ada@example.org/contacts/personal/one.vcf".into(),
                uid: "one".into(),
                card: "BEGIN:VCARD\r\nVERSION:3.0\r\nUID:one\r\nFN:Ada Lovelace\r\nEND:VCARD\r\n".into(),
                ..Default::default()
            },
            ContactOp {
                kind: ContactChangeKind::Delete,
                contact: 0,
                remote: "/carddav/ada@example.org/contacts/personal/two.vcf".into(),
                ..Default::default()
            },
        ],
    };
    let out = upload_contacts(&server.url("/carddav/"), "ada@example.org", "invented", &upload);
    assert_eq!(out.outcomes.iter().map(|o| o.result).collect::<Vec<_>>(), [EventResult::Done, EventResult::Done]);
    assert!(server.request(0).starts_with("PUT /carddav/ada@example.org/contacts/personal/one.vcf HTTP/1.1\r\n"));
    assert!(server.request(0).contains("FN:Ada Lovelace\r\n"));
    assert!(server.request(1).starts_with("DELETE /carddav/ada@example.org/contacts/personal/two.vcf HTTP/1.1\r\n"));
}

#[test]
fn a_refused_card_change_still_names_the_card() {
    let server = FakeHttp::answering(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec());
    let upload = ContactsUpload {
        account: 1,
        email: "ada@example.org".into(),
        ops: vec![ContactOp {
            kind: ContactChangeKind::Delete,
            remote: "/carddav/ada@example.org/contacts/~book-7/two.vcf".into(),
            ..Default::default()
        }],
    };
    let out = upload_contacts(&server.url("/carddav/"), "ada@example.org", "invented", &upload);
    assert_eq!(out.outcomes[0].result, EventResult::Denied);
    assert_eq!(out.outcomes[0].remote, "/carddav/ada@example.org/contacts/~book-7/two.vcf");
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
