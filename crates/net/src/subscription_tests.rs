// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The file behind a subscribed calendar's address, fetched from an in-process HTTPS server whose
//! certificate comes from an authority made for the test.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_model::subscription::{Failure, SubscriptionOutcome};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::*;
use crate::tls_test_support::{Authority, trusting};

const CALENDAR: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a@example.org\r\nDTSTART:20261110T090000Z\r\nSUMMARY:Fixture\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

/// An HTTPS server on this machine: each request is answered by `answer(number, request)`.
struct Tls {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    halt: Arc<AtomicBool>,
}

impl Tls {
    fn start(config: Arc<ServerConfig>, answer: impl Fn(usize, &str) -> Vec<u8> + Send + 'static) -> Tls {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let halt = Arc::new(AtomicBool::new(false));
        let (kept, stop) = (requests.clone(), halt.clone());
        std::thread::spawn(move || {
            for (n, tcp) in listener.incoming().flatten().enumerate() {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let Ok(connection) = ServerConnection::new(config.clone()) else { continue };
                let mut stream = StreamOwned::new(connection, tcp);
                let mut request = Vec::new();
                let mut chunk = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let reply = answer(n, &request);
                kept.lock().unwrap().push(request);
                let _ = stream.write_all(&reply);
                let _ = stream.flush();
                stream.conn.send_close_notify();
                let _ = stream.flush();
            }
        });
        Tls { port, requests, halt }
    }

    fn at(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    fn request(&self, n: usize) -> String {
        self.requests.lock().unwrap().get(n).cloned().unwrap_or_default()
    }

    fn asked(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Drop for Tls {
    fn drop(&mut self) {
        self.halt.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.at());
    }
}

fn reply(status: &str, headers: &str, body: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()
}

fn quick() -> Limits {
    Limits { patience: Duration::from_secs(10), connect: Duration::from_secs(3), retry_wait: Duration::from_millis(10), ..Limits::default() }
}

/// Run `test` with a server for the names `calendar.example` and `other.example` that this
/// machine's test authority vouches for; the names lead to the server.
fn with_server(
    answer: impl Fn(usize, &str) -> Vec<u8> + Send + 'static,
    test: impl FnOnce(&Tls, &dyn Fn(&str, &str, &str, &Limits) -> Result<Fetched, Failure>),
) {
    let authority = Authority::new("MailCraft Test Authority");
    let config = authority.server(&["calendar.example", "other.example", "127.0.0.1"]);
    trusting(vec![authority.root()], || {
        let server = Tls::start(config, answer);
        let allowed = [server.at()];
        let names = move |host: &str, _: u16, _: Duration, _: &crate::pictures::Cancel| {
            if matches!(host.trim_end_matches('.'), "calendar.example" | "other.example") { allowed.to_vec() } else { Vec::new() }
        };
        let look = |address: &str, etag: &str, modified: &str, limits: &Limits| {
            let allowed = [SocketAddr::from(([127, 0, 0, 1], server.port))];
            let reach = Reach { lookup: &names, stand_ins: &allowed, dial: None };
            fetch_with(address, etag, modified, &reach, limits)
        };
        test(&server, &look);
    });
}

fn changed(text: &str, etag: &str, last_modified: &str) -> Result<Fetched, Failure> {
    Ok(Fetched::Changed { text: text.into(), etag: etag.into(), last_modified: last_modified.into() })
}

#[test]
fn the_file_comes_with_its_marks_and_the_request_says_nothing_of_its_sender() {
    with_server(
        |_, _| {
            reply(
                "200 OK",
                "Content-Type: text/calendar\r\nETag: \"v7\"\r\nLast-Modified: Tue, 03 Nov 2026 10:00:00 GMT\r\nSet-Cookie: who=you\r\n",
                CALENDAR,
            )
        },
        |server, look| {
            assert_eq!(
                look("https://calendar.example/private/SECRET/basic.ics?x=1#top", "", "", &quick()),
                changed(CALENDAR, "\"v7\"", "Tue, 03 Nov 2026 10:00:00 GMT")
            );
            let sent = server.request(0);
            assert!(sent.starts_with("GET /private/SECRET/basic.ics?x=1 HTTP/1.1\r\n"), "{sent}");
            let headers: Vec<String> =
                sent.lines().skip(1).filter(|l| !l.is_empty()).map(|l| l.split(':').next().unwrap().to_ascii_lowercase()).collect();
            // Exactly these: no cookie, no credentials, no referrer, nothing of this machine, and no
            // question when there is no copy to ask about.
            assert_eq!(headers, ["host", "user-agent", "accept", "accept-encoding", "connection"], "{sent}");
            assert!(sent.contains("Accept: text/calendar"), "{sent}");
        },
    );
}

#[test]
fn a_copy_that_is_current_is_asked_about_and_a_304_is_taken() {
    with_server(
        |_, request| {
            if request.contains("If-None-Match: \"v7\"") { reply("304 Not Modified", "ETag: \"v7\"\r\n", "") } else { reply("200 OK", "", CALENDAR) }
        },
        |server, look| {
            assert_eq!(look("https://calendar.example/c.ics", "\"v7\"", "Tue, 03 Nov 2026 10:00:00 GMT", &quick()), Ok(Fetched::NotModified));
            let sent = server.request(0);
            assert!(sent.contains("If-None-Match: \"v7\"\r\n") && sent.contains("If-Modified-Since: Tue, 03 Nov 2026 10:00:00 GMT\r\n"), "{sent}");
            // Another mark is a new file.
            assert_eq!(look("https://calendar.example/c.ics", "\"v6\"", "", &quick()), changed(CALENDAR, "", ""));
        },
    );
    // A 304 to a question nobody asked is no answer.
    with_server(
        |_, _| reply("304 Not Modified", "", ""),
        |_, look| {
            assert_eq!(look("https://calendar.example/c.ics", "", "", &quick()), Err(Failure::Status(304)));
        },
    );
}

#[test]
fn marks_that_are_not_plain_text_are_never_sent() {
    with_server(
        |_, _| reply("200 OK", "", CALENDAR),
        |server, look| {
            assert!(look("https://calendar.example/c.ics", "\"a\"\r\nCookie: x=1", "caf\u{e9}", &quick()).is_ok());
            let sent = server.request(0);
            assert!(!sent.contains("Cookie") && !sent.contains("If-None-Match") && !sent.contains("If-Modified-Since"), "{sent}");
        },
    );
}

#[test]
fn redirects_are_followed_to_https_hosts_and_the_marks_stay_with_the_first_host() {
    let port = Arc::new(Mutex::new(0u16));
    let said = port.clone();
    with_server(
        move |_, request| {
            let port = *said.lock().unwrap();
            if request.starts_with("GET /old") {
                reply("301 Moved Permanently", &format!("Location: https://other.example:{port}/new.ics\r\n"), "moved")
            } else if request.starts_with("GET /relative") {
                reply("302 Found", "Location: /new.ics\r\n", "moved")
            } else {
                reply("200 OK", "", CALENDAR)
            }
        },
        |server, look| {
            *port.lock().unwrap() = server.port;
            let at = |path: &str| format!("https://calendar.example:{}{path}", server.port);
            assert_eq!(look(&at("/old.ics"), "\"v1\"", "", &quick()), changed(CALENDAR, "", ""));
            assert!(server.request(0).contains("If-None-Match: \"v1\""), "{}", server.request(0));
            assert!(
                server.request(1).starts_with("GET /new.ics HTTP/1.1\r\n") && !server.request(1).contains("If-None-Match"),
                "{}",
                server.request(1)
            );
            assert_eq!(look(&at("/relative.ics"), "", "", &quick()), changed(CALENDAR, "", ""));
        },
    );
}

#[test]
fn a_redirect_to_plain_http_or_anywhere_unfit_is_refused_and_so_are_too_many() {
    let answers = Arc::new(Mutex::new(String::new()));
    let said = answers.clone();
    with_server(
        move |_, _| reply("302 Found", &format!("Location: {}\r\n", said.lock().unwrap()), "moved"),
        |server, look| {
            let at = format!("https://calendar.example:{}/c.ics", server.port);
            for to in
                ["http://calendar.example/c.ics", "ftp://calendar.example/c.ics", "https://user:pw@calendar.example/c.ics", "javascript:alert(1)"]
            {
                *answers.lock().unwrap() = to.to_string();
                assert_eq!(look(&at, "", "", &quick()), Err(Failure::Address), "{to}");
            }
            // Where the redirect leads is looked at as much as the first address: never this machine.
            // (127.0.0.1 is the test's stand-in here, so a private address stands for them all.)
            *answers.lock().unwrap() = "https://10.0.0.5/c.ics".to_string();
            assert_eq!(look(&at, "", "", &quick()), Err(Failure::NotPublic));
            *answers.lock().unwrap() = format!("https://calendar.example:{}/c.ics", server.port);
            let before = server.asked();
            assert_eq!(look(&at, "", "", &quick()), Err(Failure::TooManyRedirects));
            assert_eq!(server.asked() - before, 1 + MAX_REDIRECTS);
        },
    );
}

#[test]
fn an_address_that_is_not_https_is_refused_before_anything_is_sent() {
    with_server(
        |_, _| reply("200 OK", "", CALENDAR),
        |server, look| {
            for address in [
                "http://calendar.example/c.ics",
                "ftp://calendar.example/c.ics",
                "https://user:pw@calendar.example/c.ics",
                "calendar.example/c.ics",
                "",
            ] {
                assert_eq!(look(address, "", "", &quick()), Err(Failure::Address), "{address}");
            }
            assert_eq!(server.asked(), 0);
        },
    );
}

#[test]
fn an_address_on_this_machine_or_a_private_network_is_refused() {
    let server = Tls::start(Authority::new("MailCraft Test Authority").server(&["127.0.0.1"]), |_, _| reply("200 OK", "", CALENDAR));
    // The reach every other build has: this machine is out of reach, though the server is there.
    for address in [
        format!("https://127.0.0.1:{}/c.ics", server.port),
        format!("https://localhost:{}/c.ics", server.port),
        format!("https://[::1]:{}/c.ics", server.port),
        "https://192.168.1.10/c.ics".to_string(),
        "https://10.1.2.3/c.ics".to_string(),
        "https://169.254.169.254/latest/meta-data".to_string(),
    ] {
        assert_eq!(fetch(&address, "", ""), Err(Failure::NotPublic), "{address}");
    }
    assert_eq!(server.asked(), 0);
}

#[test]
fn a_file_larger_than_may_be_taken_is_refused() {
    let big = "X".repeat(5_000);
    with_server(
        move |_, _| reply("200 OK", "", &big),
        |_, look| {
            let small = Limits { max_bytes: 4_000, ..quick() };
            assert_eq!(look("https://calendar.example/c.ics", "", "", &small), Err(Failure::TooLarge));
            assert!(look("https://calendar.example/c.ics", "", "", &Limits { max_bytes: 6_000, ..quick() }).is_ok());
        },
    );
    // The limit the real thing has is the engine's: 8 MB.
    assert_eq!(MAX_BYTES, 8 * 1024 * 1024);
}

#[test]
fn what_is_not_a_file_is_reported_by_its_status() {
    for (status, failure) in [
        ("401 Unauthorized", Failure::Status(401)),
        ("403 Forbidden", Failure::Status(403)),
        ("404 Not Found", Failure::Status(404)),
        ("500 Internal Server Error", Failure::Status(500)),
        ("204 No Content", Failure::Status(204)),
    ] {
        with_server(
            move |_, _| reply(status, "", "nope"),
            |_, look| {
                assert_eq!(look("https://calendar.example/c.ics", "", "", &quick()), Err(failure), "{status}");
            },
        );
    }
    // A redirect that says nowhere to go.
    with_server(
        |_, _| reply("302 Found", "", ""),
        |_, look| {
            assert_eq!(look("https://calendar.example/c.ics", "", "", &quick()), Err(Failure::Status(302)));
        },
    );
}

#[test]
fn a_server_that_says_not_now_is_asked_once_more() {
    with_server(
        |n, _| if n == 0 { reply("503 Service Unavailable", "Retry-After: 0\r\n", "") } else { reply("200 OK", "", CALENDAR) },
        |server, look| {
            assert_eq!(look("https://calendar.example/c.ics", "", "", &quick()), changed(CALENDAR, "", ""));
            assert_eq!(server.asked(), 2);
        },
    );
    with_server(
        |_, _| reply("503 Service Unavailable", "", ""),
        |server, look| {
            assert_eq!(look("https://calendar.example/c.ics", "", "", &quick()), Err(Failure::Status(503)));
            assert_eq!(server.asked(), 2);
        },
    );
}

#[test]
fn text_that_is_not_utf8_is_read_as_windows_1252_or_in_the_set_the_server_names() {
    with_server(
        |_, _| {
            let body = b"BEGIN:VCALENDAR\r\nSUMMARY:caf\xe9\r\nEND:VCALENDAR\r\n";
            let mut out = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
            out.extend_from_slice(body);
            out
        },
        |_, look| match look("https://calendar.example/c.ics", "", "", &quick()) {
            Ok(Fetched::Changed { text, .. }) => assert!(text.contains("caf\u{e9}") && !text.contains('\u{fffd}'), "{text}"),
            other => panic!("{other:?}"),
        },
    );
    // A set the server names is the one used (here, bytes that are no UTF-8 and mean another thing in Latin-1).
    with_server(
        |_, _| {
            let body = b"BEGIN:VCALENDAR\r\nSUMMARY:\xe8\xe9\r\nEND:VCALENDAR\r\n";
            let mut out = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/calendar; charset=\"ISO-8859-2\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            out.extend_from_slice(body);
            out
        },
        |_, look| match look("https://calendar.example/c.ics", "", "", &quick()) {
            Ok(Fetched::Changed { text, .. }) => assert!(text.contains("\u{10d}\u{e9}"), "{text}"),
            other => panic!("{other:?}"),
        },
    );
}

#[test]
fn marks_from_another_host_after_a_redirect_are_not_kept_to_be_sent_to_the_first() {
    let port = Arc::new(Mutex::new(0u16));
    let said = port.clone();
    with_server(
        move |_, request| {
            let port = *said.lock().unwrap();
            if request.starts_with("GET /old") {
                reply("301 Moved Permanently", &format!("Location: https://other.example:{port}/new.ics\r\n"), "moved")
            } else {
                reply("200 OK", "ETag: \"theirs\"\r\nLast-Modified: Tue, 03 Nov 2026 10:00:00 GMT\r\n", CALENDAR)
            }
        },
        |server, look| {
            *port.lock().unwrap() = server.port;
            let at = |path: &str| format!("https://calendar.example:{}{path}", server.port);
            // The marks the other host gave are its own: nothing is kept that the first would be sent.
            assert_eq!(look(&at("/old.ics"), "", "", &quick()), changed(CALENDAR, "", ""));
            // From the host asked, they are kept.
            assert_eq!(look(&at("/new.ics"), "", "", &quick()), changed(CALENDAR, "\"theirs\"", "Tue, 03 Nov 2026 10:00:00 GMT"));
        },
    );
}

#[test]
fn a_certificate_that_is_not_trusted_is_no_connection() {
    // A server whose certificate comes from an authority nobody trusts.
    let server = Tls::start(Authority::new("Unknown Authority").server(&["calendar.example"]), |_, _| reply("200 OK", "", CALENDAR));
    let allowed = [server.at()];
    let lookup = move |_: &str, _: u16, _: Duration, _: &crate::pictures::Cancel| allowed.to_vec();
    let reach = Reach { lookup: &lookup, stand_ins: &allowed, dial: None };
    let trusted = Authority::new("MailCraft Test Authority");
    trusting(vec![trusted.root()], || {
        let result = fetch_with(&format!("https://calendar.example:{}/c.ics", server.port), "", "", &reach, &quick());
        assert_eq!(result, Err(Failure::Network));
    });
}

#[test]
fn a_look_gives_the_engine_its_outcome_without_the_address() {
    let failed = look("http://calendar.example/private/SECRET/c.ics", "", "");
    assert_eq!(failed, SubscriptionOutcome::Failed { failure: "address".into() });
    assert!(!format!("{failed:?}").contains("SECRET"));
}
