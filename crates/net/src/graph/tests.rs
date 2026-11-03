// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Microsoft Graph against a stand-in on loopback. Nothing here leaves this machine.

use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use mailcraft_model::Body;
use mailcraft_model::remote::{EventReminder, EventRepeat, EventWall};
use mailcraft_time::Date;
use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::fake::{FakeHttp, json_answer};

const TOKEN: &str = "graph.acc-ess_tok.en-0123456789";
/// What Graph says to a message it takes.
const ACCEPTED: &[u8] = b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
const ASTRAY: &str = "Microsoft Graph named a next page that can't be followed";

fn at(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> Timestamp {
    DateTime::new(Date::new(year, month, day).unwrap(), hour, minute, 0).unwrap().to_timestamp(0)
}

/// The stretch of the calendar the tests ask for: October 2026.
fn october() -> (Timestamp, Timestamp) {
    (at(2026, 10, 1, 0, 0), at(2026, 11, 1, 0, 0))
}

fn message() -> Message {
    Message {
        message_id: "mc.1.1@example.org".into(),
        from: Address::new("Ada Lovelace", "ada@example.org"),
        to: vec![Address::new("Bo Marsh", "bo@example.org")],
        cc: vec![Address::new("", "cy@example.org")],
        subject: "Tea on Thursday?".into(),
        date: at(2026, 10, 12, 8, 0),
        body: Body { text: "Four o'clock, the usual place.\n".into(), html: None },
        ..Default::default()
    }
}

/// An answer with a JSON body, and more headers when there are any (each ending in CRLF).
fn answer(status: &str, headers: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        .into_bytes()
}

/// A refusal as Graph words it.
fn graph_error(code: &str, message: &str) -> String {
    json!({"error": {"code": code, "message": message, "innerError": {"date": "2026-10-12T08:00:00", "request-id": "1234"}}}).to_string()
}

/// One page of a list, naming the next one when there is one.
fn page(entries: Vec<Value>, next: Option<String>) -> Vec<u8> {
    let mut page = json!({"@odata.context": "https://graph.example.org/v1.0/$metadata#users('ada')/calendarView", "value": entries});
    if let Some(next) = next {
        page["@odata.nextLink"] = json!(next);
    }
    json_answer("200 OK", &page.to_string())
}

/// An event with no more than it needs.
fn event(id: &str, subject: &str) -> Value {
    json!({
        "id": id,
        "subject": subject,
        "start": {"dateTime": "2026-10-12T13:00:00.0000000", "timeZone": "UTC"},
        "end": {"dateTime": "2026-10-12T14:00:00.0000000", "timeZone": "UTC"},
    })
}

/// A stand-in whose answers may name its own address (for the next page of a list): `answer` is
/// given the number of the request, the request, and `http://127.0.0.1:{port}`.
fn serving(answer: impl Fn(usize, &str, &str) -> Vec<u8> + Send + 'static) -> FakeHttp {
    let port = Arc::new(AtomicU16::new(0));
    let known = port.clone();
    let server = FakeHttp::start(move |n, request| answer(n, request, &format!("http://127.0.0.1:{}", known.load(Ordering::Relaxed))));
    port.store(server.port, Ordering::Relaxed);
    server
}

/// What was asked for in a request, and the names and values of its query.
fn asked_for(request: &str) -> (String, Vec<(String, String)>) {
    let target = request.split(' ').nth(1).unwrap();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    (path.to_string(), http::pairs(query))
}

/// The message in a request to send: what its body is the base64 of.
fn sent_message(request: &str) -> Vec<u8> {
    STANDARD.decode(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

/// What sending comes to when Graph answers `answer`.
fn sent_to(answer: Vec<u8>) -> Result<(), NetError> {
    let server = FakeHttp::answering(answer);
    send_at(&server.url(""), TOKEN, &message(), &SendOptions::default())
}

fn rejected(text: &str) -> NetError {
    NetError::Proto(ProtoError::Rejected(text.into()))
}

/// Nothing of the token in what an error says, whole or in pieces.
fn assert_no_token(error: &NetError) {
    let shown = format!("{error} {error:?}");
    assert!((0..TOKEN.len() - 8).all(|from| !shown.contains(&TOKEN[from..from + 8])), "{shown}");
}

#[test]
fn a_message_goes_to_graph_in_base64() {
    let server = FakeHttp::answering(ACCEPTED.to_vec());
    send_at(&server.url(""), TOKEN, &message(), &SendOptions { offset_minutes: Some(120) }).unwrap();
    assert_eq!(server.asked(), 1);
    let request = server.request(0);
    assert!(request.starts_with("POST /v1.0/me/sendMail HTTP/1.1\r\n"), "{request}");
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    for header in [
        format!("Host: 127.0.0.1:{}", server.port),
        format!("Authorization: Bearer {TOKEN}"),
        "Accept: application/json".into(),
        "Content-Type: text/plain".into(),
        format!("Content-Length: {}", body.len()),
        "User-Agent: MailCraft".into(),
        // Graph's connections are kept open (#276); a send still has a new one of its own.
        "Connection: keep-alive".into(),
    ] {
        assert!(head.contains(&format!("\r\n{header}")), "{header}: {head}");
    }
    // The body is base64 and nothing else: of the message exactly as it is written for the wire.
    assert!(!body.is_empty() && body.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)), "{body}");
    let sent = sent_message(&request);
    assert_eq!(sent, mailcraft_mime::write(&message(), &WriteOptions { include_bcc: true, offset_minutes: 120 }));
    let text = String::from_utf8(sent.clone()).unwrap();
    assert!(text.starts_with("Date: Mon, 12 Oct 2026 10:00:00 +0200\r\n") && text.contains("\r\nSubject: Tea on Thursday?\r\n"), "{text}");
    // Nobody gets a blind copy: no such field.
    assert!(!text.to_lowercase().contains("\r\nbcc:"), "{text}");
    let read = mailcraft_mime::parse(&sent);
    assert_eq!(
        (read.subject.as_str(), read.from.email.as_str(), read.message_id.as_str()),
        ("Tea on Thursday?", "ada@example.org", "mc.1.1@example.org")
    );
    assert_eq!((read.to, read.cc, read.bcc), (message().to, message().cc, Vec::new()));
    assert_eq!(read.body.text.trim(), "Four o'clock, the usual place.");

    // A message that was never dated is dated as it goes.
    let before = mailcraft_time::now();
    send_at(&server.url(""), TOKEN, &Message { date: 0, ..message() }, &SendOptions::default()).unwrap();
    let dated = mailcraft_mime::parse(&sent_message(&server.request(1))).date;
    assert!((before..=mailcraft_time::now()).contains(&dated), "{dated}");
}

#[test]
fn blind_copies_are_named_to_graph_in_the_message() {
    let server = FakeHttp::answering(ACCEPTED.to_vec());
    let blind = vec![Address::new("Eve Hart", "eve@example.org"), Address::new("", "dee@example.org")];
    let message = Message { bcc: blind.clone(), ..message() };
    send_at(&server.url(""), TOKEN, &message, &SendOptions::default()).unwrap();
    // Graph has no envelope: the field is the only way it learns who else the message is for.
    let sent = sent_message(&server.request(0));
    let text = String::from_utf8(sent.clone()).unwrap();
    assert!(text.contains("\r\nBcc: ") && text.contains("eve@example.org") && text.contains("dee@example.org"), "{text}");
    let read = mailcraft_mime::parse(&sent);
    assert_eq!((read.to, read.cc, read.bcc), (message.to.clone(), message.cc.clone(), blind.clone()));
    // Once, to everybody: not a second time for those it names.
    assert_eq!(server.asked(), 1);

    // A message for blind copies alone can be sent.
    let only = Message { to: Vec::new(), cc: Vec::new(), ..message };
    send_at(&server.url(""), TOKEN, &only, &SendOptions::default()).unwrap();
    let read = mailcraft_mime::parse(&sent_message(&server.request(1)));
    assert_eq!((read.to.len(), read.cc.len(), read.bcc), (0, 0, blind));
}

#[test]
fn what_graph_says_to_a_message_it_does_not_take() {
    // The token isn't taken: a refused sign-in, for the host to get a new token over.
    let expired =
        sent_to(answer("401 Unauthorized", "", &graph_error("InvalidAuthenticationToken", "Lifetime validation failed, the token is expired.")))
            .unwrap_err();
    assert_eq!(expired, NetError::Proto(ProtoError::Auth("Microsoft Graph didn't take the sign-in (InvalidAuthenticationToken)".into())));
    assert!(expired.is_refused_sign_in() && !expired.is_consent_needed());

    // The token is good and the account may not send this way.
    let denied =
        sent_to(answer("403 Forbidden", "", &graph_error("ErrorAccessDenied", "Access is denied. Check credentials and try again."))).unwrap_err();
    assert_eq!(denied, NetError::Forbidden("this account may not send through Microsoft Graph (ErrorAccessDenied)".into()));
    assert_eq!(denied.to_string(), "this account may not send through Microsoft Graph (ErrorAccessDenied)");
    assert!(!denied.is_refused_sign_in() && !denied.is_consent_needed());
    assert_eq!(
        sent_to(answer("403 Forbidden", "", "")).unwrap_err(),
        NetError::Forbidden("this account may not send through Microsoft Graph".into())
    );

    // Not now: said with how long to stay away when Graph says, and without waiting here.
    let started = Instant::now();
    for (status, headers, wait, said) in [
        ("429 Too Many Requests", "Retry-After: 30\r\n", Some(30), "Microsoft Graph is busy; try again in 30 seconds"),
        ("429 Too Many Requests", "retry-after:  1 \r\n", Some(1), "Microsoft Graph is busy; try again in a second"),
        ("503 Service Unavailable", "Retry-After: 0\r\n", Some(0), "Microsoft Graph is busy; try again in 0 seconds"),
        ("503 Service Unavailable", "", None, "Microsoft Graph is busy; try again later"),
        ("503 Service Unavailable", "Retry-After: 99999999999999\r\n", Some(86_400), "Microsoft Graph is busy; try again in 86400 seconds"),
        ("504 Gateway Timeout", "Retry-After: soon\r\n", None, "Microsoft Graph is busy; try again later"),
        ("429 Too Many Requests", "Retry-After: -5\r\n", None, "Microsoft Graph is busy; try again later"),
    ] {
        let busy = sent_to(answer(status, headers, &graph_error("TooManyRequests", "Please retry again later."))).unwrap_err();
        assert_eq!(busy, NetError::Busy { text: said.into(), retry_after: wait }, "{status} {headers}");
        assert!(busy.to_string() == said && !busy.is_refused_sign_in() && !busy.is_consent_needed());
    }
    assert!(started.elapsed() < Duration::from_secs(20), "{:?}", started.elapsed());

    // Anything else it says is its no to this message.
    for (status, body, said) in [
        (
            "400 Bad Request",
            graph_error("ErrorInvalidRecipients", "At least one recipient isn't valid."),
            "Microsoft Graph answered with status 400 (ErrorInvalidRecipients)",
        ),
        (
            "404 Not Found",
            graph_error("MailboxNotEnabledForRESTAPI", "The mailbox is either inactive, soft-deleted, or is hosted on-premise."),
            "Microsoft Graph answered with status 404 (MailboxNotEnabledForRESTAPI)",
        ),
        ("413 Request Entity Too Large", String::new(), "the message is too large to send through Microsoft Graph"),
        ("500 Internal Server Error", "<html>oops</html>".into(), "Microsoft Graph answered with status 500"),
        // Only "accepted" is taken for sent.
        ("200 OK", "{}".into(), "Microsoft Graph answered with status 200"),
        ("201 Created", "{}".into(), "Microsoft Graph answered with status 201"),
        ("204 No Content", String::new(), "Microsoft Graph answered with status 204"),
    ] {
        assert_eq!(sent_to(answer(status, "", &body)).unwrap_err(), rejected(said), "{status}");
    }
    // Sent somewhere else, it doesn't go there.
    let elsewhere = FakeHttp::answering(ACCEPTED.to_vec());
    let server = FakeHttp::answering(answer("307 Temporary Redirect", &format!("Location: {}\r\n", elsewhere.url("/v1.0/me/sendMail")), ""));
    let moved = send_at(&server.url(""), TOKEN, &message(), &SendOptions::default());
    assert_eq!(moved, Err(rejected("Microsoft Graph answered with status 307")));
    assert_eq!((server.asked(), elsewhere.asked()), (1, 0));

    // What isn't Graph at all, or isn't there.
    assert!(matches!(sent_to(b"220 smtp.example.org ESMTP\r\n".to_vec()), Err(NetError::Proto(ProtoError::Protocol(_)))));
    assert!(matches!(sent_to(Vec::new()), Err(NetError::Proto(ProtoError::Io(_)))));
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let unreachable = send_at(&format!("http://127.0.0.1:{port}"), TOKEN, &message(), &SendOptions::default()).unwrap_err();
    assert!(matches!(unreachable, NetError::Connect { .. }) && !unreachable.is_refused_sign_in(), "{unreachable:?}");
}

#[test]
fn nothing_that_was_sent_comes_back_in_an_error() {
    // A careless server: it writes back the token it was shown and the message it was sent, as
    // the name of its refusal and in what it says about it.
    let careless = |n: usize, request: &str| {
        let (head, body) = request.split_once("\r\n\r\n").unwrap();
        let token = head.lines().find_map(|line| line.strip_prefix("Authorization: Bearer ")).unwrap();
        // `RGF0ZTog` is how every message starts: the base64 of `Date: `.
        let code = match n {
            0 => token.to_string(),
            1 => body.get(..8).unwrap().to_string(),
            2 => body.get(..40).unwrap().to_string(),
            3 => "has spaces".to_string(),
            4 => "x".repeat(MAX_CODE + 1),
            5 => "Bcc:eve".to_string(),
            _ => "ErrorAccessDenied".to_string(),
        };
        answer("403 Forbidden", "", &json!({"error": {"code": code, "message": format!("{token} may not send {body}")}}).to_string())
    };
    let server = FakeHttp::start(careless);
    let secret = Message { bcc: vec![Address::new("", "eve@example.org")], subject: "The secret plan".into(), ..message() };
    for n in 0..7 {
        let error = send_at(&server.url(""), TOKEN, &secret, &SendOptions::default()).unwrap_err();
        let named = if n == 6 { " (ErrorAccessDenied)" } else { "" };
        assert_eq!(error, NetError::Forbidden(format!("this account may not send through Microsoft Graph{named}")), "{n}");
        assert_no_token(&error);
    }
    assert!(server.request(1).split_once("\r\n\r\n").unwrap().1.starts_with("RGF0ZTog"));
    // The same of a list, where only the token went out.
    let server = FakeHttp::start(careless);
    let (from, to) = october();
    let error = events_at(&server.url(""), TOKEN, from, to).unwrap_err();
    assert_eq!(error, NetError::Forbidden("this account may not read its calendar through Microsoft Graph".into()));
    assert_no_token(&error);
}

#[test]
fn what_cannot_be_sent_is_refused_before_graph_is_asked() {
    let server = FakeHttp::answering(ACCEPTED.to_vec());
    let (base, options) = (server.url(""), SendOptions::default());
    let config = |result: Result<(), NetError>| match result {
        Err(NetError::Config(why)) => why,
        other => panic!("expected the message to be refused, got {other:?}"),
    };
    // Nobody to send it to.
    let nobody = Message { to: Vec::new(), cc: Vec::new(), ..message() };
    assert_eq!(config(send_at(&base, TOKEN, &nobody, &options)), "the message has no recipients");
    let blank = Message { to: vec![Address::new("Bo", ""), Address::new("", "  ")], cc: Vec::new(), ..message() };
    assert_eq!(config(send_at(&base, TOKEN, &blank, &options)), "the message has no recipients");
    // Somebody it can't be sent to, wherever they stand: nobody is left out silently.
    for bad in [
        "bo",
        "bo@",
        "@example.org",
        "bo marsh@example.org",
        "bo@example.org>\r\nBcc: eve@example.org",
        "bø@example.org",
        "a@b@c",
        "bo@example.org,eve@example.org",
    ] {
        let stranger = vec![Address { name: String::new(), email: bad.to_string() }];
        for message in [
            Message { to: stranger.clone(), ..message() },
            Message { cc: stranger.clone(), ..message() },
            Message { bcc: stranger.clone(), ..message() },
        ] {
            let why = config(send_at(&base, TOKEN, &message, &options));
            assert!(why.contains("isn't an address mail can be sent to") && !why.contains('\r') && !why.contains('\n'), "{bad:?}: {why}");
        }
    }
    // No token, or what couldn't be one: there is no sign-in to show, and nothing to show it to.
    let (from, to) = october();
    for token in ["", " ", "two words", "tok\r\nX-Evil: 1", "tøken", &"t".repeat(MAX_TOKEN + 1)] {
        let refused = [
            send_at(&base, token, &message(), &options).unwrap_err(),
            events_at(&base, token, from, to).unwrap_err(),
            contacts_at(&base, token).unwrap_err(),
        ];
        for error in refused {
            assert_eq!(error, NetError::Proto(ProtoError::Auth("there is no sign-in to show Microsoft Graph".into())), "{token:?}");
            assert!(error.is_refused_sign_in());
        }
    }
    // A stretch of time with no length.
    for (from, to) in [(to, from), (from, from), (i64::MAX, i64::MIN)] {
        assert!(matches!(events_at(&base, TOKEN, from, to), Err(NetError::Config(_))));
    }
    // Graph is spoken to encrypted, or on this machine.
    for elsewhere in ["http://graph.example.org", "ftp://127.0.0.1", "graph.example.org", ""] {
        assert!(matches!(send_at(elsewhere, TOKEN, &message(), &options), Err(NetError::Config(_))), "{elsewhere}");
        assert!(matches!(contacts_at(elsewhere, TOKEN), Err(NetError::Config(_))), "{elsewhere}");
    }
    assert_eq!(server.asked(), 0);
}

#[test]
fn only_graph_itself_is_ever_followed_to() {
    let graph = Graph::at(GRAPH, TOKEN).unwrap();
    assert_eq!((graph.base.host.as_str(), graph.base.secure, graph.base.port), ("graph.microsoft.com", true, 0));
    assert_eq!(graph.url(SEND_MAIL), Url { host: "graph.microsoft.com".into(), port: 0, secure: true, path: "/v1.0/me/sendMail".into() });
    let current = graph.url("/v1.0/me/contacts?$top=50");
    let next = |link: &Value| graph.next_page(link, &current);

    let followed = next(&json!("https://graph.microsoft.com/v1.0/me/contacts?%24top=50&%24skip=50")).unwrap();
    assert_eq!(
        (followed.host.as_str(), followed.secure, followed.path.as_str()),
        ("graph.microsoft.com", true, "/v1.0/me/contacts?%24top=50&%24skip=50")
    );
    assert!(next(&json!("https://GRAPH.Microsoft.com:443/v1.0/users('ada')/contacts?$skiptoken=abc")).is_ok());
    assert!(next(&json!(format!("https://graph.microsoft.com/v1.0/me/contacts?$skiptoken={}", "a".repeat(5_000)))).is_ok());

    for astray in [
        json!("https://graph.microsoft.com.evil.example/v1.0/me/contacts?$skip=50"),
        json!("https://evil.example/v1.0/me/contacts?$skip=50"),
        json!("https://graph.microsoft.com@evil.example/v1.0/me/contacts?$skip=50"),
        json!("https://evil.example/https://graph.microsoft.com/v1.0/me/contacts"),
        json!("https://evil.example/?https://graph.microsoft.com/v1.0/me/contacts"),
        json!("https://graph.microsoft.com:8443/v1.0/me/contacts?$skip=50"),
        json!("https://graph.microsoft.com./v1.0/me/contacts?$skip=50"),
        json!("https://xgraph.microsoft.com/v1.0/me/contacts?$skip=50"),
        json!("https://microsoft.com/v1.0/me/contacts?$skip=50"),
        // Not encrypted, even to the right name.
        json!("http://graph.microsoft.com/v1.0/me/contacts?$skip=50"),
        json!("http://127.0.0.1/v1.0/me/contacts?$skip=50"),
        json!("http://localhost:443/v1.0/me/contacts?$skip=50"),
        // Not a whole address.
        json!("//evil.example/v1.0/me/contacts"),
        json!("/v1.0/me/contacts?$skip=50"),
        json!("graph.microsoft.com/v1.0/me/contacts?$skip=50"),
        json!("javascript:alert(1)"),
        json!(""),
        // The page it stands on: a list that goes in a circle.
        json!("https://graph.microsoft.com/v1.0/me/contacts?$top=50"),
        // Nothing that could say more than an address.
        json!("https://graph.microsoft.com/v1.0/me/contacts?$skip=50\r\nHost: evil.example"),
        json!("https://graph.microsoft.com/v1.0/me/contacts?$skip=50 HTTP/1.1"),
        json!("https://graph.microsoft.com/v1.0/me/contacts?x=é"),
        json!(format!("https://graph.microsoft.com/v1.0/me/contacts?$skiptoken={}", "a".repeat(http::MAX_LONG_URL))),
        // Not text at all.
        json!(42),
        json!(true),
        json!(["https://graph.microsoft.com/v1.0/me/contacts?$skip=50"]),
        json!({"href": "https://graph.microsoft.com/v1.0/me/contacts?$skip=50"}),
    ] {
        assert_eq!(next(&astray), Err(nonsense(ASTRAY)), "{astray}");
    }
}

/// A meeting with everything about it that is read.
fn meeting() -> Value {
    json!({
        "@odata.etag": "W/\"ZlnW4RIAV06KYYwlrfNZvQAALfZeRQ==\"",
        "id": "AAMkAGI1-meeting_1=",
        "iCalUId": "040000008200E00074C5B7101A82E00800000000",
        "changeKey": "ZlnW4RIAV06KYYwlrfNZvQAALfZeRQ==",
        "subject": "Budget review",
        "bodyPreview": "Agenda: 1. Numbers",
        "body": {"contentType": "text", "content": "Agenda:\r\n1. Numbers\r\n2. Questions\r\n"},
        "start": {"dateTime": "2026-10-12T13:00:00.0000000", "timeZone": "UTC"},
        "end": {"dateTime": "2026-10-12T14:30:00.0000000", "timeZone": "UTC"},
        "isAllDay": false,
        "isCancelled": false,
        "location": {"displayName": "Room 4", "locationType": "default", "uniqueId": "Room 4"},
        "organizer": {"emailAddress": {"name": "Bo Marsh", "address": "bo@example.org"}},
        "attendees": [
            {"type": "required", "status": {"response": "none", "time": "0001-01-01T00:00:00Z"}, "emailAddress": {"name": "Ada Lovelace", "address": "ada@example.org"}},
            {"type": "optional", "status": {"response": "tentativelyAccepted", "time": "2026-10-02T09:00:00Z"}, "emailAddress": {"name": "Cy", "address": "cy@example.org"}},
            {"type": "required", "status": {"response": "declined"}, "emailAddress": {"name": "Dee", "address": "dee@example.org"}},
            {"type": "resource", "status": {"response": "accepted"}, "emailAddress": {"name": "Room 4", "address": "room4@example.org"}},
            {"type": "required", "status": {"response": "notResponded"}, "emailAddress": {"name": "Bo Marsh", "address": "bo@example.org"}},
        ],
        "responseStatus": {"response": "accepted", "time": "2026-10-01T08:00:00Z"},
        "showAs": "tentative",
        "sensitivity": "private",
        "type": "occurrence",
        "seriesMasterId": "AAMkAGI1-series=",
        "lastModifiedDateTime": "2026-10-01T08:00:05.1234567Z",
        "isReminderOn": true,
        "reminderMinutesBeforeStart": 15,
        "webLink": "https://outlook.example.org/owa/?itemid=AAMk",
    })
}

#[test]
fn the_calendar_comes_page_by_page() {
    let holiday = json!({
        "id": "AAMkAGI1-holiday=",
        "subject": "Company holiday",
        "bodyPreview": "",
        "body": {"contentType": "html", "content": "<html><body><b>Closed</b></body></html>"},
        "start": {"dateTime": "2026-10-16T00:00:00.0000000", "timeZone": "UTC"},
        "end": {"dateTime": "2026-10-17T00:00:00.0000000", "timeZone": "UTC"},
        "isAllDay": true,
        "isCancelled": true,
        "showAs": "oof",
        "sensitivity": "normal",
        "type": "singleInstance",
        "isReminderOn": false,
        "reminderMinutesBeforeStart": 15,
    });
    let second = holiday.clone();
    let server = serving(move |n, _, own| match n {
        0 => page(vec![meeting()], Some(format!("{own}/v1.0/me/calendarView?startDateTime=x&%24skip=1"))),
        // The second page says the first event again.
        _ => page(vec![second.clone(), meeting()], None),
    });
    let (from, to) = october();
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    assert_eq!((download.from, download.to, download.complete, download.events.len()), (from, to, true, 2));
    // Whose it is, the one who asked says.
    assert_eq!((download.account, download.email.as_str(), download.calendar.as_str()), (0, "", ""));

    // What was asked: the stretch, a page's worth, and no more of an event than is kept (and
    // then the series the meeting is one of, which this stand-in has nothing to say about).
    assert_eq!(server.asked(), 3);
    assert!(server.request(2).starts_with("POST /v1.0/$batch HTTP/1.1\r\n"), "{}", server.request(2));
    let request = server.request(0);
    assert!(
        request.starts_with(
            "GET /v1.0/me/calendarView?startDateTime=2026-10-01T00%3A00%3A00Z&endDateTime=2026-11-01T00%3A00%3A00Z&%24top=50&%24select=id%2C"
        ),
        "{request}"
    );
    let (path, query) = asked_for(&request);
    let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
    assert_eq!(path, "/v1.0/me/calendarView");
    assert_eq!(
        query,
        [
            pair("startDateTime", "2026-10-01T00:00:00Z"),
            pair("endDateTime", "2026-11-01T00:00:00Z"),
            pair("$top", "50"),
            pair("$select", EVENT_FIELDS_OPTIONS)
        ]
    );
    for header in [
        format!("Authorization: Bearer {TOKEN}\r\n"),
        "Accept: application/json\r\n".into(),
        "Prefer: outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"\r\n".into(),
    ] {
        assert!(request.contains(&header) && server.request(1).contains(&header), "{header:?}: {request}");
    }
    assert!(request.ends_with("\r\n\r\n") && !request.to_lowercase().contains("content-length"), "{request}");
    // The next page is asked for by the address the first one named.
    assert!(server.request(1).starts_with("GET /v1.0/me/calendarView?startDateTime=x&%24skip=1 HTTP/1.1\r\n"), "{}", server.request(1));

    let person = |name: &str, email: &str, optional: bool, response: Response| Attendee { address: Address::new(name, email), optional, response };
    assert_eq!(
        download.events[0],
        RemoteEvent {
            id: "AAMkAGI1-meeting_1=".into(),
            change_key: "ZlnW4RIAV06KYYwlrfNZvQAALfZeRQ==".into(),
            uid: "040000008200E00074C5B7101A82E00800000000".into(),
            title: "Budget review".into(),
            location: "Room 4".into(),
            description: "Agenda:\n1. Numbers\n2. Questions".into(),
            start: at(2026, 10, 12, 13, 0),
            end: at(2026, 10, 12, 14, 30),
            all_day: false,
            organizer: Some(Address::new("Bo Marsh", "bo@example.org")),
            attendees: vec![
                person("Ada Lovelace", "ada@example.org", false, Response::None),
                person("Cy", "cy@example.org", true, Response::Tentative),
                person("Dee", "dee@example.org", false, Response::Declined),
                person("Room 4", "room4@example.org", false, Response::Accepted),
                person("Bo Marsh", "bo@example.org", false, Response::None),
            ],
            response: Response::Accepted,
            show_as: ShowAs::Tentative,
            private: true,
            cancelled: false,
            kind: RemoteEventKind::Occurrence,
            series: "AAMkAGI1-series=".into(),
            reminder_minutes: Some(15),
            modified: at(2026, 10, 1, 8, 0) + 5,
            // Whether the owner organizes it, and the mark it was made with, weren't said.
            is_organizer: None,
            token: String::new(),
            ..Default::default()
        }
    );
    // Whole days are the days as Graph names them; HTML is not taken for text; a reminder that is
    // off is none; and an event with no change key of its own has none.
    assert_eq!(
        download.events[1],
        RemoteEvent {
            id: "AAMkAGI1-holiday=".into(),
            title: "Company holiday".into(),
            start: at(2026, 10, 16, 0, 0),
            end: at(2026, 10, 17, 0, 0),
            all_day: true,
            show_as: ShowAs::OutOfOffice,
            cancelled: true,
            ..Default::default()
        }
    );
}

#[test]
fn events_are_read_as_they_are_said() {
    let read = |more: Value| {
        let mut entry = event("e-1", "x");
        for (name, value) in more.as_object().unwrap() {
            entry[name] = value.clone();
        }
        event_of(&entry)
    };
    let plain = read(json!({})).unwrap();
    assert_eq!(
        (plain.start, plain.end, plain.kind, plain.show_as, plain.response),
        (at(2026, 10, 12, 13, 0), at(2026, 10, 12, 14, 0), RemoteEventKind::Single, ShowAs::Busy, Response::None)
    );
    // Without a change key, the ETag is the mark of the version.
    assert_eq!(read(json!({"@odata.etag": "W/\"abc==\""})).unwrap().change_key, "W/\"abc==\"");
    assert_eq!(read(json!({"@odata.etag": "W/\"abc==\"", "changeKey": "abc=="})).unwrap().change_key, "abc==");
    // UTC by any of its names, or by an offset of the time's own; a zone that is known (it is not
    // what is asked for, but it says when it is all the same); no other zone is guessed at.
    for (start, zone, expected) in [
        ("2026-10-12T13:00:00.0000000", "UTC", Some(at(2026, 10, 12, 13, 0))),
        ("2026-10-12T13:00:00", "utc", Some(at(2026, 10, 12, 13, 0))),
        ("2026-10-12T13:00", "", Some(at(2026, 10, 12, 13, 0))),
        ("2026-10-12T13:00:00Z", "Etc/GMT", Some(at(2026, 10, 12, 13, 0))),
        ("2026-10-12T15:00:00+02:00", "tzone://Microsoft/Utc", Some(at(2026, 10, 12, 13, 0))),
        ("2026-10-12", "UTC", Some(at(2026, 10, 12, 0, 0))),
        ("2026-10-12T13:00:00", "Pacific Standard Time", Some(at(2026, 10, 12, 20, 0))),
        ("2026-10-12T13:00:00.0000000", "Europe/Berlin", Some(at(2026, 10, 12, 11, 0))),
        ("2026-12-12T13:00:00", "Europe/Berlin", Some(at(2026, 12, 12, 12, 0))),
        ("2026-10-12T13:00:00", "Customized Time Zone", None),
        ("2026-10-12T13:00:00", "tzone://Microsoft/Custom", None),
        ("tomorrow", "UTC", None),
        ("2026-13-40T13:00:00", "UTC", None),
        ("", "UTC", None),
        ("2026-10-12T13:00:00.00000000000000000000000000000000000000", "UTC", None),
    ] {
        let event = read(json!({"start": {"dateTime": start, "timeZone": zone}}));
        assert_eq!(event.map(|event| event.start), expected, "{start} {zone}");
    }
    // An end before its start is no length, not a negative one.
    let backwards = read(json!({"end": {"dateTime": "2026-10-12T12:00:00", "timeZone": "UTC"}})).unwrap();
    assert_eq!((backwards.start, backwards.end), (at(2026, 10, 12, 13, 0), at(2026, 10, 12, 13, 0)));
    // What kind of event, how it shows, how private it is.
    for (kind, expected) in [
        ("singleInstance", RemoteEventKind::Single),
        ("occurrence", RemoteEventKind::Occurrence),
        ("exception", RemoteEventKind::Exception),
        ("seriesMaster", RemoteEventKind::SeriesMaster),
        ("SERIESMASTER", RemoteEventKind::SeriesMaster),
        ("somethingNew", RemoteEventKind::Single),
    ] {
        assert_eq!(read(json!({"type": kind})).unwrap().kind, expected, "{kind}");
    }
    for (show_as, expected) in [
        ("free", ShowAs::Free),
        ("tentative", ShowAs::Tentative),
        ("busy", ShowAs::Busy),
        ("oof", ShowAs::OutOfOffice),
        ("workingElsewhere", ShowAs::WorkingElsewhere),
        ("unknown", ShowAs::Busy),
    ] {
        assert_eq!(read(json!({"showAs": show_as})).unwrap().show_as, expected, "{show_as}");
    }
    for (sensitivity, private) in [("normal", false), ("personal", false), ("private", true), ("confidential", true), ("Private", true), ("", false)]
    {
        assert_eq!(read(json!({"sensitivity": sensitivity})).unwrap().private, private, "{sensitivity}");
    }
    for (response, expected) in [
        ("none", Response::None),
        ("organizer", Response::Accepted),
        ("accepted", Response::Accepted),
        ("tentativelyAccepted", Response::Tentative),
        ("declined", Response::Declined),
        ("notResponded", Response::None),
        ("maybe", Response::None),
    ] {
        assert_eq!(read(json!({"responseStatus": {"response": response}})).unwrap().response, expected, "{response}");
    }
    // The description is the text Graph was asked for, or else the short text it makes itself.
    for (body, preview, expected) in [
        (json!({"contentType": "text", "content": "The whole text."}), "The whole", "The whole text."),
        (json!({"contentType": "Text", "content": "  \r\n"}), "The preview", "The preview"),
        (json!({"contentType": "html", "content": "<p>The whole text.</p>"}), "The preview", "The preview"),
        (json!({"content": "No kind is said."}), "The preview", "The preview"),
        (json!("The whole text."), "The preview", "The preview"),
        (json!(null), "", ""),
    ] {
        assert_eq!(read(json!({"body": body, "bodyPreview": preview})).unwrap().description, expected, "{body}");
    }
    // A reminder is one when it is on and says how long before.
    for (on, minutes, expected) in [
        (json!(true), json!(15), Some(15)),
        (json!(true), json!(0), Some(0)),
        (json!(false), json!(15), None),
        (json!(true), json!(-15), None),
        (json!(true), json!("15"), None),
        (json!("true"), json!(15), None),
        (json!(true), json!(18446744073709551615u64), Some(40_320)),
    ] {
        assert_eq!(read(json!({"isReminderOn": on, "reminderMinutesBeforeStart": minutes})).unwrap().reminder_minutes, expected, "{on} {minutes}");
    }
}

#[test]
fn a_list_without_end_is_cut_short() {
    let (from, to) = october();
    // Every page names another: no more than so many are asked for.
    let server = serving(|n, _, own| page(vec![event(&format!("e-{n}"), "x")], Some(format!("{own}/v1.0/me/calendarView?page={}", n + 1))));
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    assert_eq!((server.asked(), download.events.len(), download.complete), (MAX_PAGES, MAX_PAGES, false));

    // Pages with more on them than was asked for: no more events are taken than so many.
    let crowded = |last: usize| {
        serving(move |n, _, own| {
            let entries = (0..2_500).map(|i| event(&format!("e-{n}-{i}"), "x")).collect();
            page(entries, (n < last).then(|| format!("{own}/v1.0/me/calendarView?page={}", n + 1)))
        })
    };
    let server = crowded(usize::MAX);
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    assert_eq!((download.events.len(), download.complete, server.asked()), (MAX_LISTED_EVENTS, false, MAX_LISTED_EVENTS / 2_500 + 1));
    // A calendar that ends where the taking does is all there.
    let server = crowded(MAX_LISTED_EVENTS / 2_500 - 1);
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    assert_eq!((download.events.len(), download.complete, server.asked()), (MAX_LISTED_EVENTS, true, MAX_LISTED_EVENTS / 2_500));

    // Pages with nothing on them that still name a next one are followed to the end.
    let server = serving(|n, _, own| match n {
        0..=2 => page(Vec::new(), Some(format!("{own}/v1.0/me/calendarView?page={}", n + 1))),
        _ => page(vec![event("e-last", "x")], None),
    });
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    assert_eq!((download.events.len(), download.complete, server.asked()), (1, true, 4));
    // The end of a list may be said with nothing, or with null.
    let server = FakeHttp::answering(json_answer("200 OK", r#"{"value":[],"@odata.nextLink":null}"#));
    assert_eq!(contacts_at(&server.url(""), TOKEN).unwrap(), ContactsDownload { complete: true, ..Default::default() });
    assert_eq!(server.asked(), 1);
}

#[test]
fn a_next_page_somewhere_else_is_never_asked_for() {
    let (from, to) = october();
    // Another port of this machine is another server.
    let elsewhere = FakeHttp::answering(page(vec![event("stolen", "x")], None));
    let there = elsewhere.url("/v1.0/me/calendarView?page=2");
    let astray = Err(nonsense(ASTRAY));
    for next in
        [json!(there), json!(there.replace("127.0.0.1", "localhost")), json!("https://evil.example/v1.0/me/calendarView?page=2"), json!(""), json!(7)]
    {
        let link = next.clone();
        let server = FakeHttp::start(move |_, _| {
            let mut page: Value = json!({"value": [event("e-1", "x")]});
            page["@odata.nextLink"] = link.clone();
            json_answer("200 OK", &page.to_string())
        });
        assert_eq!(events_at(&server.url(""), TOKEN, from, to), astray, "{next}");
        assert_eq!(contacts_at(&server.url(""), TOKEN).map(|download| download.contacts.len()), Err(nonsense(ASTRAY)), "{next}");
        assert_eq!(server.asked(), 2, "{next}");
    }
    assert_eq!(elsewhere.asked(), 0);
    // A page that names itself as the next one.
    let server = serving(|_, request, own| page(vec![event("e-1", "x")], Some(format!("{own}{}", request.split(' ').nth(1).unwrap()))));
    assert_eq!(events_at(&server.url(""), TOKEN, from, to), astray);
    assert_eq!(server.asked(), 1);
}

#[test]
fn hostile_calendar_answers_give_events_or_nothing() {
    let (from, to) = october();
    let listed = |body: &str| {
        let server = FakeHttp::answering(json_answer("200 OK", body));
        events_at(&server.url(""), TOKEN, from, to)
    };
    // Not a page at all.
    let deep = format!(r#"{{"value":{}{}}}"#, "[".repeat(5_000), "]".repeat(5_000));
    for junk in [
        "",
        "not json",
        "[]",
        "null",
        "42",
        "\"value\"",
        "{}",
        r#"{"value":null}"#,
        r#"{"value":{"id":"x"}}"#,
        r#"{"value":"[]"}"#,
        "{\"value\":[",
        deep.as_str(),
    ] {
        let result = listed(junk);
        assert!(
            matches!(&result, Err(NetError::Proto(ProtoError::Protocol(why))) if why.starts_with("Microsoft Graph's answer")),
            "{junk:.40}: {result:?}"
        );
    }
    // More than is taken of one page: refused before it is read.
    let padded = format!(r#"{{"value":[],"padding":"{}"}}"#, "x".repeat(MAX_PAGE));
    assert_eq!(listed(&padded), Err(nonsense("the server's answer is longer than 4096 KB")));

    // What isn't an event is passed over; the download then doesn't claim to be all there is.
    let good = event("e-good", "The one that can be read");
    let timed = |id: Value| json!({"id": id, "start": {"dateTime": "2026-10-12T13:00:00", "timeZone": "UTC"}, "end": {"dateTime": "2026-10-12T14:00:00", "timeZone": "UTC"}});
    let mixed = json!({"value": [
        1, "x", null, [], {}, true,
        {"id": "no-times"},
        {"id": "no-end", "start": {"dateTime": "2026-10-12T13:00:00", "timeZone": "UTC"}},
        {"id": "another-zone", "start": {"dateTime": "2026-10-12T13:00:00", "timeZone": "Somewhere Standard Time"}, "end": {"dateTime": "2026-10-12T14:00:00", "timeZone": "UTC"}},
        {"id": "no-time", "start": {"dateTime": "tomorrow", "timeZone": "UTC"}, "end": {"dateTime": "later", "timeZone": "UTC"}},
        {"id": "times-not-said", "start": "2026-10-12T13:00:00", "end": "2026-10-12T14:00:00"},
        timed(json!(7)), timed(json!(null)), timed(json!("")), timed(json!("two words")), timed(json!("line\r\nbreak")), timed(json!("x".repeat(2_000))), timed(json!(["e-1"])),
        good.clone(),
    ]});
    let download = listed(&mixed.to_string()).unwrap();
    assert_eq!((download.events.len(), download.complete), (1, false));
    assert_eq!((download.events[0].id.as_str(), download.events[0].title.as_str()), ("e-good", "The one that can be read"));
    assert!(listed(&json!({"value": [good]}).to_string()).unwrap().complete);

    // An event with everything about it of the wrong kind is still an event, with nothing said.
    let odd = json!({"value": [{
        "id": "e-odd", "iCalUId": 5, "changeKey": ["x"], "@odata.etag": {"x": 1}, "subject": 5, "bodyPreview": ["x"], "body": "text",
        "start": {"dateTime": "2026-10-12T13:00:00"}, "end": {"dateTime": "2026-10-12T14:00:00Z", "timeZone": null},
        "isAllDay": "yes", "isCancelled": 1, "location": "Room 4", "organizer": {"emailAddress": "bo@example.org"}, "attendees": {"0": {}},
        "responseStatus": "accepted", "showAs": 7, "sensitivity": null, "type": [], "seriesMasterId": 9, "lastModifiedDateTime": 5,
        "isReminderOn": true, "reminderMinutesBeforeStart": "15",
    }]});
    let download = listed(&odd.to_string()).unwrap();
    assert!(download.complete);
    assert_eq!(
        download.events,
        [RemoteEvent { id: "e-odd".into(), start: at(2026, 10, 12, 13, 0), end: at(2026, 10, 12, 14, 0), ..Default::default() }]
    );

    // What is said is cut to size, and nothing in it can say more than it is.
    let long = json!({"value": [{
        "id": "e-long",
        "subject": format!("Budget\r\nreview{}", "x".repeat(5_000)),
        "location": {"displayName": "Room\u{0}4\u{202e}"},
        "body": {"contentType": "text", "content": "é".repeat(300_000)},
        "start": {"dateTime": "2026-10-12T13:00:00", "timeZone": "UTC"}, "end": {"dateTime": "2026-10-12T14:00:00", "timeZone": "UTC"},
        "organizer": {"emailAddress": {"name": "Bo\r\nBcc: eve@example.org", "address": "bo@example.org\r\nBcc: eve@example.org"}},
        "attendees": (0..2_000).map(|n| json!({"type": "required", "emailAddress": {"name": "", "address": format!("p{n}@example.org")}})).collect::<Vec<_>>(),
    }]});
    let download = listed(&long.to_string()).unwrap();
    let event = &download.events[0];
    assert_eq!((event.title.chars().count(), event.description.chars().count(), event.attendees.len()), (500, 100_000, MAX_PEOPLE));
    assert!(event.title.starts_with("Budget  review") && !event.title.contains('\r') && !event.location.contains('\u{0}'), "{:?}", event.location);
    // A name of one line is kept; an address that isn't one is not.
    assert_eq!(event.organizer, Some(Address { name: "Bo  Bcc: eve@example.org".into(), email: String::new() }));
}

#[test]
fn what_graph_says_when_it_will_not_list() {
    let (from, to) = october();
    let listed = |answer: Vec<u8>| {
        let server = FakeHttp::answering(answer);
        (events_at(&server.url(""), TOKEN, from, to).unwrap_err(), contacts_at(&server.url(""), TOKEN).unwrap_err())
    };
    let (calendar, people) = listed(answer("401 Unauthorized", "", &graph_error("InvalidAuthenticationToken", "Access token has expired.")));
    assert_eq!(calendar, NetError::Proto(ProtoError::Auth("Microsoft Graph didn't take the sign-in (InvalidAuthenticationToken)".into())));
    assert!(calendar == people && calendar.is_refused_sign_in());

    let (calendar, people) = listed(answer("403 Forbidden", "", &graph_error("ErrorAccessDenied", "Access is denied.")));
    assert_eq!(calendar, NetError::Forbidden("this account may not read its calendar through Microsoft Graph (ErrorAccessDenied)".into()));
    assert_eq!(people, NetError::Forbidden("this account may not read its contacts through Microsoft Graph (ErrorAccessDenied)".into()));

    let (calendar, people) =
        listed(answer("429 Too Many Requests", "Retry-After: 7\r\n", &graph_error("ApplicationThrottled", "Too many requests.")));
    assert_eq!(calendar, NetError::Busy { text: "Microsoft Graph is busy; try again in 7 seconds".into(), retry_after: Some(7) });
    assert_eq!(people, calendar);

    for (status, said) in [
        ("400 Bad Request", "Microsoft Graph answered with status 400 (ErrorInvalidParameter)"),
        ("404 Not Found", "Microsoft Graph answered with status 404 (ErrorInvalidParameter)"),
        // Too much to take is only ever said of a message.
        ("413 Request Entity Too Large", "Microsoft Graph answered with status 413 (ErrorInvalidParameter)"),
        ("500 Internal Server Error", "Microsoft Graph answered with status 500 (ErrorInvalidParameter)"),
        ("204 No Content", "Microsoft Graph answered with status 204"),
        ("301 Moved Permanently", "Microsoft Graph answered with status 301 (ErrorInvalidParameter)"),
    ] {
        let (calendar, people) = listed(answer(status, "", &graph_error("ErrorInvalidParameter", "No.")));
        assert_eq!((calendar, people), (rejected(said), rejected(said)), "{status}");
    }

    // A list that breaks off half-way is no list: nothing of it is handed on.
    let server = serving(|n, _, own| match n {
        0 => page(vec![event("e-1", "x")], Some(format!("{own}/v1.0/me/calendarView?page=2"))),
        _ => answer("503 Service Unavailable", "Retry-After: 120\r\n", ""),
    });
    let broken = events_at(&server.url(""), TOKEN, from, to);
    assert_eq!(broken, Err(NetError::Busy { text: "Microsoft Graph is busy; try again in 120 seconds".into(), retry_after: Some(120) }));
    assert_eq!(server.asked(), 2);
}

#[test]
fn contacts_come_page_by_page() {
    let ada = json!({
        "@odata.etag": "W/\"EQAAABYAAAC\"",
        "id": "AAMkAGI1-ada=",
        "changeKey": "EQAAABYAAAC",
        "displayName": "Ada Lovelace",
        "givenName": "Ada",
        "middleName": "King",
        "surname": "Lovelace",
        "nickName": "Countess",
        "title": "Dr.",
        "jobTitle": "Analyst",
        "companyName": "Analytical Engines Ltd",
        "department": "Research",
        "emailAddresses": [{"name": "Ada Lovelace", "address": "ada@example.org"}, {"name": "Ada at home", "address": " ada@home.example "}, {"name": "No address", "address": ""}],
        "businessPhones": ["+44 20 7946 0000", ""],
        "homePhones": ["+44 20 7946 0001"],
        "mobilePhone": "+44 7700 900000",
        "businessAddress": {"street": "1 Engine Way\r\nSuite 2", "city": "London", "state": "", "postalCode": "N1 9GU", "countryOrRegion": "United Kingdom"},
        "homeAddress": {},
        "otherAddress": {"city": "Lynton", "state": "Devon"},
        "businessHomePage": "https://engines.example",
        "birthday": "1815-12-10T11:59:00Z",
        "personalNotes": "Met at the\r\nRoyal Society.",
        "lastModifiedDateTime": "2026-09-30T10:00:00Z",
    });
    let server = serving(move |n, _, own| {
        let page = match n {
            0 => json!({"value": [ada.clone()], "@odata.nextLink": format!("{own}/v1.0/me/contacts?%24top=50&%24skip=1")}),
            _ => {
                json!({"value": [{"id": "AAMkAGI1-bo=", "displayName": "Bo", "birthday": null, "mobilePhone": null, "emailAddresses": [], "businessAddress": null}, ada.clone()]})
            }
        };
        json_answer("200 OK", &page.to_string())
    });
    let download = contacts_at(&server.url(""), TOKEN).unwrap();
    assert_eq!((download.complete, download.contacts.len(), download.account, download.email.as_str()), (true, 2, 0, ""));

    assert_eq!(server.asked(), 2);
    let request = server.request(0);
    let (path, query) = asked_for(&request);
    assert_eq!(path, "/v1.0/me/contacts");
    assert_eq!(query, [("$top".to_string(), "50".to_string()), ("$select".to_string(), CONTACT_FIELDS.to_string())]);
    assert!(
        request.starts_with("GET ") && request.contains(&format!("Authorization: Bearer {TOKEN}\r\n")) && !request.contains("Prefer:"),
        "{request}"
    );
    assert!(server.request(1).starts_with("GET /v1.0/me/contacts?%24top=50&%24skip=1 HTTP/1.1\r\n"), "{}", server.request(1));

    assert_eq!(
        download.contacts[0],
        RemoteContact {
            id: "AAMkAGI1-ada=".into(),
            change_key: "EQAAABYAAAC".into(),
            display_name: "Ada Lovelace".into(),
            first: "Ada".into(),
            middle: "King".into(),
            last: "Lovelace".into(),
            nickname: "Countess".into(),
            company: "Analytical Engines Ltd".into(),
            // What she does, not what she is called.
            title: "Analyst".into(),
            department: "Research".into(),
            emails: vec![Labeled::new("", "ada@example.org"), Labeled::new("", "ada@home.example")],
            phones: vec![
                Labeled::new("mobile", "+44 7700 900000"),
                Labeled::new("work", "+44 20 7946 0000"),
                Labeled::new("home", "+44 20 7946 0001")
            ],
            addresses: vec![
                PostalAddress {
                    label: "work".into(),
                    street: "1 Engine Way\nSuite 2".into(),
                    city: "London".into(),
                    region: String::new(),
                    postal_code: "N1 9GU".into(),
                    country: "United Kingdom".into(),
                },
                PostalAddress { label: "other".into(), city: "Lynton".into(), region: "Devon".into(), ..Default::default() },
            ],
            websites: vec!["https://engines.example".into()],
            birthday: Date::new(1815, 12, 10),
            notes: "Met at the\nRoyal Society.".into(),
            modified: at(2026, 9, 30, 10, 0),
        }
    );
    // Somebody of whom only the name is known.
    assert_eq!(download.contacts[1], RemoteContact { id: "AAMkAGI1-bo=".into(), display_name: "Bo".into(), ..Default::default() });
}

#[test]
fn hostile_contact_answers_give_contacts_or_nothing() {
    let listed = |page: Value| {
        let server = FakeHttp::answering(json_answer("200 OK", &page.to_string()));
        contacts_at(&server.url(""), TOKEN)
    };
    for junk in [json!([]), json!(null), json!({}), json!({"value": "x"}), json!({"value": {"id": "c-1"}})] {
        assert!(matches!(listed(junk.clone()), Err(NetError::Proto(ProtoError::Protocol(_)))), "{junk}");
    }
    // What isn't a contact is passed over, and then not everything is said to be there.
    let download = listed(
        json!({"value": [1, null, "x", [], {}, {"displayName": "No id"}, {"id": 7}, {"id": "two words"}, {"id": "c-good", "givenName": "Bo"}]}),
    )
    .unwrap();
    assert_eq!((download.contacts.len(), download.complete, download.contacts[0].first.as_str()), (1, false, "Bo"));

    // Everything of the wrong kind: still a contact, with nothing said of it.
    let odd = json!({"value": [{
        "id": "c-odd", "changeKey": 5, "displayName": ["Ada"], "givenName": 5, "surname": null, "jobTitle": {"x": 1}, "emailAddresses": "ada@example.org",
        "businessPhones": "+44", "homePhones": [5, null, ["x"]], "mobilePhone": 7, "businessAddress": "1 Engine Way", "homeAddress": [1], "otherAddress": {"city": 5},
        "businessHomePage": ["x"], "birthday": 18151210, "personalNotes": false, "lastModifiedDateTime": "yesterday",
    }]});
    let download = listed(odd).unwrap();
    assert!(download.complete);
    assert_eq!(download.contacts, [RemoteContact { id: "c-odd".into(), ..Default::default() }]);

    // What is said is cut to size: so many of each, an address that is one, a day that is one.
    let crowded = json!({"value": [{
        "id": "c-crowded",
        "displayName": format!("Ada\r\nLovelace{}", "x".repeat(5_000)),
        "emailAddresses": (0..500).map(|n| json!({"address": format!("ada{n}@example.org")})).chain([json!({"address": "not an address"}), json!(5)]).collect::<Vec<_>>(),
        "businessPhones": (0..500).map(|n| json!(format!("+44 {n}"))).collect::<Vec<_>>(),
        "birthday": "1815-13-45T00:00:00Z",
        "personalNotes": "é".repeat(300_000),
    }, {
        "id": "c-days",
        "emailAddresses": [{"address": "ada@example.org\r\nBcc: eve@example.org"}, {"address": "ada lovelace@example.org"}, {"name": "ada@example.org"}],
        "birthday": "1815-12-10",
    }, {
        "id": "c-late", "birthday": "1815-12-10T23:59:59.9999999-11:00",
    }]});
    let download = listed(crowded).unwrap();
    let contact = &download.contacts[0];
    assert_eq!((contact.display_name.chars().count(), contact.emails.len(), contact.phones.len()), (500, MAX_ENTRIES, MAX_ENTRIES));
    assert_eq!((contact.birthday, contact.notes.chars().count()), (None, 100_000));
    assert!(contact.display_name.starts_with("Ada  Lovelace") && contact.emails.iter().all(|email| email.value.ends_with("@example.org")));
    assert_eq!((download.contacts[1].emails.len(), download.contacts[1].birthday), (0, Date::new(1815, 12, 10)));
    // A birthday is the day that is written, wherever the clock next to it is set.
    assert_eq!(download.contacts[2].birthday, Date::new(1815, 12, 10));
}

#[test]
fn how_long_to_stay_away() {
    let now = at(2026, 10, 21, 7, 26);
    assert_eq!(wait_of(None, now), None);
    for (said, expected) in [
        ("30", Some(30)),
        (" 0 ", Some(0)),
        ("86400", Some(86_400)),
        ("86401", Some(86_400)),
        ("18446744073709551615", Some(86_400)),
        ("18446744073709551616", None),
        // The time to come back at.
        ("Wed, 21 Oct 2026 07:28:00 GMT", Some(120)),
        ("Wed, 21 Oct 2026 07:26:00 GMT", Some(0)),
        ("Wed, 21 Oct 2026 07:00:00 GMT", Some(0)),
        ("Fri, 31 Dec 9999 23:59:59 GMT", Some(86_400)),
        ("", None),
        ("soon", None),
        ("-5", None),
        ("1.5", None),
        ("30 seconds", None),
    ] {
        assert_eq!(wait_of(Some(said), now), expected, "{said:?}");
    }
    assert_eq!(wait_of(Some("Thu, 01 Jan 1970 00:00:10 GMT"), i64::MIN), Some(86_400));
    assert_eq!(wait_of(Some("Thu, 01 Jan 1970 00:00:10 GMT"), i64::MAX), Some(0));
}

#[test]
fn the_name_of_a_refusal_is_a_name_or_nothing() {
    let named = |body: &str| code_of(body.as_bytes(), &[TOKEN, "U3ViamVjdDogVGVhIG9uIFRodXJzZGF5Pw0K"]);
    assert_eq!(named(r#"{"error":{"code":"ErrorAccessDenied","message":"Access is denied."}}"#), " (ErrorAccessDenied)");
    assert_eq!(named(r#"{"error":{"code":"Request_ResourceNotFound"}}"#), " (Request_ResourceNotFound)");
    assert_eq!(named(r#"{"error":{"code":"Microsoft.Graph.Throttled"}}"#), " (Microsoft.Graph.Throttled)");
    for nameless in [
        "",
        "<html>Forbidden</html>",
        "[]",
        r#"{"error":"ErrorAccessDenied"}"#,
        r#"{"error":{"code":5}}"#,
        r#"{"error":{"code":""}}"#,
        r#"{"error":{"code":"two words"}}"#,
        r#"{"error":{"code":"line\r\nbreak"}}"#,
        r#"{"error":{"code":"Größe"}}"#,
        r#"{"error":{"code":"<b>bold</b>"}}"#,
        r#"{"code":"ErrorAccessDenied"}"#,
        // A piece of what was sent is not written back, be it of the token or of the message,
        // short or long, alone or with something around it.
        r#"{"error":{"code":"graph.acc-ess_tok.en-0123456789"}}"#,
        r#"{"error":{"code":"tok.en"}}"#,
        r#"{"error":{"code":"VGVh"}}"#,
        r#"{"error":{"code":"U3ViamVjdDogVGVhIG9uIFRodXJzZGF5Pw0K"}}"#,
        r#"{"error":{"code":"ErrorU3ViamVjdDogVGVhIG9uDenied"}}"#,
        r#"{"error":{"code":"Xess_tok.enX"}}"#,
        r#"{"error":{"code":"Error.0123456789"}}"#,
    ] {
        assert_eq!(named(nameless), "", "{nameless}");
    }
    // A few characters that happen to be in both say nothing.
    assert_eq!(named(r#"{"error":{"code":"ErrorVGVhDenied"}}"#), " (ErrorVGVhDenied)");
    assert_eq!(named(&format!(r#"{{"error":{{"code":"{}"}}}}"#, "x".repeat(MAX_CODE))).len(), MAX_CODE + 3);
    assert_eq!(named(&format!(r#"{{"error":{{"code":"{}"}}}}"#, "x".repeat(MAX_CODE + 1))), "");
}

/// JSON of any shape, with the names Graph uses among its keys and the words it uses among its values.
fn any_json() -> impl Strategy<Value = Value> {
    let name = prop_oneof![
        proptest::sample::select(vec![
            "value",
            "@odata.nextLink",
            "@odata.etag",
            "error",
            "code",
            "message",
            "id",
            "iCalUId",
            "changeKey",
            "subject",
            "bodyPreview",
            "body",
            "contentType",
            "content",
            "start",
            "end",
            "dateTime",
            "timeZone",
            "isAllDay",
            "isCancelled",
            "location",
            "displayName",
            "organizer",
            "attendees",
            "emailAddress",
            "name",
            "address",
            "type",
            "status",
            "response",
            "responseStatus",
            "showAs",
            "sensitivity",
            "seriesMasterId",
            "lastModifiedDateTime",
            "isReminderOn",
            "reminderMinutesBeforeStart",
            "givenName",
            "surname",
            "emailAddresses",
            "businessPhones",
            "homePhones",
            "mobilePhone",
            "businessAddress",
            "homeAddress",
            "otherAddress",
            "street",
            "city",
            "state",
            "postalCode",
            "countryOrRegion",
            "birthday",
            "personalNotes",
            "businessHomePage",
        ])
        .prop_map(str::to_string),
        "\\PC{0,6}",
    ];
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>().prop_map(|number| json!(number)),
        any::<String>().prop_map(Value::from),
        "[0-9]{1,5}-[0-9]{1,3}-[0-9]{1,3}[T ][0-9]{1,3}:[0-9]{1,3}(:[0-9]{1,3})?(\\.[0-9]{0,9})?(Z|[+-][0-9]{1,4}(:[0-9]{1,3})?)?"
            .prop_map(Value::from),
        proptest::sample::select(vec![
            "UTC",
            "utc",
            "Pacific Standard Time",
            "text",
            "html",
            "accepted",
            "declined",
            "tentativelyAccepted",
            "optional",
            "private",
            "oof",
            "occurrence",
            "seriesMaster",
            "e-1",
            "ada@example.org",
            "2026-10-12T13:00:00.0000000",
            "2026-10-12T14:00:00Z",
            "2026-10-12",
            "https://graph.microsoft.com/v1.0/me/contacts?$skip=50",
            "https://evil.example/next",
        ])
        .prop_map(Value::from),
    ];
    leaf.prop_recursive(5, 64, 8, move |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            proptest::collection::vec((name.clone(), inner), 0..10).prop_map(|members| Value::Object(members.into_iter().collect())),
        ]
    })
}

/// An entry that has what it takes to be read, and anything at all for the rest.
fn any_entry() -> impl Strategy<Value = Value> {
    (any_json(), any_json(), "[A-Za-z0-9=_-]{1,12}", any::<bool>()).prop_map(|(mut entry, more, id, timed)| {
        if !entry.is_object() {
            entry = json!({});
        }
        entry["id"] = json!(id);
        if timed {
            entry["start"] = json!({"dateTime": "2026-10-12T13:00:00.0000000", "timeZone": "UTC"});
            entry["end"] = json!({"dateTime": "2026-10-12T14:00:00.0000000", "timeZone": "UTC"});
        }
        for name in ["attendees", "organizer", "body", "emailAddresses", "businessAddress", "homePhones"] {
            if entry.get(name).is_none() {
                entry[name] = more.clone();
            }
        }
        entry
    })
}

proptest! {
    #[test]
    fn arbitrary_json_gives_an_event_and_a_contact_or_none(entry in prop_oneof![any_json(), any_entry()]) {
        if let Some(event) = event_of(&entry) {
            prop_assert!(!event.id.is_empty() && event.end >= event.start && event.attendees.len() <= MAX_PEOPLE);
            prop_assert!(!event.title.chars().any(char::is_control) && event.reminder_minutes.is_none_or(|minutes| minutes <= 40_320));
            let mut again = event.clone();
            again.tidy();
            prop_assert_eq!(again, event);
        }
        if let Some(contact) = contact_of(&entry) {
            prop_assert!(!contact.id.is_empty() && contact.emails.len() <= MAX_ENTRIES && contact.phones.len() <= 2 * MAX_ENTRIES + 1 && contact.addresses.len() <= 3);
            prop_assert!(contact.emails.iter().all(|email| email.value.contains('@')));
            let mut again = contact.clone();
            again.tidy();
            prop_assert_eq!(again, contact);
        }
    }

    #[test]
    fn arbitrary_pages_never_panic(page in any_json(), noise in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = page_of(&noise);
        let graph = Graph::at(GRAPH, TOKEN).unwrap();
        if let Ok(page) = page_of(page.to_string().as_bytes()) {
            for entry in &page.entries {
                let _ = event_of(entry);
                let _ = contact_of(entry);
            }
            // Whatever a page names as the next one, only Graph is followed to.
            if let Some(Ok(next)) = page.next.as_ref().map(|link| graph.next_page(link, &graph.url(CONTACTS))) {
                prop_assert!(next.secure && next.host.eq_ignore_ascii_case("graph.microsoft.com") && matches!(next.port, 0 | 443));
            }
        }
    }

    #[test]
    fn any_next_page_is_on_graph_or_refused(link in "(https?://)?(graph\\.microsoft\\.com|GRAPH\\.Microsoft\\.COM|evil\\.example|127\\.0\\.0\\.1|localhost)?(\\.evil\\.example|@evil\\.example|:443|:80|:8443|\\.)?(/v1\\.0/me/contacts)?(\\?\\$skip=[0-9]{1,3})?\\PC{0,8}") {
        let graph = Graph::at(GRAPH, TOKEN).unwrap();
        if let Ok(next) = graph.next_page(&json!(link), &graph.url(CONTACTS)) {
            prop_assert!(next.secure && next.host.eq_ignore_ascii_case("graph.microsoft.com") && matches!(next.port, 0 | 443), "{}", link);
            prop_assert!(link.to_ascii_lowercase().starts_with("https://graph.microsoft.com"), "{}", link);
        }
    }

    #[test]
    fn any_refusal_reads_as_one_line_without_what_was_sent(
        status in 100u16..600,
        body in prop_oneof![any_json().prop_map(|json| json.to_string().into_bytes()), proptest::collection::vec(any::<u8>(), 0..200)],
        code in "\\PC{0,80}|[A-Za-z0-9_.]{0,70}|graph\\.acc-ess_tok\\.en-0123456789|[A-Za-z]{0,9}(tok\\.en|ess_tok\\.en|U3ViamVjdDogVGVhIG9uIFRodXJzZGF5|VGVhIG9uIFRo)[A-Za-z]{0,9}",
        retry_after in proptest::option::of("\\PC{0,30}|[0-9]{1,25}"),
        now in any::<i64>(),
    ) {
        let mail = "U3ViamVjdDogVGVhIG9uIFRodXJzZGF5Pw0K";
        let named = json!({"error": {"code": code, "message": format!("{TOKEN} {mail}")}}).to_string().into_bytes();
        for body in [body, named] {
            for doing in [Doing::Sending, Doing::Calendar, Doing::Contacts, Doing::Writing] {
                let error = refusal(doing, &Reply { status, body: body.clone(), retry_after: retry_after.clone(), ..Default::default() }, &[TOKEN, mail], now);
                let shown = error.to_string();
                prop_assert!(!shown.contains('\r') && !shown.contains('\n') && shown.chars().count() < 200, "{}", shown);
                // No more than a few characters in a row of the token or of the mail, ever.
                for sent in [TOKEN, mail] {
                    prop_assert!((0..sent.len() - MAX_ECHO).all(|from| !shown.contains(&sent[from..from + MAX_ECHO])), "{}", shown);
                }
                if let NetError::Busy { retry_after, .. } = error {
                    prop_assert!(matches!(status, 429 | 503 | 504) && retry_after.is_none_or(|seconds| seconds <= 86_400));
                }
            }
        }
    }

    #[test]
    fn any_upload_is_written_or_refused_without_panic(
        kinds in proptest::collection::vec(0u8..5, 0..6),
        remote in "\\PC{0,30}",
        series in "\\PC{0,12}",
        occurrence in any::<i64>(),
        rule in (0u8..4, any::<u32>(), proptest::option::of(-3i8..8), proptest::option::of(any::<u32>())),
        flags in (any::<bool>(), any::<bool>(), 0u8..4),
        mark in "\\PC{0,30}|[a-f0-9-]{36}",
        title in "\\PC{0,40}",
        start in any::<i64>(),
        end in any::<i64>(),
        all_day in any::<bool>(),
        zone in "\\PC{0,20}",
        status in prop_oneof![Just(200u16), Just(201), Just(204), Just(400), Just(401), Just(403), Just(404), Just(409), Just(429), Just(500)],
        body in any_json(),
    ) {
        let answer = answer(&format!("{status} Whatever"), "", &body.to_string());
        let server = FakeHttp::start(move |_, _| answer.clone());
        let repeat = EventRepeat {
            rule: Some(Recurrence {
                freq: [Freq::Daily, Freq::Weekly, Freq::Monthly, Freq::Yearly][usize::from(rule.0)],
                interval: rule.1,
                by_day: vec![Weekday::Fri, Weekday::Mon],
                nth: rule.2,
                count: rule.3,
                until: None,
            }),
            first: Date::new(2026, 10, 12).unwrap(),
        };
        let ops = kinds.iter().map(|kind| EventOp {
            kind: [EventChangeKind::Create, EventChangeKind::Update, EventChangeKind::Delete, EventChangeKind::Respond, EventChangeKind::Forward]
                [usize::from(*kind)],
            event: 7,
            remote: if flags.0 { remote.clone() } else { String::new() },
            token: mark.clone(),
            fields: EventFields {
                title: Some(title.clone()),
                time: Some(EventTime { start, end, all_day }),
                repeat: flags.1.then(|| repeat.clone()),
                attendees: Some(vec![Attendee { address: Address::new(title.clone(), remote.clone()), ..Default::default() }]),
                categories: Some(vec![title.clone()]),
                importance: flags.1.then_some(mailcraft_model::Importance::High),
                request_responses: Some(flags.0),
                allow_proposals: Some(flags.1),
                online: (flags.2 > 1).then_some(mailcraft_model::OnlineProvider::Teams),
                ..Default::default()
            },
            proposed: flags.1.then_some(EventTime { start, end, all_day }),
            to: vec![Address::new(title.clone(), remote.clone()), Address::new(remote.clone(), "bo@example.org")],
            series: series.clone(),
            occurrence,
            calendar: series.clone(),
            cancel: flags.1,
            response: [Response::None, Response::Accepted, Response::Tentative, Response::Declined][usize::from(flags.2)],
            send: flags.0,
            comment: title.clone(),
        }).collect::<Vec<_>>();
        let upload = EventsUpload { account: 3, email: "ada@example.org".into(), ops, ..Default::default() };
        let uploaded = push_events_at(&server.url(""), TOKEN, &upload, &EventOptions { zone, ..Default::default() });
        prop_assert!(uploaded.outcomes.len() <= upload.ops.len());
        for outcome in &uploaded.outcomes {
            prop_assert!(!outcome.error.contains('\n') && outcome.error.chars().count() < 300, "{}", outcome.error);
            prop_assert!(!outcome.remote.chars().any(char::is_whitespace) && !outcome.change_key.chars().any(char::is_whitespace));
        }
    }
}

/// What a request's body says, as JSON.
fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}

/// A mark as a new event is sent with.
const MARK: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

fn create(fields: EventFields) -> EventOp {
    EventOp { kind: EventChangeKind::Create, event: 7, token: MARK.into(), fields, ..Default::default() }
}

fn update(remote: &str, fields: EventFields) -> EventOp {
    EventOp { kind: EventChangeKind::Update, event: 8, remote: remote.into(), fields, ..Default::default() }
}

fn delete(remote: &str) -> EventOp {
    EventOp { kind: EventChangeKind::Delete, event: 9, remote: remote.into(), ..Default::default() }
}

fn upload(ops: Vec<EventOp>) -> EventsUpload {
    EventsUpload { account: 3, email: "ada@example.org".into(), ops, ..Default::default() }
}

/// What Graph says to a deletion it carried out.
const DELETED: &[u8] = b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n";

#[test]
fn events_made_changed_and_deleted_here_are_written_to_graph() {
    let server = FakeHttp::start(|n, _| {
        match n {
        0 => json_answer(
            "201 Created",
            &json!({"@odata.etag": "W/\"ck-new==\"", "id": "AAMk-new=", "changeKey": "ck-new==", "iCalUId": "0400-uid", "subject": "Lunch", "transactionId": MARK})
                .to_string(),
        ),
        1 => json_answer("200 OK", &json!({"@odata.etag": "W/\"ck-2==\"", "id": "AAMk/1+x=", "subject": "Budget review"}).to_string()),
        _ => DELETED.to_vec(),
    }
    });
    // 16:00 to 16:30 UTC, which is noon in New York in October.
    let lunch = EventFields {
        title: Some("Lunch".into()),
        location: Some("Canteen".into()),
        description: Some("Bring\nthe numbers.".into()),
        time: Some(EventTime { start: at(2026, 10, 15, 16, 0), end: at(2026, 10, 15, 16, 30), all_day: false }),
        show_as: Some(ShowAs::Busy),
        private: Some(false),
        reminder: Some(EventReminder { on: true, minutes: 15 }),
        ..Default::default()
    };
    let moved =
        EventFields { time: Some(EventTime { start: at(2026, 10, 14, 13, 0), end: at(2026, 10, 14, 14, 0), all_day: false }), ..Default::default() };
    let ops = vec![create(lunch), update("AAMk/1+x=", moved), delete("AAMk-2=")];
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(ops.clone()), &EventOptions::default());
    assert_eq!((uploaded.account, uploaded.email.as_str(), uploaded.token_refused, server.asked()), (3, "ada@example.org", false, 3));

    // A new event: everything about it, with the mark that keeps a second try from making a second one.
    let request = server.request(0);
    assert!(request.starts_with("POST /v1.0/me/events HTTP/1.1\r\n"), "{request}");
    for header in [
        format!("Authorization: Bearer {TOKEN}\r\n"),
        "Accept: application/json\r\n".into(),
        "Content-Type: application/json\r\n".into(),
        "Prefer: outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"\r\n".into(),
    ] {
        assert!(request.contains(&header), "{header:?}: {request}");
    }
    assert_eq!(
        body_of(&request),
        json!({
            "subject": "Lunch",
            "location": {"displayName": "Canteen"},
            "body": {"contentType": "text", "content": "Bring\nthe numbers."},
            "start": {"dateTime": "2026-10-15T16:00:00", "timeZone": "UTC"},
            "end": {"dateTime": "2026-10-15T16:30:00", "timeZone": "UTC"},
            "isAllDay": false,
            "showAs": "busy",
            "sensitivity": "normal",
            "isReminderOn": true,
            "reminderMinutesBeforeStart": 15,
            "transactionId": MARK,
        })
    );
    // A change: to the event by its id (as part of an address), and only what changed.
    let request = server.request(1);
    assert!(request.starts_with("PATCH /v1.0/me/events/AAMk%2F1%2Bx%3D HTTP/1.1\r\n"), "{request}");
    assert_eq!(
        body_of(&request),
        json!({"start": {"dateTime": "2026-10-14T13:00:00", "timeZone": "UTC"}, "end": {"dateTime": "2026-10-14T14:00:00", "timeZone": "UTC"}, "isAllDay": false})
    );
    // A deletion: nothing but the event's id.
    let request = server.request(2);
    assert!(request.starts_with("DELETE /v1.0/me/events/AAMk-2%3D HTTP/1.1\r\n"), "{request}");
    assert!(request.ends_with("\r\n\r\n") && !request.to_lowercase().contains("content-length") && !request.contains("Content-Type"), "{request}");

    // What Graph said: the new event's id, change key and UID; the changed one's new change key.
    let said: Vec<_> =
        uploaded.outcomes.iter().map(|o| (o.result, o.remote.as_str(), o.change_key.as_str(), o.uid.as_str(), o.error.as_str())).collect();
    assert_eq!(
        said,
        [
            (EventResult::Done, "AAMk-new=", "ck-new==", "0400-uid", ""),
            (EventResult::Done, "AAMk/1+x=", "W/\"ck-2==\"", "", ""),
            (EventResult::Done, "", "", "", ""),
        ]
    );
    // Each outcome carries what was asked, for the mailbox to tell what it was about.
    assert_eq!(uploaded.outcomes.iter().map(|o| o.op.clone()).collect::<Vec<_>>(), ops);
}

#[test]
fn what_changed_of_an_event_is_all_that_is_sent() {
    let server = FakeHttp::answering(json_answer("200 OK", r#"{"id":"e-1","changeKey":"ck"}"#));
    let sent = |fields: EventFields| {
        let before = server.asked();
        let nothing = fields.is_empty();
        let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![update("e-1", fields)]), &EventOptions::default());
        // A change that says nothing is never called done: nothing was done.
        let expected = if nothing { EventResult::Refused } else { EventResult::Done };
        assert_eq!(uploaded.outcomes[0].result, expected, "{}", uploaded.outcomes[0].error);
        (server.asked() > before).then(|| body_of(&server.request(before)))
    };
    let fields = EventFields::default;
    assert_eq!(sent(EventFields { title: Some("Renamed".into()), ..fields() }), Some(json!({"subject": "Renamed"})));
    assert_eq!(sent(EventFields { location: Some(String::new()), ..fields() }), Some(json!({"location": {"displayName": ""}})));
    assert_eq!(
        sent(EventFields { description: Some("New\ntext".into()), ..fields() }),
        Some(json!({"body": {"contentType": "text", "content": "New\ntext"}}))
    );
    for (show_as, name) in [
        (ShowAs::Free, "free"),
        (ShowAs::Tentative, "tentative"),
        (ShowAs::Busy, "busy"),
        (ShowAs::OutOfOffice, "oof"),
        (ShowAs::WorkingElsewhere, "workingElsewhere"),
    ] {
        assert_eq!(sent(EventFields { show_as: Some(show_as), ..fields() }), Some(json!({"showAs": name})));
    }
    assert_eq!(sent(EventFields { private: Some(true), ..fields() }), Some(json!({"sensitivity": "private"})));
    assert_eq!(sent(EventFields { private: Some(false), ..fields() }), Some(json!({"sensitivity": "normal"})));
    let reminder = |on: bool, minutes: u32| EventFields { reminder: Some(EventReminder { on, minutes }), ..fields() };
    assert_eq!(sent(reminder(true, 0)), Some(json!({"isReminderOn": true, "reminderMinutesBeforeStart": 0})));
    assert_eq!(sent(reminder(true, u32::MAX)), Some(json!({"isReminderOn": true, "reminderMinutesBeforeStart": 40_320})));
    assert_eq!(sent(reminder(false, 15)), Some(json!({"isReminderOn": false})));
    // Text is cut to size and to one line where it is one; nothing in it can say more than it is.
    let long = sent(EventFields { title: Some(format!("Budget\r\nreview{}", "x".repeat(5_000))), ..fields() }).unwrap();
    assert_eq!(long["subject"].as_str().map(|s| (s.chars().count(), s.starts_with("Budget  review"))), Some((500, true)));
    // A change that says nothing: Graph isn't asked, and the answer doesn't say "done".
    assert_eq!(sent(fields()), None);
}

#[test]
fn times_and_whole_days_are_written_as_graph_takes_them() {
    let server = FakeHttp::answering(json_answer("200 OK", r#"{"id":"e-1","changeKey":"ck"}"#));
    let sent = |time: EventTime, zone: &str| {
        let before = server.asked();
        let fields = EventFields { time: Some(time), ..Default::default() };
        push_events_at(&server.url(""), TOKEN, &upload(vec![update("e-1", fields)]), &EventOptions { zone: zone.into(), ..Default::default() });
        body_of(&server.request(before))
    };
    let stamp = |text: &str, zone: &str| json!({"dateTime": text, "timeZone": zone});
    // Instants go as UTC, whatever the zone: nine in the morning in New York is 13:00 UTC the
    // week before the clocks go back and 14:00 UTC the week after, and each is said as it is.
    for zone in ["", "Eastern Standard Time"] {
        let before = sent(EventTime { start: at(2026, 10, 26, 13, 0), end: at(2026, 10, 26, 14, 0), all_day: false }, zone);
        assert_eq!((before["start"].clone(), before["end"].clone()), (stamp("2026-10-26T13:00:00", "UTC"), stamp("2026-10-26T14:00:00", "UTC")));
        let after = sent(EventTime { start: at(2026, 11, 2, 14, 0), end: at(2026, 11, 2, 15, 0), all_day: false }, zone);
        assert_eq!(
            (after["start"].clone(), after["end"].clone(), after["isAllDay"].clone()),
            (stamp("2026-11-02T14:00:00", "UTC"), stamp("2026-11-02T15:00:00", "UTC"), json!(false))
        );
    }
    // One that runs over the change is three and a half hours long, as it is on the clock.
    let night = sent(EventTime { start: at(2026, 11, 1, 4, 30), end: at(2026, 11, 1, 8, 0), all_day: false }, "");
    assert_eq!((night["start"]["dateTime"].as_str(), night["end"]["dateTime"].as_str()), (Some("2026-11-01T04:30:00"), Some("2026-11-01T08:00:00")));
    // An end before the start is no length, not a negative one.
    let backwards = sent(EventTime { start: at(2026, 10, 26, 13, 0), end: at(2026, 10, 26, 12, 0), all_day: false }, "");
    assert_eq!(backwards["end"], stamp("2026-10-26T13:00:00", "UTC"));

    // Whole days: midnights, in the owner's zone when it is known (a day is a day there), from
    // the first day to the day after the last.
    let days = EventTime { start: at(2026, 10, 31, 0, 0), end: at(2026, 11, 2, 0, 0), all_day: true };
    let whole = sent(days, "Eastern Standard Time");
    assert_eq!(
        whole,
        json!({"start": stamp("2026-10-31T00:00:00", "Eastern Standard Time"), "end": stamp("2026-11-02T00:00:00", "Eastern Standard Time"), "isAllDay": true})
    );
    assert_eq!(sent(days, "Europe/Berlin")["start"], stamp("2026-10-31T00:00:00", "Europe/Berlin"));
    // In UTC when it isn't, or when what is given can't be the name of a zone.
    for zone in ["", "  ", "Eastern\r\nStandard", "\"quoted\"", &"x".repeat(100)] {
        assert_eq!(sent(days, zone)["start"], stamp("2026-10-31T00:00:00", "UTC"), "{zone:?}");
    }
    // At least one day, and whole days whatever time of day is named.
    let odd = sent(EventTime { start: at(2026, 10, 31, 9, 30), end: at(2026, 10, 31, 9, 30), all_day: true }, "");
    assert_eq!((odd["start"].clone(), odd["end"].clone()), (stamp("2026-10-31T00:00:00", "UTC"), stamp("2026-11-01T00:00:00", "UTC")));

    // A zone Graph doesn't know (400): the same days are sent once more in UTC, and that is all.
    let picky = FakeHttp::start(|_, request| {
        if request.contains("Mars/Olympus") {
            answer("400 Bad Request", "", &graph_error("ErrorInvalidRequest", "Unknown time zone."))
        } else {
            json_answer("201 Created", r#"{"id":"e-new","changeKey":"ck"}"#)
        }
    });
    let whole_day = create(EventFields { title: Some("Offsite".into()), time: Some(days), ..Default::default() });
    let uploaded =
        push_events_at(&picky.url(""), TOKEN, &upload(vec![whole_day.clone()]), &EventOptions { zone: "Mars/Olympus".into(), ..Default::default() });
    assert_eq!((uploaded.outcomes[0].result, uploaded.outcomes[0].remote.as_str(), picky.asked()), (EventResult::Done, "e-new", 2));
    assert_eq!(body_of(&picky.request(0))["start"], stamp("2026-10-31T00:00:00", "Mars/Olympus"));
    assert_eq!(body_of(&picky.request(1))["start"], stamp("2026-10-31T00:00:00", "UTC"));
    // Refused in UTC too, it is refused; and an event with times is not asked about twice.
    let never = FakeHttp::answering(answer("400 Bad Request", "", &graph_error("ErrorInvalidRequest", "No.")));
    let uploaded =
        push_events_at(&never.url(""), TOKEN, &upload(vec![whole_day]), &EventOptions { zone: "Mars/Olympus".into(), ..Default::default() });
    assert_eq!((uploaded.outcomes[0].result, never.asked()), (EventResult::Refused, 2));
    let timed =
        create(EventFields { title: Some("Lunch".into()), time: Some(EventTime { start: 0, end: 60, all_day: false }), ..Default::default() });
    let uploaded = push_events_at(&never.url(""), TOKEN, &upload(vec![timed]), &EventOptions { zone: "Mars/Olympus".into(), ..Default::default() });
    assert_eq!((uploaded.outcomes[0].result, never.asked()), (EventResult::Refused, 3));
}

#[test]
fn what_graph_says_to_a_change_it_does_not_take() {
    let title = || EventFields { title: Some("The secret plan".into()), ..Default::default() };
    let ops = || vec![create(title()), update("e-1", title()), delete("e-2")];
    // What each status comes to for a new event, a change and a deletion; and whether the rest
    // of the upload is left for later.
    use EventResult::{Denied, Done, Failed, Gone, Refused};
    for (status, headers, results, stops) in [
        ("400 Bad Request", "", [Refused, Refused, Refused], false),
        ("403 Forbidden", "", [Denied, Denied, Denied], false),
        ("404 Not Found", "", [Refused, Gone, Gone], false),
        ("405 Method Not Allowed", "", [Refused, Refused, Refused], false),
        // A new event that is there already (its mark is known) waits for the download to say so.
        ("409 Conflict", "", [Failed, Refused, Refused], false),
        ("410 Gone", "", [Refused, Gone, Gone], false),
        ("412 Precondition Failed", "", [Refused, Refused, Refused], false),
        ("413 Request Entity Too Large", "", [Refused, Refused, Refused], false),
        ("429 Too Many Requests", "Retry-After: 30\r\n", [Failed, Failed, Failed], true),
        ("500 Internal Server Error", "", [Failed, Failed, Failed], false),
        ("503 Service Unavailable", "", [Failed, Failed, Failed], true),
        ("504 Gateway Timeout", "", [Failed, Failed, Failed], true),
        // A yes is a yes, however it is worded: taking it for a no would put back here what the
        // server has taken on.
        ("202 Accepted", "", [Done, Done, Done], false),
        ("204 No Content", "", [Done, Done, Done], false),
        ("301 Moved Permanently", "Location: https://evil.example/v1.0/me/events\r\n", [Refused, Refused, Refused], false),
    ] {
        let server = FakeHttp::answering(answer(status, headers, &graph_error("ErrorWhatever", "No: The secret plan.")));
        let uploaded = push_events_at(&server.url(""), TOKEN, &upload(ops()), &EventOptions::default());
        assert!(!uploaded.token_refused, "{status}");
        let expected = if stops { 1 } else { 3 };
        assert_eq!((uploaded.outcomes.len(), server.asked()), (expected, expected), "{status}");
        for (outcome, result) in uploaded.outcomes.iter().zip(results) {
            assert_eq!(outcome.result, result, "{status} {:?}", outcome.op.kind);
            // Why, in a line, and never with what was sent.
            if result != Done {
                assert!(
                    !outcome.error.is_empty() && !outcome.error.contains("secret") && !outcome.error.contains(MARK),
                    "{status}: {}",
                    outcome.error
                );
            }
            assert_eq!(outcome.retry_after, (status.starts_with("429")).then_some(30), "{status}");
        }
    }
    // What the refusals read like.
    let said = |status: &str, op: EventOp| {
        let server = FakeHttp::answering(answer(status, "", &graph_error("ErrorAccessDenied", "Access is denied.")));
        push_events_at(&server.url(""), TOKEN, &upload(vec![op]), &EventOptions::default()).outcomes.remove(0).error
    };
    assert_eq!(said("403 Forbidden", update("e-1", title())), "this account may not change its calendar through Microsoft Graph (ErrorAccessDenied)");
    assert_eq!(said("400 Bad Request", create(title())), "Microsoft Graph answered with status 400 (ErrorAccessDenied)");
    assert_eq!(said("503 Service Unavailable", delete("e-2")), "Microsoft Graph is busy; try again later");

    // The token isn't taken (401): nothing more goes with it, and what wasn't tried says nothing.
    let server = FakeHttp::start(|n, _| match n {
        0 => json_answer("201 Created", r#"{"id":"e-new","changeKey":"ck"}"#),
        _ => answer("401 Unauthorized", "", &graph_error("InvalidAuthenticationToken", "Access token has expired.")),
    });
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(ops()), &EventOptions::default());
    assert_eq!((uploaded.token_refused, uploaded.outcomes.len(), uploaded.outcomes[0].result, server.asked()), (true, 1, Done, 2));
    // No token, or what couldn't be one: nothing is asked at all.
    for token in ["", "two words", "tok\r\nX-Evil: 1"] {
        let uploaded = push_events_at(&server.url(""), token, &upload(ops()), &EventOptions::default());
        assert!(uploaded.token_refused && uploaded.outcomes.is_empty(), "{token:?}");
    }
    assert_eq!(server.asked(), 2);

    // Graph can't be reached: the change waits, and the rest isn't tried against a dead line.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let uploaded = push_events_at(&format!("http://127.0.0.1:{port}"), TOKEN, &upload(ops()), &EventOptions::default());
    assert_eq!((uploaded.outcomes.len(), uploaded.outcomes[0].result, uploaded.token_refused), (1, Failed, false));
    assert!(!uploaded.outcomes[0].error.is_empty());
    // An answer that isn't one, or that breaks off: the same.
    for broken in [b"220 smtp.example.org ESMTP\r\n".to_vec(), Vec::new()] {
        let server = FakeHttp::answering(broken);
        let uploaded = push_events_at(&server.url(""), TOKEN, &upload(ops()), &EventOptions::default());
        assert_eq!((uploaded.outcomes.len(), uploaded.outcomes[0].result), (1, Failed));
    }

    // An event that was made, though the answer doesn't say under what name: done, with no name
    // (the download that follows knows it by its mark).
    let server = FakeHttp::answering(json_answer("201 Created", "{}"));
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![create(title())]), &EventOptions::default());
    assert_eq!((uploaded.outcomes[0].result, uploaded.outcomes[0].remote.as_str()), (Done, ""));
    // An id that can't be one is no id.
    let server = FakeHttp::answering(json_answer("201 Created", &json!({"id": "two words", "changeKey": ["x"], "iCalUId": 7}).to_string()));
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![create(title())]), &EventOptions::default());
    assert_eq!((uploaded.outcomes[0].remote.as_str(), uploaded.outcomes[0].change_key.as_str(), uploaded.outcomes[0].uid.as_str()), ("", "", ""));
}

#[test]
fn what_cannot_be_a_change_is_refused_before_graph_is_asked() {
    let server = FakeHttp::answering(json_answer("200 OK", r#"{"id":"e-1"}"#));
    let title = || EventFields { title: Some("x".into()), ..Default::default() };
    let nameless = vec![
        // No mark to make it with; no id to change or delete it by; an id that can't be one.
        EventOp { token: String::new(), ..create(title()) },
        EventOp { token: "not a mark!".into(), ..create(title()) },
        update("", title()),
        delete(""),
        update("two words", title()),
        delete("line\r\nbreak"),
        delete(&"x".repeat(2_000)),
    ];
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(nameless.clone()), &EventOptions::default());
    assert_eq!(uploaded.outcomes.len(), nameless.len());
    for outcome in &uploaded.outcomes {
        assert_eq!((outcome.result, outcome.error.as_str()), (EventResult::Refused, "the change doesn't say which event it is about"));
    }
    assert_eq!(server.asked(), 0);
    // No more than so many go in one upload: the rest wait for the next.
    let many = upload((0..MAX_EVENT_OPS + 20).map(|n| delete(&format!("e-{n}"))).collect());
    let server = FakeHttp::answering(DELETED.to_vec());
    let uploaded = push_events_at(&server.url(""), TOKEN, &many, &EventOptions::default());
    assert_eq!((uploaded.outcomes.len(), server.asked()), (MAX_EVENT_OPS, MAX_EVENT_OPS));
    // Graph is spoken to encrypted, or on this machine.
    let uploaded = push_events_at("http://graph.example.org", TOKEN, &upload(vec![delete("e-1")]), &EventOptions::default());
    assert!(uploaded.token_refused && uploaded.outcomes.is_empty());
}

#[test]
fn whose_meeting_it_is_and_the_mark_of_a_new_event_come_with_the_calendar() {
    let mine = json!({"isOrganizer": true, "transactionId": MARK});
    let theirs = json!({"isOrganizer": false, "transactionId": "not a mark!"});
    let entry = |id: &str, more: &Value| {
        let mut entry = event(id, "x");
        for (name, value) in more.as_object().unwrap() {
            entry[name] = value.clone();
        }
        entry
    };
    let server = FakeHttp::answering(page(
        vec![entry("e-1", &mine), entry("e-2", &theirs), event("e-3", "x"), entry("e-4", &json!({"isOrganizer": "yes", "transactionId": 7}))],
        None,
    ));
    let (from, to) = october();
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    let read: Vec<_> = download.events.iter().map(|e| (e.id.as_str(), e.is_organizer, e.token.as_str())).collect();
    assert_eq!(read, [("e-1", Some(true), MARK), ("e-2", Some(false), ""), ("e-3", None, ""), ("e-4", None, "")]);
    let (_, query) = asked_for(&server.request(0));
    assert!(query.iter().any(|(name, value)| name == "$select" && value.contains(",isOrganizer,transactionId,")), "{query:?}");
    assert_eq!(server.asked(), 1);

    // Should Graph not have the question with the newest fields in it (400), it is put as it was
    // before them, and before those that were added before them: the calendar still comes.
    let server = FakeHttp::start(|n, _| match n {
        0..=2 => answer("400 Bad Request", "", &graph_error("BadRequest", "Could not find a property named 'transactionId'.")),
        _ => page(vec![event("e-1", "x")], None),
    });
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    assert_eq!((download.events.len(), download.complete, server.asked()), (1, true, 4));
    assert!(!download.has_categories && !download.has_options);
    let select = |n: usize| asked_for(&server.request(n)).1.into_iter().find(|(name, _)| name == "$select").map(|(_, value)| value).unwrap();
    assert_eq!(
        (select(0), select(1), select(2), select(3)),
        (EVENT_FIELDS_OPTIONS.to_string(), EVENT_FIELDS_FULL.to_string(), EVENT_FIELDS.to_string(), EVENT_FIELDS_PLAIN.to_string())
    );
    assert!(
        !EVENT_FIELDS_PLAIN.contains("transactionId") && EVENT_FIELDS.starts_with(EVENT_FIELDS_PLAIN) && EVENT_FIELDS_FULL.starts_with(EVENT_FIELDS)
    );
    assert!(EVENT_FIELDS_OPTIONS.starts_with(EVENT_FIELDS_FULL) && !EVENT_FIELDS_FULL.contains("importance"));
    // Refused once, for the newest fields only: the categories still come, and say so.
    let server = FakeHttp::start(|n, _| match n {
        0 => answer("400 Bad Request", "", &graph_error("BadRequest", "Could not find a property named 'onlineMeeting'.")),
        _ => page(vec![event("e-1", "x")], None),
    });
    let download = events_at(&server.url(""), TOKEN, from, to).unwrap();
    assert_eq!((server.asked(), download.has_categories, download.has_options), (2, true, false));
    // Any other refusal is what it is, and asked about once.
    let server = FakeHttp::answering(answer("403 Forbidden", "", &graph_error("ErrorAccessDenied", "Access is denied.")));
    assert!(matches!(events_at(&server.url(""), TOKEN, from, to), Err(NetError::Forbidden(_))));
    assert_eq!(server.asked(), 1);
}

/// A series as Graph has it, repeating as `recurrence` says.
fn series(id: &str, subject: &str, recurrence: Value) -> Value {
    let mut entry = event(id, subject);
    entry["type"] = json!("seriesMaster");
    entry["recurrence"] = recurrence;
    entry
}

/// One occurrence of the series `of`, an hour from `hour` on a day of October 2026.
fn occurrence(id: &str, of: &str, day: u8, hour: u8, kind: &str) -> Value {
    json!({
        "id": id,
        "subject": "Stand-up",
        "type": kind,
        "seriesMasterId": of,
        "start": {"dateTime": format!("2026-10-{day:02}T{hour:02}:00:00.0000000"), "timeZone": "UTC"},
        "end": {"dateTime": format!("2026-10-{day:02}T{:02}:00:00.0000000", hour + 1), "timeZone": "UTC"},
        "originalStart": format!("2026-10-{day:02}T13:00:00Z"),
        "categories": ["Team", "Team", 7, " Red category "],
    })
}

/// The answer to a batch: each question's number with a status and a body.
fn batch_answer(answers: Vec<(&str, u16, Value)>) -> Vec<u8> {
    let responses: Vec<Value> = answers.into_iter().map(|(id, status, body)| json!({"id": id, "status": status, "body": body})).collect();
    json_answer("200 OK", &json!({"responses": responses}).to_string())
}

fn weekly() -> Value {
    json!({
        "pattern": {"type": "weekly", "interval": 2, "daysOfWeek": ["wednesday", "monday"], "firstDayOfWeek": "sunday", "index": "first", "month": 0, "dayOfMonth": 0},
        "range": {"type": "endDate", "startDate": "2026-10-05", "endDate": "2026-12-23", "recurrenceTimeZone": "Eastern Standard Time", "numberOfOccurrences": 0},
    })
}

#[test]
fn a_series_comes_as_itself_beside_its_occurrences() {
    let first_weekday = json!({
        "pattern": {"type": "relativeMonthly", "interval": 1, "daysOfWeek": ["monday", "tuesday", "wednesday", "thursday", "friday"], "index": "first"},
        "range": {"type": "noEnd", "startDate": "2026-10-01"},
    });
    let server = FakeHttp::start(move |n, _| match n {
        0 => page(
            vec![
                occurrence("o-5", "s-1", 5, 13, "occurrence"),
                occurrence("o-7", "s-1", 7, 15, "exception"),
                occurrence("p-1", "s-2", 1, 9, "occurrence"),
                event("e-1", "Dentist"),
            ],
            None,
        ),
        // Answers come in any order, each with the number of its question; one that names no
        // question, or one that was never asked, is no answer.
        _ => batch_answer(vec![
            ("2", 404, json!({"error": {"code": "ErrorItemNotFound"}})),
            ("1", 200, series("s-2", "First weekday", first_weekday.clone())),
            ("0", 200, series("ignored", "Stand-up", weekly())),
            ("7", 200, series("s-9", "Never asked for", weekly())),
            ("x", 200, series("s-8", "No question", weekly())),
        ]),
    });
    let (from, to) = october();
    let query = EventsQuery { from, to, series: vec!["s-3".into(), "s-1".into(), "two words".into()], ..Default::default() };
    let download = events_in_at(&server.url(""), TOKEN, &query).unwrap();
    assert_eq!((server.asked(), download.complete), (2, true));
    // One request for all of them: those the stretch names, then those the mailbox expected.
    let request = server.request(1);
    assert!(request.starts_with("POST /v1.0/$batch HTTP/1.1\r\n") && request.contains(&format!("Authorization: Bearer {TOKEN}\r\n")), "{request}");
    let asked = body_of(&request);
    let urls: Vec<&str> = asked["requests"].as_array().unwrap().iter().map(|one| one["url"].as_str().unwrap()).collect();
    assert_eq!(urls.len(), 3);
    for (url, id) in urls.iter().zip(["s-1", "s-2", "s-3"]) {
        assert!(
            url.starts_with(&format!("/me/events/{id}?%24select=id%2C"))
                && url.contains("%2Crecurrence%2CoriginalStartTimeZone%2CoriginalEndTimeZone%2C")
                && url.ends_with("%2ChasAttachments"),
            "{url}"
        );
    }
    assert_eq!(asked["requests"][0]["method"], "GET");
    assert_eq!(asked["requests"][0]["headers"]["Prefer"], "outlook.timezone=\"UTC\", outlook.body-content-type=\"text\"");

    let read: Vec<_> = download.events.iter().map(|e| (e.id.as_str(), e.kind, e.series.as_str(), e.original)).collect();
    assert_eq!(
        read,
        [
            ("o-5", RemoteEventKind::Occurrence, "s-1", Some(at(2026, 10, 5, 13, 0))),
            ("o-7", RemoteEventKind::Exception, "s-1", Some(at(2026, 10, 7, 13, 0))),
            ("p-1", RemoteEventKind::Occurrence, "s-2", Some(at(2026, 10, 1, 13, 0))),
            ("e-1", RemoteEventKind::Single, "", None),
            // The series: under the name it was asked for by, whatever the answer calls it.
            ("s-1", RemoteEventKind::SeriesMaster, "", None),
            ("s-2", RemoteEventKind::SeriesMaster, "", None),
        ]
    );
    assert_eq!(download.events[0].categories, ["Team", "Red category"]);
    let day = |month: u8, day: u8| Date::new(2026, month, day).unwrap();
    let rule =
        Recurrence { freq: Freq::Weekly, interval: 2, by_day: vec![Weekday::Mon, Weekday::Wed], until: Some(day(12, 23)), ..Default::default() };
    assert_eq!((download.events[4].recurrence.clone(), download.events[4].series_day), (Some(rule), Some(day(10, 5))));
    // "The first weekday of the month" is a rule the mailbox can't say: the series comes without one.
    assert_eq!((download.events[5].recurrence.clone(), download.events[5].series_day), (None, None));
    // And the one the server no longer has is said to be gone.
    assert_eq!(download.gone, ["s-3"]);

    // Graph won't take questions together: they are asked one by one. One that can't be asked
    // just now is left out, and nothing is said to be gone for that.
    let server = FakeHttp::start(|n, request| match n {
        0 => page(vec![occurrence("o-5", "s-1", 5, 13, "occurrence")], None),
        1 => answer("405 Method Not Allowed", "", "{}"),
        _ if request.starts_with("GET /v1.0/me/events/s-1?") => json_answer("200 OK", &series("s-1", "Stand-up", weekly()).to_string()),
        _ if request.starts_with("GET /v1.0/me/events/s-3?") => answer("404 Not Found", "", "{}"),
        _ => answer("503 Service Unavailable", "", "{}"),
    });
    let query = EventsQuery { from, to, series: vec!["s-3".into(), "s-4".into()], ..Default::default() };
    let download = events_in_at(&server.url(""), TOKEN, &query).unwrap();
    assert_eq!(server.asked(), 5);
    assert_eq!(download.events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["o-5", "s-1"]);
    assert_eq!((download.gone.clone(), download.complete), (vec!["s-3".to_string()], true));
    // A batch that fails altogether leaves the calendar as complete as it was, without series.
    let server = FakeHttp::start(|n, _| match n {
        0 => page(vec![occurrence("o-5", "s-1", 5, 13, "occurrence")], None),
        _ => answer("503 Service Unavailable", "", "{}"),
    });
    let download = events_in_at(&server.url(""), TOKEN, &EventsQuery { from, to, ..Default::default() }).unwrap();
    assert_eq!((download.events.len(), download.gone.len(), download.complete, server.asked()), (1, 0, true, 2));

    // Another calendar of the account is asked for by what the server calls it.
    let server = FakeHttp::answering(page(vec![event("e-1", "x")], None));
    let download = events_in_at(&server.url(""), TOKEN, &EventsQuery { calendar: "AAMk/cal+2=".into(), from, to, ..Default::default() }).unwrap();
    assert!(server.request(0).starts_with("GET /v1.0/me/calendars/AAMk%2Fcal%2B2%3D/calendarView?startDateTime="), "{}", server.request(0));
    assert_eq!(download.calendar, "AAMk/cal+2=");
    let odd = EventsQuery { calendar: "two words".into(), from, to, ..Default::default() };
    assert!(matches!(events_in_at(&server.url(""), TOKEN, &odd), Err(NetError::Config(_))));
}

#[test]
fn how_a_series_repeats_is_read_and_written() {
    let day = |month: u8, day: u8| Date::new(2026, month, day).unwrap();
    let rule = |freq: Freq| Recurrence { freq, interval: 1, ..Default::default() };
    // Monday 2026-10-12 is the second Monday of October.
    let first = day(10, 12);
    let cases: Vec<(Recurrence, Value, Value)> = vec![
        (rule(Freq::Daily), json!({"type": "daily", "interval": 1}), json!({"type": "noEnd"})),
        (
            Recurrence { interval: 3, count: Some(10), ..rule(Freq::Daily) },
            json!({"type": "daily", "interval": 3}),
            json!({"type": "numbered", "numberOfOccurrences": 10}),
        ),
        (
            Recurrence { by_day: vec![Weekday::Mon, Weekday::Fri], until: Some(day(12, 18)), ..rule(Freq::Weekly) },
            json!({"type": "weekly", "interval": 1, "daysOfWeek": ["monday", "friday"], "firstDayOfWeek": "monday"}),
            json!({"type": "endDate", "endDate": "2026-12-18"}),
        ),
        (
            Recurrence { interval: 2, ..rule(Freq::Monthly) },
            json!({"type": "absoluteMonthly", "interval": 2, "dayOfMonth": 12}),
            json!({"type": "noEnd"}),
        ),
        (
            Recurrence { by_day: vec![Weekday::Mon], nth: Some(2), ..rule(Freq::Monthly) },
            json!({"type": "relativeMonthly", "interval": 1, "daysOfWeek": ["monday"], "index": "second"}),
            json!({"type": "noEnd"}),
        ),
        (rule(Freq::Yearly), json!({"type": "absoluteYearly", "interval": 1, "dayOfMonth": 12, "month": 10}), json!({"type": "noEnd"})),
        (
            Recurrence { by_day: vec![Weekday::Mon], nth: Some(-1), ..rule(Freq::Yearly) },
            json!({"type": "relativeYearly", "interval": 1, "daysOfWeek": ["monday"], "index": "last", "month": 10}),
            json!({"type": "noEnd"}),
        ),
    ];
    for (rule, pattern, range) in cases {
        let written = recurrence_body(&rule, first, "Pacific Standard Time", 0).unwrap_or_else(|| panic!("{rule:?}"));
        let mut expected = range.clone();
        expected["startDate"] = json!("2026-10-12");
        expected["recurrenceTimeZone"] = json!("Pacific Standard Time");
        assert_eq!(written, json!({"pattern": pattern, "range": expected}), "{rule:?}");
        // And it reads back as it was.
        assert_eq!(rule_of(&written), Some((rule.clone(), first)), "{written}");
    }
    // A week without days named is one on the day it starts; no zone is UTC.
    let written = recurrence_body(&rule(Freq::Weekly), first, "", 0).unwrap();
    assert_eq!(written["pattern"]["daysOfWeek"], json!(["monday"]));
    assert_eq!(written["range"]["recurrenceTimeZone"], "UTC");
    // "Every weekday" is said as a week with those days in it.
    let weekdays = Recurrence { by_day: vec![Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri], ..rule(Freq::Daily) };
    assert_eq!(recurrence_body(&weekdays, first, "", 0).unwrap()["pattern"]["type"], "weekly");
    // What Graph has no way of saying is not said some other way.
    for odd in [
        Recurrence { interval: 2, by_day: vec![Weekday::Mon], ..rule(Freq::Daily) },
        Recurrence { by_day: vec![Weekday::Mon, Weekday::Tue], nth: Some(1), ..rule(Freq::Monthly) },
        Recurrence { by_day: vec![Weekday::Mon], nth: Some(5), ..rule(Freq::Monthly) },
    ] {
        assert_eq!(recurrence_body(&odd, first, "", 0), None, "{odd:?}");
    }
    // And what the mailbox has no way of saying is not read as something near it.
    let read = |pattern: Value, range: Value| rule_of(&json!({"pattern": pattern, "range": range}));
    let no_end = || json!({"type": "noEnd", "startDate": "2026-10-12"});
    for pattern in [
        json!({"type": "absoluteMonthly", "interval": 1, "dayOfMonth": 15}),
        json!({"type": "absoluteYearly", "interval": 1, "dayOfMonth": 12, "month": 11}),
        json!({"type": "relativeMonthly", "interval": 1, "daysOfWeek": ["monday", "tuesday"], "index": "first"}),
        json!({"type": "relativeMonthly", "interval": 1, "daysOfWeek": ["monday"], "index": "sixth"}),
        json!({"type": "weekly", "interval": 1, "daysOfWeek": ["someday"]}),
        json!({"type": "weekly", "interval": 0, "daysOfWeek": ["monday"]}),
        json!({"type": "daily", "interval": 5000}),
        json!({"type": "hourly", "interval": 1}),
    ] {
        assert_eq!(read(pattern.clone(), no_end()), None, "{pattern}");
    }
    let daily = || json!({"type": "daily", "interval": 1});
    for range in [
        json!({"type": "noEnd"}),
        json!({"type": "noEnd", "startDate": "2026-13-45"}),
        json!({"type": "endDate", "startDate": "2026-10-12"}),
        json!({"type": "numbered", "startDate": "2026-10-12", "numberOfOccurrences": 0}),
        json!({"type": "forever", "startDate": "2026-10-12"}),
    ] {
        assert_eq!(read(daily(), range.clone()), None, "{range}");
    }
    assert_eq!(rule_of(&json!({"pattern": daily()})), None);
    assert_eq!(rule_of(&json!("daily")), None);
}

#[test]
fn one_occurrence_of_a_series_is_found_by_its_time() {
    let instances = || {
        page(
            vec![
                json!({"id": "o-18", "start": {"dateTime": "2026-10-18T13:00:00.0000000", "timeZone": "UTC"}, "originalStart": "2026-10-18T13:00:00Z"}),
                // Moved to the afternoon: still the one the series has at one o'clock.
                json!({"id": "o/19+x=", "start": {"dateTime": "2026-10-19T17:00:00.0000000", "timeZone": "UTC"}, "originalStart": "2026-10-19T13:00:00Z"}),
                json!({"id": "two words", "start": {"dateTime": "2026-10-19T13:00:00.0000000", "timeZone": "UTC"}}),
                json!({"start": {"dateTime": "2026-10-19T13:00:00.0000000", "timeZone": "UTC"}}),
                json!("noise"),
            ],
            None,
        )
    };
    let one = |kind: EventChangeKind, when: Timestamp| EventOp {
        kind,
        event: 7,
        series: "s/1=".into(),
        occurrence: when,
        fields: EventFields { title: (kind == EventChangeKind::Update).then(|| "Stand-up, later".to_string()), ..Default::default() },
        ..Default::default()
    };
    let server = FakeHttp::start(move |n, _| match n {
        0 | 2 | 4 => instances(),
        1 => json_answer("200 OK", r#"{"changeKey":"ck-2"}"#),
        _ => DELETED.to_vec(),
    });
    let ops = vec![
        one(EventChangeKind::Update, at(2026, 10, 19, 13, 0)),
        one(EventChangeKind::Delete, at(2026, 10, 18, 13, 0)),
        // The series has nothing at two o'clock: there is nothing to delete.
        one(EventChangeKind::Delete, at(2026, 10, 19, 14, 0)),
    ];
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(ops), &EventOptions::default());
    assert_eq!(server.asked(), 5);
    let request = server.request(0);
    assert!(
        request.starts_with(
            "GET /v1.0/me/events/s%2F1%3D/instances?startDateTime=2026-10-18T13%3A00%3A00Z&endDateTime=2026-10-20T13%3A00%3A00Z&%24top=50&%24select=id%2Cstart%2CoriginalStart%2CisAllDay HTTP/1.1\r\n"
        ),
        "{request}"
    );
    assert!(server.request(1).starts_with("PATCH /v1.0/me/events/o%2F19%2Bx%3D HTTP/1.1\r\n"), "{}", server.request(1));
    assert_eq!(body_of(&server.request(1)), json!({"subject": "Stand-up, later"}));
    assert!(server.request(3).starts_with("DELETE /v1.0/me/events/o-18 HTTP/1.1\r\n"), "{}", server.request(3));
    let said: Vec<_> = uploaded.outcomes.iter().map(|o| (o.result, o.remote.as_str(), o.change_key.as_str())).collect();
    // The changed one is known by its own name from now on, though the answer didn't say it.
    assert_eq!(said, [(EventResult::Done, "o/19+x=", "ck-2"), (EventResult::Done, "", ""), (EventResult::Gone, "", "")]);

    // A whole-day occurrence is named by its day, wherever the series puts its midnight.
    let server = FakeHttp::start(|n, _| match n {
        0 => page(
            vec![
                json!({"id": "d-19", "isAllDay": true, "start": {"dateTime": "2026-10-19T00:00:00.0000000", "timeZone": "UTC"}, "originalStart": "2026-10-18T11:00:00Z"}),
                json!({"id": "d-20", "isAllDay": true, "start": {"dateTime": "2026-10-20T00:00:00.0000000", "timeZone": "UTC"}, "originalStart": "2026-10-19T11:00:00Z"}),
            ],
            None,
        ),
        _ => DELETED.to_vec(),
    });
    push_events_at(&server.url(""), TOKEN, &upload(vec![one(EventChangeKind::Delete, at(2026, 10, 19, 0, 0))]), &EventOptions::default());
    assert!(server.request(1).starts_with("DELETE /v1.0/me/events/d-19 HTTP/1.1\r\n"), "{}", server.request(1));

    // The series is gone, the sign-in is over, Graph is busy: each is what it is, and nothing is written.
    for (status, result, refused, tried) in [
        ("404 Not Found", Some(EventResult::Gone), false, 2),
        ("403 Forbidden", Some(EventResult::Denied), false, 2),
        ("400 Bad Request", Some(EventResult::Refused), false, 2),
        ("503 Service Unavailable", Some(EventResult::Failed), false, 1),
        ("401 Unauthorized", None, true, 1),
    ] {
        let server = FakeHttp::answering(answer(status, "", "{}"));
        let ops = vec![one(EventChangeKind::Delete, at(2026, 10, 19, 13, 0)), one(EventChangeKind::Delete, at(2026, 10, 18, 13, 0))];
        let uploaded = push_events_at(&server.url(""), TOKEN, &upload(ops), &EventOptions::default());
        assert_eq!((uploaded.outcomes.first().map(|o| o.result), uploaded.token_refused, server.asked()), (result, refused, tried), "{status}");
        assert!((0..server.asked()).all(|n| server.request(n).starts_with("GET ")), "{status}");
    }
}

#[test]
fn meetings_series_and_answers_are_written_as_graph_takes_them() {
    let server = FakeHttp::start(|n, _| match n {
        0 | 1 => json_answer("201 Created", r#"{"id":"made","changeKey":"ck"}"#),
        2 | 5 => json_answer("200 OK", r#"{"id":"e-1","changeKey":"ck"}"#),
        _ => ACCEPTED.to_vec(),
    });
    let day = Date::new(2026, 10, 12).unwrap();
    let clock = |hour: u8| DateTime::new(day, hour, 0, 0).unwrap();
    let invited = vec![
        Attendee { address: Address::new("Bo Marsh", "bo@example.org"), ..Default::default() },
        Attendee { address: Address::new("", "cy@example.org"), optional: true, response: Response::Accepted },
        // Nobody is invited twice, and somebody without an address can't be.
        Attendee { address: Address::new("Bo again", "BO@example.org"), ..Default::default() },
        Attendee { address: Address::new("Room 4", ""), ..Default::default() },
    ];
    let meeting = EventFields {
        title: Some("Sync".into()),
        time: Some(EventTime { start: at(2026, 10, 12, 16, 0), end: at(2026, 10, 12, 17, 0), all_day: false }),
        wall: Some(EventWall { start: clock(9), end: clock(10) }),
        repeat: Some(EventRepeat { rule: Some(Recurrence { freq: Freq::Weekly, interval: 1, ..Default::default() }), first: day }),
        attendees: Some(invited),
        categories: Some(vec!["Team".into(), " team ".into(), "Red category".into()]),
        ..Default::default()
    };
    let respond = |response: Response, send: bool| EventOp {
        kind: EventChangeKind::Respond,
        event: 9,
        remote: "e-9".into(),
        response,
        send,
        comment: "Running late".into(),
        ..Default::default()
    };
    let ops = vec![
        create(meeting.clone()),
        EventOp { calendar: "cal/2=".into(), ..create(EventFields { title: Some("Elsewhere".into()), ..Default::default() }) },
        update("e-1", EventFields { repeat: Some(EventRepeat { rule: None, first: day }), attendees: Some(Vec::new()), ..Default::default() }),
        EventOp { cancel: true, comment: "Sorry,\nsomething came up.".into(), ..delete("e-2") },
        respond(Response::Accepted, true),
        update("e-1", EventFields { categories: Some(Vec::new()), ..Default::default() }),
        respond(Response::Tentative, true),
        respond(Response::Declined, false),
        respond(Response::None, true),
    ];
    let options = EventOptions { zone: "Pacific Standard Time".into(), ..Default::default() };
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(ops), &options);
    assert_eq!(server.asked(), 8);
    // A series with people invited: by the clock in the owner's zone, with its rule and who is asked.
    assert_eq!(
        body_of(&server.request(0)),
        json!({
            "subject": "Sync",
            "start": {"dateTime": "2026-10-12T09:00:00", "timeZone": "Pacific Standard Time"},
            "end": {"dateTime": "2026-10-12T10:00:00", "timeZone": "Pacific Standard Time"},
            "isAllDay": false,
            "recurrence": {
                "pattern": {"type": "weekly", "interval": 1, "daysOfWeek": ["monday"], "firstDayOfWeek": "monday"},
                "range": {"type": "noEnd", "startDate": "2026-10-12", "recurrenceTimeZone": "Pacific Standard Time"},
            },
            "attendees": [
                {"emailAddress": {"address": "bo@example.org", "name": "Bo Marsh"}, "type": "required"},
                {"emailAddress": {"address": "cy@example.org", "name": ""}, "type": "optional"},
            ],
            "categories": ["Team", "Red category"],
            "transactionId": MARK,
        })
    );
    // In another calendar of the account.
    assert!(server.request(1).starts_with("POST /v1.0/me/calendars/cal%2F2%3D/events HTTP/1.1\r\n"), "{}", server.request(1));
    // No longer a series, and nobody invited any more.
    assert_eq!(body_of(&server.request(2)), json!({"recurrence": null, "attendees": []}));
    // Called off, with the owner's note; answered, with or without telling the organizer.
    assert!(server.request(3).starts_with("POST /v1.0/me/events/e-2/cancel HTTP/1.1\r\n"), "{}", server.request(3));
    assert_eq!(body_of(&server.request(3)), json!({"comment": "Sorry,\nsomething came up."}));
    assert!(server.request(4).starts_with("POST /v1.0/me/events/e-9/accept HTTP/1.1\r\n"), "{}", server.request(4));
    assert_eq!(body_of(&server.request(4)), json!({"comment": "Running late", "sendResponse": true}));
    assert_eq!(body_of(&server.request(5)), json!({"categories": []}));
    assert!(server.request(6).starts_with("POST /v1.0/me/events/e-9/tentativelyAccept HTTP/1.1\r\n"), "{}", server.request(6));
    assert!(server.request(7).starts_with("POST /v1.0/me/events/e-9/decline HTTP/1.1\r\n"), "{}", server.request(7));
    assert_eq!(body_of(&server.request(7)), json!({"comment": "", "sendResponse": false}));
    let said: Vec<_> = uploaded.outcomes.iter().map(|o| o.result).collect();
    assert_eq!(said[..8], [EventResult::Done; 8]);
    // An answer that is none is no question Graph can be asked.
    assert_eq!((said[8], uploaded.outcomes[8].error.as_str()), (EventResult::Refused, "the change doesn't say which event it is about"));

    // A rule Graph has no way of saying is refused before anything is sent; so nobody is written to.
    let odd = EventRepeat { rule: Some(Recurrence { freq: Freq::Monthly, interval: 1, nth: Some(5), ..Default::default() }), first: day };
    let server = FakeHttp::answering(json_answer("201 Created", r#"{"id":"made"}"#));
    let uploaded = push_events_at(&server.url(""), TOKEN, &upload(vec![create(EventFields { repeat: Some(odd), ..meeting.clone() })]), &options);
    assert_eq!((server.asked(), uploaded.outcomes[0].result), (0, EventResult::Refused));
    assert_eq!(uploaded.outcomes[0].error, "it repeats in a way Microsoft 365 has no way of keeping");
    // A zone Graph doesn't know: once more by the instant, in UTC.
    let server = FakeHttp::start(|n, _| match n {
        0 => answer("400 Bad Request", "", &graph_error("TimeZoneNotSupportedException", "A valid TimeZone value must be specified.")),
        _ => json_answer("201 Created", r#"{"id":"made"}"#),
    });
    let uploaded =
        push_events_at(&server.url(""), TOKEN, &upload(vec![create(meeting)]), &EventOptions { zone: "Nowhere/Land".into(), ..Default::default() });
    assert_eq!((server.asked(), uploaded.outcomes[0].result), (2, EventResult::Done));
    let again = body_of(&server.request(1));
    assert_eq!(again["start"], json!({"dateTime": "2026-10-12T16:00:00", "timeZone": "UTC"}));
    assert_eq!(again["recurrence"]["range"]["recurrenceTimeZone"], "UTC");
}

#[test]
fn the_accounts_calendars_come_with_their_groups_and_colours() {
    let server = FakeHttp::start(|n, request| match n {
        0 => {
            page(vec![json!({"id": "g/1=", "name": "My Calendars"}), json!({"id": "g-2", "name": "Other Calendars"}), json!({"name": "No id"})], None)
        }
        1 => {
            assert!(
                request.starts_with(
                    "GET /v1.0/me/calendarGroups/g%2F1%3D/calendars?%24top=50&%24select=id%2Cname%2Ccolor%2ChexColor%2CcanEdit%2CcanViewPrivateItems%2Cowner%2CisDefaultCalendar%2CallowedOnlineMeetingProviders "
                ),
                "{request}"
            );
            page(
                vec![
                    json!({"id": "c-main", "name": "Calendar", "color": "auto", "hexColor": "", "canEdit": true, "isDefaultCalendar": true,
                        "allowedOnlineMeetingProviders": ["teamsForBusiness", "skypeForBusiness", "somethingNew", 7]}),
                    json!({"id": "c-team", "name": "Team\r\nplans", "color": "lightGreen", "hexColor": "#2e8b57", "canEdit": true, "isDefaultCalendar": false,
                        "owner": {"name": "Bo\r\nExample", "address": "bo@example.org"}}),
                    json!({"id": "c-pink", "name": "Pink", "color": "lightPink", "hexColor": "", "canEdit": true}),
                ],
                None,
            )
        }
        _ => page(
            vec![
                json!({"id": "c-holidays", "name": "Holidays", "color": "auto", "canEdit": false, "isDefaultCalendar": true, "owner": "nobody"}),
                json!({"id": "c-team", "name": "Said twice"}),
                json!({"name": "No id"}),
                json!("noise"),
            ],
            None,
        ),
    });
    let download = calendars_at(&server.url(""), TOKEN, None).unwrap();
    assert_eq!((server.asked(), download.complete, download.account), (3, true, 0));
    assert!(server.request(0).starts_with("GET /v1.0/me/calendarGroups?%24top=50&%24select=id%2Cname HTTP/1.1\r\n"), "{}", server.request(0));
    assert!(server.request(0).contains(&format!("Authorization: Bearer {TOKEN}\r\n")));
    let read: Vec<_> =
        download.calendars.iter().map(|c| (c.id.as_str(), c.name.as_str(), c.group.as_str(), c.color, !c.rights.can_edit(), c.main)).collect();
    assert_eq!(
        read,
        [
            ("c-main", "Calendar", "My Calendars", None, false, true),
            // The colour as Graph gives it; by its name in our own shade when it gives none.
            ("c-team", "Team  plans", "My Calendars", Some(Rgb(0x2E, 0x8B, 0x57)), false, false),
            ("c-pink", "Pink", "My Calendars", Some(Rgb(0xD9, 0x6C, 0xA6)), false, false),
            // Read only; and only one calendar is the main one, whatever is said.
            ("c-holidays", "Holidays", "Other Calendars", None, true, false),
        ]
    );
    // What a calendar's meetings may be held online with, when Graph says; nothing when it doesn't.
    use mailcraft_model::OnlineProvider::{SkypeForBusiness, Teams};
    assert_eq!(download.calendars[0].online_providers, Some(vec![Teams, SkypeForBusiness]));
    assert_eq!(download.calendars[1].online_providers, None);
    // Whose each is, as Graph says: one line, bounded; nothing when it isn't a name and an address.
    let owners: Vec<_> = download.calendars.iter().map(|c| c.owner.as_ref().map(|o| (o.address.as_str(), o.name.as_str()))).collect();
    assert_eq!(owners, [None, Some(("bo@example.org", "Bo  Example")), None, None]);
    // Should Graph not have the question with that in it (400), the calendars come without.
    let server = FakeHttp::start(|n, request| match n {
        0 => page(vec![json!({"id": "g-1", "name": "My Calendars"})], None),
        1 => answer("400 Bad Request", "", &graph_error("BadRequest", "Could not find a property named 'allowedOnlineMeetingProviders'.")),
        _ => {
            assert!(
                request.contains("%24select=id%2Cname%2Ccolor%2ChexColor%2CcanEdit%2CcanViewPrivateItems%2Cowner%2CisDefaultCalendar "),
                "{request}"
            );
            page(vec![json!({"id": "c-main", "name": "Calendar", "isDefaultCalendar": true})], None)
        }
    });
    let download = calendars_at(&server.url(""), TOKEN, None).unwrap();
    assert_eq!((server.asked(), download.calendars.len(), download.complete, &download.calendars[0].online_providers), (3, 1, true, &None));

    // No groups to be had: the calendars as one list. Nothing at all: no reason to take every
    // calendar away.
    let server = FakeHttp::start(|n, _| match n {
        0 => answer("400 Bad Request", "", &graph_error("BadRequest", "no groups here")),
        _ => page(vec![json!({"id": "c-main", "name": "Calendar", "isDefaultCalendar": true})], None),
    });
    let download = calendars_at(&server.url(""), TOKEN, None).unwrap();
    assert!(server.request(1).starts_with("GET /v1.0/me/calendars?%24top=50&"), "{}", server.request(1));
    assert_eq!((download.calendars.len(), download.calendars[0].group.as_str(), download.complete), (1, "", true));
    let server = FakeHttp::answering(page(Vec::new(), None));
    let download = calendars_at(&server.url(""), TOKEN, None).unwrap();
    assert_eq!((download.calendars.len(), download.complete), (0, false));
    // A sign-in that is no longer taken is that, and nothing more is asked.
    let server = FakeHttp::answering(answer("401 Unauthorized", "", "{}"));
    let refused = calendars_at(&server.url(""), TOKEN, None).unwrap_err();
    assert!(refused.is_refused_sign_in() && server.asked() == 1, "{refused:?}");
    assert_no_token(&refused);
}

#[test]
fn the_mailboxs_categories_come_with_a_colour_each_or_are_not_to_be_had() {
    let server = FakeHttp::answering(page(
        vec![
            json!({"id": "1", "displayName": "Red category", "color": "preset0"}),
            json!({"id": "2", "displayName": "Project X\r\n", "color": "Preset7"}),
            json!({"id": "3", "displayName": "red CATEGORY", "color": "preset3"}),
            json!({"id": "4", "displayName": "Plain", "color": "none"}),
            json!({"id": "5", "displayName": "Odd", "color": "preset99"}),
            json!({"id": "6", "displayName": " ", "color": "preset1"}),
            json!(7),
        ],
        None,
    ));
    let download = categories_at(&server.url(""), TOKEN).unwrap();
    assert!(server.request(0).starts_with("GET /v1.0/me/outlook/masterCategories HTTP/1.1\r\n"), "{}", server.request(0));
    let read: Vec<_> = download.categories.iter().map(|c| (c.name.as_str(), c.color)).collect();
    let grey = Rgb(0x8A, 0x8A, 0x8A);
    assert_eq!(read, [("Red category", Rgb(0xD1, 0x3B, 0x3B)), ("Project X", Rgb(0x2F, 0x6F, 0xD0)), ("Plain", grey), ("Odd", grey)]);
    // The permission for it is one an older sign-in doesn't have: that is said as what it is,
    // for the host to note and go on.
    let server =
        FakeHttp::answering(answer("403 Forbidden", "", &graph_error("ErrorAccessDenied", "Access is denied. Check credentials and try again.")));
    let refused = categories_at(&server.url(""), TOKEN).unwrap_err();
    assert_eq!(
        refused.to_string(),
        NetError::Forbidden("this account may not read its list of categories through Microsoft Graph (ErrorAccessDenied)".into()).to_string()
    );
    assert!(matches!(refused, NetError::Forbidden(_)));
    assert_no_token(&refused);
}

#[test]
fn free_and_busy_times_are_asked_for_people_by_their_addresses() {
    let item = |status: &str, start: &str, end: &str| json!({"status": status, "subject": "Not read", "location": "Nor this", "start": {"dateTime": start, "timeZone": "UTC"}, "end": {"dateTime": end, "timeZone": "UTC"}});
    let server = FakeHttp::answering(json_answer(
        "200 OK",
        &json!({"value": [
            {"scheduleId": "BO@example.org", "availabilityView": "0022", "scheduleItems": [
                item("busy", "2026-10-15T13:00:00.0000000", "2026-10-15T14:00:00.0000000"),
                item("oof", "2026-10-15T15:00:00.0000000", "2026-10-15T16:00:00.0000000"),
                item("unknown", "2026-10-15T16:00:00.0000000", "2026-10-15T17:00:00.0000000"),
                item("tentative", "2026-10-15T18:00:00.0000000", "2026-10-15T18:00:00.0000000"),
                json!({"status": "busy", "start": {"dateTime": "2026-10-15T13:00:00", "timeZone": "Pacific Standard Time"}, "end": {"dateTime": "soon"}}),
            ]},
            {"scheduleId": "cy@example.org", "error": {"message": "Unable to resolve the mailbox for cy@example.org", "responseCode": "ErrorMailRecipientNotFound"}},
            {"scheduleId": "eve@example.org", "scheduleItems": [item("busy", "2026-10-15T13:00:00", "2026-10-15T14:00:00")]},
            "noise",
        ]})
        .to_string(),
    ));
    let ask = FreeBusyAsk {
        people: vec!["bo@example.org".into(), " cy@example.org ".into(), "not an address".into(), "Bo@Example.org".into()],
        from: at(2026, 10, 15, 12, 0),
        to: at(2026, 10, 15, 22, 0),
    };
    let answer_ = schedule_at(&server.url(""), TOKEN, &ask).unwrap();
    let request = server.request(0);
    assert!(
        request.starts_with("POST /v1.0/me/calendar/getSchedule HTTP/1.1\r\n") && request.contains("Prefer: outlook.timezone=\"UTC\"\r\n"),
        "{request}"
    );
    assert_eq!(
        body_of(&request),
        json!({
            "schedules": ["bo@example.org", "cy@example.org"],
            "startTime": {"dateTime": "2026-10-15T12:00:00", "timeZone": "UTC"},
            "endTime": {"dateTime": "2026-10-15T22:00:00", "timeZone": "UTC"},
            "availabilityViewInterval": 30,
        })
    );
    assert_eq!((answer_.from, answer_.to, answer_.people.len()), (ask.from, ask.to, 2));
    // When, and how it shows: nothing of what the time is taken by. Under the address that was asked about.
    assert_eq!(answer_.people[0].email, "bo@example.org");
    assert_eq!(
        answer_.people[0].busy,
        [
            BusyTime { start: at(2026, 10, 15, 13, 0), end: at(2026, 10, 15, 14, 0), show_as: ShowAs::Busy },
            BusyTime { start: at(2026, 10, 15, 15, 0), end: at(2026, 10, 15, 16, 0), show_as: ShowAs::OutOfOffice },
        ]
    );
    assert!(!format!("{answer_:?}").contains("Not read"));
    // Somebody Graph can't look up says so, by the name of the refusal only; and somebody who
    // wasn't asked about is not in the answer.
    assert_eq!(
        (answer_.people[1].email.as_str(), answer_.people[1].error.as_str()),
        ("cy@example.org", "their time couldn't be looked up (ErrorMailRecipientNotFound)")
    );
    assert!(answer_.people[1].busy.is_empty());

    // What can't be asked is refused before Graph hears of it.
    for odd in [
        FreeBusyAsk { people: vec!["nobody".into()], ..ask.clone() },
        FreeBusyAsk { to: ask.from, ..ask.clone() },
        FreeBusyAsk { to: ask.from + 63 * 86_400, ..ask.clone() },
    ] {
        let quiet = FakeHttp::answering(json_answer("200 OK", r#"{"value":[]}"#));
        assert!(matches!(schedule_at(&quiet.url(""), TOKEN, &odd), Err(NetError::Config(_))), "{odd:?}");
        assert_eq!(quiet.asked(), 0);
    }
    // And what Graph says no to is what it is.
    let server = FakeHttp::answering(answer("403 Forbidden", "", &graph_error("ErrorAccessDenied", "Access is denied.")));
    let refused = schedule_at(&server.url(""), TOKEN, &ask).unwrap_err();
    assert!(matches!(refused, NetError::Forbidden(_)));
    assert_no_token(&refused);
}

proptest! {
    #[test]
    fn arbitrary_series_calendars_and_schedules_never_panic(entry in any_json(), noise in any_json()) {
        // A rule from outside is one the mailbox can say, or none; and what is written of it reads back.
        if let Some((rule, first)) = rule_of(&entry) {
            prop_assert!((1..=999).contains(&rule.interval));
            if let Some(written) = recurrence_body(&rule, first, "UTC", 0) {
                prop_assert_eq!(rule_of(&written), Some((rule, first)));
            }
        }
        let _ = calendar_of(&entry, "Group");
        let page = json!({"value": [entry.clone(), noise.clone(), {"scheduleId": "bo@example.org", "scheduleItems": [entry.clone(), noise.clone()], "error": noise.clone()}]});
        let batch = json!({"responses": [{"id": "0", "status": 200, "body": entry.clone()}, {"id": noise.clone(), "status": noise.clone(), "body": noise.clone()}, noise.clone()]});
        let (page, batch) = (page.to_string(), batch.to_string());
        let server = FakeHttp::start(move |_, request| json_answer("200 OK", if request.contains("$batch") { &batch } else { &page }));
        let (from, to) = october();
        if let Ok(download) = events_in_at(&server.url(""), TOKEN, &EventsQuery { from, to, series: vec!["s-1".into()], ..Default::default() }) {
            prop_assert!(download.events.iter().all(|event| !event.id.is_empty() && event.categories.len() <= 50));
        }
        if let Ok(list) = calendars_at(&server.url(""), TOKEN, None) {
            prop_assert!(list.calendars.iter().all(|calendar| !calendar.id.is_empty()));
        }
        let _ = categories_at(&server.url(""), TOKEN);
        let ask = FreeBusyAsk { people: vec!["bo@example.org".into()], from, to };
        if let Ok(answer) = schedule_at(&server.url(""), TOKEN, &ask) {
            prop_assert!(answer.people.iter().all(|person| person.busy.iter().all(|time| time.end > time.start)));
        }
        let one = EventOp { kind: EventChangeKind::Delete, series: "s-1".into(), occurrence: from, ..Default::default() };
        let _ = push_events_at(&server.url(""), TOKEN, &upload(vec![one]), &EventOptions::default());
    }
}

// ---- time zones ----

/// The zones of an event as an upload names them.
fn zones(start: &str, end: &str) -> Option<mailcraft_model::remote::EventZones> {
    Some(mailcraft_model::remote::EventZones { start: start.into(), end: end.into() })
}

#[test]
fn the_zones_an_event_was_made_in_are_read() {
    let read = |more: Value| {
        let mut entry = event("e-1", "Review");
        entry.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
        event_of(&entry).unwrap()
    };
    // Asked for in UTC, the times are instants; where the event was made comes beside them, by
    // whatever name Graph has for the zone.
    let made = read(json!({"originalStartTimeZone": "Pacific Standard Time", "originalEndTimeZone": "Asia/Tokyo"}));
    assert_eq!((made.start_zone.as_str(), made.end_zone.as_str()), ("Pacific Standard Time", "Asia/Tokyo"));
    assert_eq!(read(json!({})).start_zone, "");
    // A series counts by the zone of its rule, whatever its start says.
    let mut master = series("s-1", "Stand-up", weekly());
    master["originalStartTimeZone"] = json!("UTC");
    let master = event_of(&master).unwrap();
    assert_eq!((master.kind, master.start_zone.as_str()), (RemoteEventKind::SeriesMaster, "Eastern Standard Time"));
    // Names that are none, and names of any length, are names all the same here: what they
    // mean is for the mailbox to say, which keeps a zone only when it is one.
    for odd in
        [json!("tzone://Microsoft/Custom"), json!("Customized Time Zone"), json!(7), json!(null), json!("x".repeat(1 << 20)), json!("Line\r\nbreak")]
    {
        let event = read(json!({"originalStartTimeZone": odd}));
        assert!(event.start_zone.len() <= 64 && !event.start_zone.contains('\n'), "{:?}", event.start_zone);
        assert_eq!(event.start, at(2026, 10, 12, 13, 0));
    }
}

#[test]
fn an_event_with_zones_of_its_own_is_written_by_their_clocks() {
    let server = FakeHttp::answering(json_answer("200 OK", r#"{"id":"e-1","changeKey":"ck"}"#));
    let sent = |fields: EventFields, shown: &str| {
        let before = server.asked();
        let upload = EventsUpload { zone: shown.into(), ..upload(vec![update("e-1", fields)]) };
        let uploaded = push_events_at(&server.url(""), TOKEN, &upload, &EventOptions { zone: "Hawaiian Standard Time".into(), ..Default::default() });
        (uploaded.outcomes[0].result, (server.asked() > before).then(|| body_of(&server.request(before))))
    };
    let stamp = |text: &str, zone: &str| json!({"dateTime": text, "timeZone": zone});
    // 20:00 UTC on October 12 is one in the afternoon in Los Angeles and five the next morning
    // in Tokyo: said by those clocks, under the names Graph is sure to know.
    let time = Some(EventTime { start: at(2026, 10, 12, 20, 0), end: at(2026, 10, 12, 21, 0), all_day: false });
    let (result, body) = sent(EventFields { time, zones: zones("America/Los_Angeles", "Asia/Tokyo"), ..Default::default() }, "America/Chicago");
    assert_eq!(result, EventResult::Done);
    assert_eq!(
        body,
        Some(
            json!({"start": stamp("2026-10-12T13:00:00", "Pacific Standard Time"), "end": stamp("2026-10-13T06:00:00", "Tokyo Standard Time"), "isAllDay": false})
        )
    );
    // Half and three quarters of an hour, the south in its summer, a zone Windows has no name for.
    for (zone, name, start) in [
        ("Asia/Kolkata", "India Standard Time", "2026-10-13T01:30:00"),
        ("Asia/Kathmandu", "Nepal Standard Time", "2026-10-13T01:45:00"),
        ("Australia/Sydney", "AUS Eastern Standard Time", "2026-10-13T07:00:00"),
        ("America/Detroit", "America/Detroit", "2026-10-12T16:00:00"),
        ("UTC", "UTC", "2026-10-12T20:00:00"),
    ] {
        let (_, body) = sent(EventFields { time, zones: zones(zone, ""), ..Default::default() }, "");
        assert_eq!(body.unwrap()["start"], stamp(start, name), "{zone}");
    }
    // An event without zones of its own goes as instants, as it always did; whole days are
    // days in the zone the calendar is shown in, which the upload names (and only when it
    // doesn't, in the one the host knows the computer to be in).
    let (_, body) = sent(EventFields { time, ..Default::default() }, "America/Chicago");
    assert_eq!(body.unwrap()["start"], stamp("2026-10-12T20:00:00", "UTC"));
    let days = Some(EventTime { start: at(2026, 10, 31, 0, 0), end: at(2026, 11, 1, 0, 0), all_day: true });
    let (_, body) = sent(EventFields { time: days, zones: zones("Asia/Tokyo", ""), ..Default::default() }, "America/Chicago");
    assert_eq!(body.unwrap()["start"], stamp("2026-10-31T00:00:00", "Central Standard Time"));
    for unnamed in ["", "UTC+05:30", "Nowhere/At_All"] {
        let (_, body) = sent(EventFields { time: days, ..Default::default() }, unnamed);
        assert_eq!(body.unwrap()["start"], stamp("2026-10-31T00:00:00", "Hawaiian Standard Time"), "{unnamed}");
    }

    // A series in a zone of its own: by that zone's clock, and its rule counts that zone's days.
    let rule = Recurrence { freq: Freq::Weekly, interval: 1, by_day: vec![Weekday::Mon], ..Default::default() };
    let series = EventFields {
        time: Some(EventTime { start: at(2026, 10, 5, 14, 0), end: at(2026, 10, 5, 15, 0), all_day: false }),
        wall: Some(EventWall { start: clock(2026, 10, 5, 16, 0), end: clock(2026, 10, 5, 17, 0) }),
        zones: zones("Europe/Berlin", "Europe/Berlin"),
        repeat: Some(EventRepeat { rule: Some(rule.clone()), first: Date::new(2026, 10, 5).unwrap() }),
        ..Default::default()
    };
    let (_, body) = sent(series.clone(), "America/Chicago");
    let body = body.unwrap();
    assert_eq!(
        (body["start"].clone(), body["end"].clone()),
        (stamp("2026-10-05T16:00:00", "W. Europe Standard Time"), stamp("2026-10-05T17:00:00", "W. Europe Standard Time"))
    );
    assert_eq!(body["recurrence"]["range"], json!({"type": "noEnd", "startDate": "2026-10-05", "recurrenceTimeZone": "W. Europe Standard Time"}));
    // The same series without a zone of its own is by the clock of the zone the calendar is
    // shown in.
    let (_, body) = sent(
        EventFields { zones: None, wall: Some(EventWall { start: clock(2026, 10, 5, 9, 0), end: clock(2026, 10, 5, 10, 0) }), ..series.clone() },
        "America/Chicago",
    );
    let body = body.unwrap();
    assert_eq!(
        (body["start"].clone(), body["recurrence"]["range"]["recurrenceTimeZone"].clone()),
        (stamp("2026-10-05T09:00:00", "Central Standard Time"), json!("Central Standard Time"))
    );
    // A series by the clock of a zone Graph has no name for (an offset, a name that is none) is
    // not written by another clock: it is refused, and Graph isn't asked.
    for unnamed in ["UTC+05:30", "Nowhere/At_All", "x\r\ny"] {
        let (result, body) = sent(EventFields { zones: zones(unnamed, ""), ..series.clone() }, "America/Chicago");
        assert_eq!((result, body), (EventResult::Refused, None), "{unnamed}");
    }
    // An event that happens once in such a zone is the same moment in UTC.
    let (result, body) = sent(EventFields { time, zones: zones("UTC+05:30", ""), ..Default::default() }, "America/Chicago");
    assert_eq!((result, body.unwrap()["start"].clone()), (EventResult::Done, stamp("2026-10-12T20:00:00", "UTC")));

    // Graph won't have a zone (400): an event that happens once is sent again as instants; a
    // series is not sent by another clock, and says so.
    let picky = FakeHttp::start(|_, request| {
        if request.contains("Standard Time") {
            answer("400 Bad Request", "", &graph_error("ErrorInvalidRequest", "Unknown time zone."))
        } else {
            json_answer("200 OK", r#"{"id":"e-1","changeKey":"ck"}"#)
        }
    });
    let once = update("e-1", EventFields { time, zones: zones("America/Los_Angeles", ""), ..Default::default() });
    let uploaded = push_events_at(&picky.url(""), TOKEN, &upload(vec![once]), &EventOptions::default());
    assert_eq!((uploaded.outcomes[0].result, picky.asked()), (EventResult::Done, 2));
    assert_eq!(body_of(&picky.request(1))["start"], stamp("2026-10-12T20:00:00", "UTC"));
    let uploaded = push_events_at(&picky.url(""), TOKEN, &upload(vec![update("s-1", series)]), &EventOptions::default());
    assert_eq!((uploaded.outcomes[0].result, picky.asked()), (EventResult::Refused, 3));
    assert_eq!(uploaded.outcomes[0].error, "it repeats in a way Microsoft 365 has no way of keeping");
}

fn clock(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> DateTime {
    DateTime::new(Date::new(year, month, day).unwrap(), hour, minute, 0).unwrap()
}

proptest! {
    /// What is written for an event in a zone is read back by Graph's own rule (a clock time
    /// and the name of its zone) as the instants it was written for, whatever the zone.
    #[test]
    fn a_time_written_in_any_zone_reads_back_as_its_instant(index in 0usize..200, start in 0i64..4_000_000_000, minutes in 0i64..5_000) {
        let choices = mailcraft_time::Zone::choices();
        let zone = choices[index % choices.len()];
        // (A time in the second of an hour that comes twice has no clock time of its own.)
        let end = start + minutes * 60;
        prop_assume!(zone.instant(zone.wall(start)) == start && zone.instant(zone.wall(end)) == end);
        let fields = EventFields { time: Some(EventTime { start, end, all_day: false }), zones: zones(&zone.id(), ""), ..Default::default() };
        let body: Value = serde_json::from_str(&event_body(&fields, "", "", true).unwrap()).unwrap();
        // The name is one Graph knows, and one that is read back as the same zone.
        let name = body["start"]["timeZone"].as_str().unwrap();
        prop_assert_eq!(mailcraft_time::Zone::named(name), Some(zone));
        prop_assert_eq!((when(body.get("start")), when(body.get("end"))), (Some(start), Some(end)));
        // And as instants, when Graph wouldn't have the zone.
        let plain: Value = serde_json::from_str(&event_body(&fields, "", "", false).unwrap()).unwrap();
        prop_assert_eq!((when(plain.get("start")), when(plain.get("end")), plain["start"]["timeZone"].as_str()), (Some(start), Some(end), Some("UTC")));
    }
}

/// Graph's requests go on connections kept open (#276): reads one after another on one, and
/// what would happen twice on a new one of its own, with the token on every request.
#[test]
fn graph_requests_reuse_their_connection() {
    use crate::fake::{FakeKeptHttp, Kept, kept_answer};

    let server = FakeKeptHttp::start(|_, _, _| Kept::Answer(kept_answer("200 OK", r#"{"value":[]}"#)));
    let graph = Graph::at(&server.url(""), TOKEN).unwrap();
    for _ in 0..4 {
        assert_eq!(graph.ask(&graph.url(CONTACTS), "GET", &[], None, ASK_TIMEOUT, MAX_PAGE).unwrap().status, 200);
    }
    assert_eq!((server.connections(), server.asked()), (1, 4));
    graph.ask(&graph.url(SEND_MAIL), "POST", &[("Content-Type", "text/plain")], Some("aGk="), SEND_TIMEOUT, MAX_PAGE).unwrap();
    assert_eq!(server.connections(), 2, "a send has a connection of its own");
    // By the ground rules of mail a $batch of reads may go on one that waited; a move may not.
    let read = graph.call(pace::Kind::Read, &graph.url("/v1.0/$batch"), "POST", &[], Some("{}"), ASK_TIMEOUT, MAX_PAGE).unwrap();
    assert_eq!((read.status, server.connections()), (200, 2));
    graph.call(pace::Kind::Once, &graph.url("/v1.0/me/messages/AAMk/move"), "POST", &[], Some("{}"), ASK_TIMEOUT, MAX_PAGE).unwrap();
    assert_eq!(server.connections(), 3);
    assert_eq!(graph.requests_asked(), 7);
    for n in 0..7 {
        assert!(server.request(n).contains(&format!("Authorization: Bearer {TOKEN}\r\n")), "{}", server.request(n));
    }
}
mod meetings;
