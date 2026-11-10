// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The desktop's JMAP host over real sockets: the crate's JMAP server in the process
//! (`mailcraft_jmap::standin`) behind a small HTTP server on this computer's loopback, reached
//! through [`Transport`] and `mailcraft_net::https` as a real server would be. Nothing here leaves
//! this computer; every name, address and message is invented.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use mailcraft_jmap::standin::{PASSWORD, SESSION, StandIn, TOKEN, USER};
use mailcraft_jmap::{RetryAfter, UnixTime};
use mailcraft_model::Id;
use mailcraft_model::sync::{JmapFetchRequest, JmapMailRequest, JmapMailResult, JmapScope};
use mailcraft_net::NetError;

use super::*;

const ACCOUNT: Id = 7;

/// A message as a mail program writes one.
fn mail(subject: &str, n: usize) -> Vec<u8> {
    format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nMessage-ID: <{n}@example.org>\r\nDate: Thu, 15 Oct 2026 09:{n:02}:00 +0200\r\n\r\nHello Ada, this is about {subject}.\r\n"
    )
    .into_bytes()
}

/// The stand-in served on a loopback port of its own, a connection at a time, until the test ends:
/// its session's address, and how many requests it took.
struct Served {
    session: String,
    requests: Arc<AtomicUsize>,
}

fn serve(server: &StandIn) -> Served {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let (server, counted) = (server.clone(), requests.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::Relaxed);
            let server = server.clone();
            std::thread::spawn(move || {
                let _ = answer(&server, stream);
            });
        }
    });
    Served { session: format!("http://127.0.0.1:{port}/jmap/session"), requests }
}

/// One request on `stream`, answered by the stand-in, and the connection closed.
fn answer(server: &StandIn, stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or("/").to_string());
    let mut fields: Vec<(String, String)> = Vec::new();
    loop {
        let mut field = String::new();
        if reader.read_line(&mut field)? == 0 || field.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = field.split_once(':') {
            fields.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let length = fields.iter().find(|(name, _)| name.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0usize);
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    // The stand-in knows itself by its own name; the path is what it reads.
    let origin = SESSION.trim_end_matches("/jmap/session");
    let named: Vec<(&str, &str)> = fields.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
    let mut stream = stream;
    let Ok(answer) = server.answer(&method, &format!("{origin}{target}"), &named, &body) else {
        return write!(stream, "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    };
    let out = answer.body();
    let mut head = format!("HTTP/1.1 {} Answer\r\nContent-Length: {}\r\nConnection: close\r\n", answer.status(), out.len());
    if let Some(kind) = answer.content_type() {
        head.push_str(&format!("Content-Type: {kind}\r\n"));
    }
    if let Some(after) = answer.retry_after() {
        head.push_str(&format!("Retry-After: {after}\r\n"));
    }
    stream.write_all(head.as_bytes())?;
    stream.write_all(b"\r\n")?;
    stream.write_all(out)
}

/// A stand-in with an Inbox, a Sent folder and a folder inside another, and three messages: the
/// server, and the ids of the Inbox's two.
fn filled() -> (StandIn, Vec<String>) {
    let server = StandIn::new();
    let ids = server.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        d.mailbox("Sent", None, Some("sent"));
        let projects = d.mailbox("Projects", None, None);
        let apollo = d.mailbox("Apollo", Some(&projects), None);
        d.deliver(&apollo, &mail("Launch plan", 9), &["$flagged"], "2026-10-15T07:09:00Z");
        vec![
            d.deliver(&inbox, &mail("Kickoff", 1), &["$seen"], "2026-10-15T07:01:00Z"),
            d.deliver(&inbox, &mail("Budget", 2), &[], "2026-10-15T07:02:00Z"),
        ]
    });
    (server, ids)
}

fn run_request(session: &str, email: &str, bearer: bool) -> JmapMailRequest {
    JmapMailRequest {
        account: ACCOUNT,
        email: email.into(),
        session: session.into(),
        bearer,
        window: 50,
        list_folders: true,
        scope: JmapScope::All,
        ..Default::default()
    }
}

/// Every piece of a run with `secret`, the last one last.
fn run(request: &JmapMailRequest, secret: &str, cancel: &Arc<AtomicBool>) -> Vec<JmapMailResult> {
    let mut pieces = Vec::new();
    run_mail_with(request, secret, &Stop::new(cancel.clone()), &mut |piece| pieces.push(piece));
    pieces
}

#[test]
fn a_run_reads_the_folders_and_the_newest_mail_over_http() {
    let (server, _) = filled();
    let served = serve(&server);
    let pieces = run(&run_request(&served.session, "ada@example.org", true), TOKEN, &Arc::new(AtomicBool::new(false)));
    let last = pieces.last().unwrap();
    assert!(!last.partial && last.whole && !last.stopped && !last.auth_refused, "a clean run: {:?}", last.errors);
    assert!(last.errors.is_empty(), "{:?}", last.errors);
    assert!(pieces[..pieces.len() - 1].iter().all(|piece| piece.partial), "only the last piece ends the run");
    let listing = pieces.iter().find_map(|piece| piece.listing.as_ref()).expect("the folders came");
    let names: Vec<&str> = listing.folders.iter().map(|folder| folder.name.as_str()).collect();
    for name in ["Inbox", "Sent", "Projects", "Apollo"] {
        assert!(names.contains(&name), "{name} is listed: {names:?}");
    }
    let subjects: Vec<&str> = pieces.iter().flat_map(|piece| &piece.pages).flat_map(|page| &page.messages).map(|m| m.subject.as_str()).collect();
    for subject in ["Kickoff", "Budget", "Launch plan"] {
        assert!(subjects.contains(&subject), "{subject} came down: {subjects:?}");
    }
    // Every piece carries whose run it is.
    assert!(pieces.iter().all(|piece| piece.account == ACCOUNT && piece.session == served.session));
}

#[test]
fn a_password_signs_in_as_the_account_s_address() {
    let (server, _) = filled();
    let served = serve(&server);
    let last = run(&run_request(&served.session, USER, false), PASSWORD, &Arc::new(AtomicBool::new(false))).pop().unwrap();
    assert!(last.whole && !last.auth_refused, "{:?}", last.errors);
}

#[test]
fn a_secret_the_server_refuses_asks_the_owner_again() {
    let (server, _) = filled();
    let served = serve(&server);
    let pieces = run(&run_request(&served.session, "ada@example.org", true), "not-the-token", &Arc::new(AtomicBool::new(false)));
    assert_eq!(pieces.len(), 1);
    assert!(pieces[0].auth_refused && !pieces[0].whole);
    // Nothing of the secret is in what the run says.
    assert!(!pieces[0].errors.iter().any(|line| line.contains("not-the-token")));
}

#[test]
fn no_secret_sends_nothing() {
    let (server, _) = filled();
    let served = serve(&server);
    let pieces = run(&run_request(&served.session, "ada@example.org", true), "", &Arc::new(AtomicBool::new(false)));
    assert_eq!(pieces.len(), 1);
    assert!(pieces[0].auth_refused);
    assert_eq!(served.requests.load(Ordering::Relaxed), 0, "no connection was made");
}

#[test]
fn a_run_called_off_before_it_begins_sends_nothing() {
    let (server, _) = filled();
    let served = serve(&server);
    let pieces = run(&run_request(&served.session, "ada@example.org", true), TOKEN, &Arc::new(AtomicBool::new(true)));
    assert_eq!(pieces.len(), 1);
    assert!(pieces[0].stopped && !pieces[0].partial && pieces[0].errors.is_empty());
    assert_eq!(served.requests.load(Ordering::Relaxed), 0, "no connection was made");
}

#[test]
fn a_server_nobody_answers_at_is_said_and_ends_the_run() {
    // A port that was free a moment ago: nobody listens there.
    let port = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
    let session = format!("http://127.0.0.1:{port}/jmap/session");
    let pieces = run(&run_request(&session, "ada@example.org", true), TOKEN, &Arc::new(AtomicBool::new(false)));
    assert_eq!(pieces.len(), 1);
    let last = &pieces[0];
    assert!(!last.partial && !last.whole && !last.auth_refused && !last.errors.is_empty());
    assert!(!last.errors.iter().any(|line| line.contains(TOKEN)));
}

#[test]
fn plain_http_goes_only_to_this_computer() {
    let pieces = run(&run_request("http://jmap.example/jmap/session", "ada@example.org", true), TOKEN, &Arc::new(AtomicBool::new(false)));
    assert_eq!(pieces.len(), 1);
    assert!(!pieces[0].whole && !pieces[0].errors.is_empty(), "refused before anything was sent");
}

#[test]
fn messages_are_fetched_whole_over_http() {
    let (server, ids) = filled();
    let served = serve(&server);
    let request = JmapFetchRequest {
        account: ACCOUNT,
        email: "ada@example.org".into(),
        session: served.session.clone(),
        bearer: true,
        ids: ids.clone(),
        ..Default::default()
    };
    let fetched = run_fetch_with(&request, TOKEN, &Stop::new(Arc::new(AtomicBool::new(false))));
    assert!(fetched.errors.is_empty() && fetched.failed.is_empty(), "{:?}", fetched.errors);
    assert_eq!(fetched.fetched.len(), 2);
    let budget = fetched.fetched.iter().find(|f| f.id == ids[1]).unwrap();
    assert!(String::from_utf8_lossy(&budget.mime).contains("Subject: Budget"));
}

#[test]
fn a_fetch_without_a_secret_or_called_off_sends_nothing() {
    let (server, ids) = filled();
    let served = serve(&server);
    let request = JmapFetchRequest {
        account: ACCOUNT,
        email: "ada@example.org".into(),
        session: served.session.clone(),
        bearer: true,
        ids,
        ..Default::default()
    };
    let unsigned = run_fetch_with(&request, "", &Stop::new(Arc::new(AtomicBool::new(false))));
    assert!(unsigned.fetched.is_empty() && !unsigned.errors.is_empty());
    let called_off = run_fetch_with(&request, TOKEN, &Stop::new(Arc::new(AtomicBool::new(true))));
    assert!(called_off.fetched.is_empty() && called_off.errors.is_empty());
    assert_eq!(served.requests.load(Ordering::Relaxed), 0);
}

#[test]
fn the_answer_keeps_what_the_client_reads_and_a_date_becomes_seconds() {
    let now = UnixTime::from_secs(1_791_993_600); // Wednesday 2026-10-14 16:00:00 UTC.
    let answer = mailcraft_net::https::Response {
        status: 503,
        body: b"{}".to_vec(),
        content_type: Some("application/problem+json".into()),
        location: Some("/elsewhere".into()),
        retry_after: Some("Wed, 14 Oct 2026 16:02:00 GMT".into()),
        ..Default::default()
    };
    let read = response(answer, now);
    assert_eq!((read.status(), read.body()), (503, &b"{}"[..]));
    assert_eq!(read.content_type(), Some("application/problem+json"));
    assert_eq!(read.location(), Some("/elsewhere"));
    assert_eq!(read.retry_after(), Some("120"));
    assert_eq!(RetryAfter::parse(read.retry_after().unwrap()).unwrap().delay_from(now), Duration::from_secs(120));
    // Seconds stay seconds; a date gone by is no wait; nonsense is no field.
    let after = |value: &str| response(mailcraft_net::https::Response { status: 429, retry_after: Some(value.into()), ..Default::default() }, now);
    assert_eq!(after("30").retry_after(), Some("30"));
    assert_eq!(after("Wed, 14 Oct 2026 15:00:00 GMT").retry_after(), Some("0"));
    assert_eq!(after("soon").retry_after(), None);
}

#[test]
fn failures_are_told_by_their_kind_alone() {
    let connect = NetError::Connect { host: "jmap.example".into(), reason: "refused".into() };
    let tls = NetError::Tls { host: "jmap.example".into(), reason: "bad".into() };
    assert_eq!(judged(&connect), TransportError::Unreachable);
    assert_eq!(judged(&tls), TransportError::Tls);
    assert_eq!(judged(&NetError::Config("no".into())), TransportError::Other);
}

#[test]
fn credentials_take_the_kind_the_account_says() {
    assert!(credentials("ada@example.org", true, "").is_none());
    assert!(credentials("ada@example.org", false, "").is_none());
    assert!(credentials("ada@example.org", true, TOKEN).is_some());
    assert!(credentials(" ada@example.org ", false, PASSWORD).is_some());
    // A secret no header field can carry.
    assert!(credentials("ada@example.org", true, "two\r\nlines").is_none());
}

#[test]
fn a_stop_once_heard_holds_for_the_job_and_never_sets_the_window_s_flag() {
    let window = Arc::new(AtomicBool::new(false));
    let stop = Stop::new(window.clone());
    assert!(!stop.stopped());
    // The window stops its run, then a run after it sets the flag back: this job stays stopped.
    window.store(true, Ordering::Relaxed);
    assert!(stop.stopped());
    window.store(false, Ordering::Relaxed);
    assert!(stop.stopped(), "latched");
    // A job whose window no longer listens stops itself alone.
    let other = Stop::new(window.clone());
    other.stop();
    assert!(other.stopped());
    assert!(!window.load(Ordering::Relaxed), "the window's flag is the runs' after it");
    assert!(!Stop::new(window).stopped());
}

#[test]
fn a_run_whose_window_no_longer_listens_stops_and_leaves_the_window_s_flag_alone() {
    let (server, _) = filled();
    let served = serve(&server);
    let window = Arc::new(AtomicBool::new(false));
    let stop = Stop::new(window.clone());
    let mut pieces = 0;
    run_mail_with(&run_request(&served.session, "ada@example.org", true), TOKEN, &stop, &mut |_| {
        // The first piece finds nobody listening.
        pieces += 1;
        stop.stop();
    });
    assert!(stop.stopped());
    assert!(!window.load(Ordering::Relaxed));
    let asked = served.requests.load(Ordering::Relaxed);
    // A whole run takes more requests than were made before the first piece and the last.
    let whole = serve(&server);
    run(&run_request(&whole.session, "ada@example.org", true), TOKEN, &Arc::new(AtomicBool::new(false)));
    assert!(asked < whole.requests.load(Ordering::Relaxed), "{asked} requests after the stop, of {}", whole.requests.load(Ordering::Relaxed));
    assert!(pieces >= 1);
}

#[test]
fn a_large_answer_has_more_time_than_a_session() {
    use mailcraft_jmap::RequestKind;
    assert!(patience(RequestKind::Session) < patience(RequestKind::Api));
    assert!(patience(RequestKind::Api) < patience(RequestKind::Download));
    assert!(patience(RequestKind::Download) >= Duration::from_secs(300), "as the browser gives an exchange");
    assert!(patience(RequestKind::Download) <= mailcraft_net::https::MOST_PATIENCE);
}

#[test]
fn a_retry_date_is_no_answer_while_the_clock_is_not_set() {
    // A computer just booted, its clock still in 1970.
    let early = UnixTime::from_secs(60);
    let after = |value: &str| response(mailcraft_net::https::Response { status: 503, retry_after: Some(value.into()), ..Default::default() }, early);
    assert_eq!(after("Wed, 14 Oct 2026 16:02:00 GMT").retry_after(), None);
    // Seconds need no clock.
    assert_eq!(after("45").retry_after(), Some("45"));
}
