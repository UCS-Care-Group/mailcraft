// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Connections kept open: reused when the answer leaves them fit, dropped otherwise, and a
//! request that would happen twice never asked on one that waited, nor asked again.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use proptest::prelude::*;

use super::*;
use crate::fake::{FakeKeptHttp, Kept, kept_answer};
use crate::http::{MAX_BODY, read_reply_kept};

const SOON: Duration = Duration::from_secs(5);

fn url(server: &FakeKeptHttp, path: &str) -> Url {
    Url::parse(&server.url(path)).unwrap()
}

fn get(pool: &Pool, at: &Url) -> Result<Reply, NetError> {
    pool.request(at, &Request { method: "GET", headers: &[("Accept", "application/json")], body: None, patience: SOON, max_body: MAX_BODY }, true)
}

fn post(pool: &Pool, at: &Url, repeatable: bool) -> Result<Reply, NetError> {
    let headers = [("Content-Type", "application/json")];
    pool.request(at, &Request { method: "POST", headers: &headers, body: Some(b"{}"), patience: SOON, max_body: MAX_BODY }, repeatable)
}

fn pool() -> Pool {
    Pool::new(PER_HOST, MOST, IDLE_FOR)
}

/// Every answer `{"n":…}` with its length, the connection left open.
fn counting() -> FakeKeptHttp {
    FakeKeptHttp::start(|n, _, _| Kept::Answer(kept_answer("200 OK", &format!("{{\"n\":{n}}}"))))
}

#[test]
fn requests_one_after_another_go_on_one_connection() {
    let server = counting();
    let pool = pool();
    let at = url(&server, "/v1.0/me/mailFolders");
    for n in 0..5 {
        let reply = get(&pool, &at).unwrap();
        assert_eq!((reply.status, reply.body), (200, format!("{{\"n\":{n}}}").into_bytes()));
    }
    assert_eq!((server.connections(), server.asked(), pool.waiting()), (1, 5, 1));
    // It asks for the connection to stay open, and nothing a caller says changes that.
    assert!(server.request(0).contains("Connection: keep-alive\r\n"), "{}", server.request(0));
    let told =
        Request { method: "GET", headers: &[("Connection", "close"), ("connection", "upgrade")], body: None, patience: SOON, max_body: MAX_BODY };
    pool.request(&at, &told, true).unwrap();
    let sent = server.request(5).to_ascii_lowercase();
    assert_eq!(sent.matches("connection:").count(), 1, "{sent}");
    assert_eq!(server.connections(), 1);
}

#[test]
fn answers_in_chunks_and_without_a_body_leave_the_connection_open() {
    let server = FakeKeptHttp::start(|n, _, _| {
        Kept::Answer(match n % 3 {
            0 => b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n".to_vec(),
            1 => b"HTTP/1.1 204 No Content\r\n\r\n".to_vec(),
            _ => b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
        })
    });
    let pool = pool();
    let at = url(&server, "/v1.0/$batch");
    let statuses: Vec<u16> = (0..6).map(|_| get(&pool, &at).unwrap().status).collect();
    assert_eq!(statuses, [200, 204, 429, 200, 204, 429]);
    assert_eq!(get(&pool, &at).unwrap().body, b"{\"a\":1}");
    assert_eq!(server.connections(), 1);
}

#[test]
fn a_server_that_says_it_closes_is_not_asked_again_on_that_connection() {
    for answer in [
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: Keep-Alive, Close\r\n\r\n{}",
        // HTTP/1.0 closes unless it says otherwise, and that is not taken from it.
        "HTTP/1.0 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\n{}",
        // A body that runs to the end of the connection.
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{}",
        // A coding that isn't chunks last, and a length beside chunks.
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\nContent-Length: 2\r\n\r\n{}",
        "HTTP/1.1 200 OK\r\nContent-Length: 7\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n",
        // More than the answer: what comes after it is not the start of the next one.
        "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}",
    ] {
        let server = FakeKeptHttp::start(move |_, _, _| Kept::AnswerAndClose(answer.as_bytes().to_vec()));
        let pool = pool();
        let at = url(&server, "/v1.0/me");
        for _ in 0..3 {
            assert_eq!(get(&pool, &at).unwrap().body, b"{}", "{answer:?}");
        }
        assert_eq!((server.connections(), server.asked(), pool.waiting()), (3, 3, 0), "{answer:?}");
    }
}

#[test]
fn a_connection_that_closed_while_it_waited_is_replaced_for_a_read() {
    // The first answer of every connection is the last: the server hangs up without a word.
    let server = FakeKeptHttp::start(|n, _, _| Kept::AnswerAndClose(kept_answer("200 OK", &format!("{{\"n\":{n}}}"))));
    let pool = pool();
    let at = url(&server, "/v1.0/me/messages");
    for n in 0..3 {
        // Asked on the closed one, found closed, asked on a new one: no error.
        assert_eq!(get(&pool, &at).unwrap().body, format!("{{\"n\":{n}}}").into_bytes());
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!((server.connections(), server.asked()), (3, 3));
}

#[test]
fn what_would_happen_twice_never_goes_on_a_connection_that_waited() {
    let server = counting();
    let pool = pool();
    let at = url(&server, "/v1.0/me/sendMail");
    get(&pool, &at).unwrap();
    assert_eq!(pool.waiting(), 1);
    // A POST that may not be asked again has a new connection, however many wait...
    post(&pool, &at, false).unwrap();
    post(&pool, &at, false).unwrap();
    assert_eq!(server.connections(), 3);
    // ...and its connection may serve a read after it.
    assert_eq!(pool.waiting(), 3);
    get(&pool, &at).unwrap();
    assert_eq!(server.connections(), 3);
    // A $batch of reads may go on one that waited.
    post(&pool, &at, true).unwrap();
    assert_eq!(server.connections(), 3);
}

#[test]
fn what_would_happen_twice_is_asked_once_even_when_its_answer_is_lost() {
    // The server takes the request and hangs up before a word of the answer.
    let server =
        FakeKeptHttp::start(|_, _, request| if request.starts_with("POST ") { Kept::Hang } else { Kept::Answer(kept_answer("200 OK", "{}")) });
    let pool = pool();
    let at = url(&server, "/v1.0/me/messages/AAMk/send");
    get(&pool, &at).unwrap();
    let lost = post(&pool, &at, false).unwrap_err();
    assert!(http::is_closed(&lost), "{lost:?}");
    assert_eq!(server.asked(), 2, "the POST was asked once");
    assert!(server.request(1).starts_with("POST "));
}

#[test]
fn a_read_whose_answer_was_cut_off_is_not_asked_again_here() {
    // A byte of the answer came: the server had the request, and what to do is the caller's call.
    let server = FakeKeptHttp::start(|n, _, _| {
        if n == 1 {
            Kept::AnswerAndClose(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}".to_vec())
        } else {
            Kept::Answer(kept_answer("200 OK", "{}"))
        }
    });
    let pool = pool();
    let at = url(&server, "/v1.0/me");
    get(&pool, &at).unwrap();
    assert!(http::is_closed(&get(&pool, &at).unwrap_err()));
    assert_eq!(server.asked(), 2);
    assert_eq!(pool.waiting(), 0, "the broken connection was dropped");
    get(&pool, &at).unwrap();
    assert_eq!(server.connections(), 2);
}

#[test]
fn an_answer_that_is_still_to_come_is_never_read_as_the_next_ones() {
    // Five "go on"s, then the answer proper late: it must not be taken for the next request's.
    let continues = "HTTP/1.1 100 Continue\r\n\r\n".repeat(5);
    let server = FakeKeptHttp::start(move |n, _, _| {
        if n == 0 {
            let mut answer = continues.as_bytes().to_vec();
            answer.extend(kept_answer("200 OK", "{\"late\":1}"));
            Kept::Answer(answer)
        } else {
            Kept::Answer(kept_answer("200 OK", &format!("{{\"n\":{n}}}")))
        }
    });
    let pool = pool();
    let at = url(&server, "/v1.0/me");
    get(&pool, &at).unwrap_err();
    assert_eq!(pool.waiting(), 0, "the connection with an answer still on it was dropped");
    let reply = get(&pool, &at).unwrap();
    assert_eq!(reply.body, b"{\"n\":1}".to_vec());
    assert_eq!(server.connections(), 2);
    assert!(kept_after(b"HTTP/1.1 100 Continue\r\n\r\n".repeat(5).as_slice()).is_none());
}

#[test]
fn an_answer_that_is_too_long_drops_its_connection() {
    let server = FakeKeptHttp::start(|n, _, _| Kept::Answer(kept_answer("200 OK", &"x".repeat(if n == 1 { MAX_BODY + 1 } else { 2 }))));
    let pool = pool();
    let at = url(&server, "/v1.0/me");
    get(&pool, &at).unwrap();
    assert!(http::is_too_long(&get(&pool, &at).unwrap_err()));
    assert_eq!(pool.waiting(), 0);
    get(&pool, &at).unwrap();
    assert_eq!(server.connections(), 2);
}

#[test]
fn connections_wait_within_their_limits() {
    // Not longer than they may.
    let server = counting();
    let brief = Pool::new(PER_HOST, MOST, Duration::from_millis(50));
    let at = url(&server, "/v1.0/me");
    get(&brief, &at).unwrap();
    std::thread::sleep(Duration::from_millis(120));
    get(&brief, &at).unwrap();
    assert_eq!(server.connections(), 2);
    assert_eq!(brief.waiting(), 1, "the one that waited too long is gone");

    // Not more for one server, nor in all, than they may: the one that waited longest makes room.
    let (one, two) = (counting(), counting());
    let small = Pool::new(2, 3, IDLE_FOR);
    for server in [&one, &one, &one, &two, &two] {
        let at = url(server, "/v1.0/me");
        let stream = http::connect(&at, SOON).unwrap();
        small.put(Key::of(&at), stream, 1);
    }
    assert_eq!(small.waiting(), 3);
    assert_eq!(small.idle().iter().filter(|idle| idle.key.port == one.port).count(), 1);
    // Nor one that has carried as many requests as one may.
    let at = url(&one, "/v1.0/me");
    let used = Pool::new(PER_HOST, MOST, IDLE_FOR);
    used.put(Key::of(&at), http::connect(&at, SOON).unwrap(), MOST_USES);
    assert_eq!(used.waiting(), 0);
    let none = Pool::new(0, MOST, IDLE_FOR);
    none.put(Key::of(&at), http::connect(&at, SOON).unwrap(), 1);
    assert_eq!(none.waiting(), 0);
}

#[test]
fn connections_are_kept_apart_by_server() {
    let (one, two) = (counting(), counting());
    let pool = pool();
    for _ in 0..3 {
        get(&pool, &url(&one, "/a")).unwrap();
        get(&pool, &url(&two, "/b")).unwrap();
    }
    assert_eq!((one.connections(), two.connections(), pool.waiting()), (1, 1, 2));
    // The same server written another way is the same; another port is not.
    let shouting = Url::parse(&format!("http://LOCALHOST:{}/c", one.port)).unwrap();
    assert_eq!(Key::of(&shouting), Key { host: "localhost".into(), port: one.port, secure: false });
    assert_ne!(Key::of(&url(&one, "/")), Key::of(&url(&two, "/")));
    assert_eq!(Key::of(&Url::parse("https://graph.example.org/v1.0").unwrap()), Key { host: "graph.example.org".into(), port: 443, secure: true });
    assert_ne!(Key::of(&Url::parse("https://localhost:80/").unwrap()), Key::of(&Url::parse("http://localhost/").unwrap()));
}

#[test]
fn the_rules_of_every_connection_still_hold() {
    let pool = pool();
    // What is no way of asking is not sent.
    let at = Url::parse("http://127.0.0.1:9/").unwrap();
    let bad = Request { method: "get", headers: &[], body: None, patience: SOON, max_body: MAX_BODY };
    assert!(matches!(pool.request(&at, &bad, true), Err(NetError::Config(_))));
    // Plain HTTP only to this machine: an address that says otherwise is no address.
    let plain = Url { host: "graph.example.org".into(), port: 0, secure: false, path: "/".into() };
    let asked = Request { method: "GET", headers: &[], body: None, patience: SOON, max_body: MAX_BODY };
    assert!(matches!(pool.request(&plain, &asked, true), Err(NetError::Config(_))));
    assert_eq!(pool.waiting(), 0);
}

/// Bytes to read and a place to write, as a connection.
struct Wire {
    input: Vec<u8>,
    at: usize,
}

impl Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let rest = &self.input[self.at..];
        let n = rest.len().min(buf.len()).min(7);
        buf[..n].copy_from_slice(&rest[..n]);
        self.at += n;
        Ok(n)
    }
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A connection is kept after an answer only when the reader knows where it ended.
fn kept_after(bytes: &[u8]) -> Option<(Reply, bool)> {
    read_reply_kept(&mut &bytes[..], Instant::now(), SOON, MAX_BODY).ok()
}

#[test]
fn the_reader_says_when_a_connection_is_fit_for_more() {
    let fit = |text: &str| kept_after(text.as_bytes()).unwrap().1;
    assert!(fit("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"));
    assert!(fit("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n"));
    assert!(fit("HTTP/1.1 304 Not Modified\r\nContent-Length: 99\r\n\r\n"));
    assert!(fit("HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"));
    assert!(!fit("HTTP/1.1 200 OK\r\n\r\n{}"));
    assert!(!fit("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nconnection: CLOSE\r\n\r\n{}"));
    assert!(!fit("HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\n{}"));
    assert!(!fit("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}x"));
    // Chunks whose end is the end of the stream (no empty line after the last).
    assert!(!fit("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n"));
}

proptest! {
    #[test]
    fn arbitrary_answers_on_a_kept_connection_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..800)) {
        let checked = |bytes: &[u8]| {
            if let Some((reply, _)) = kept_after(bytes) {
                assert!(reply.body.len() <= MAX_BODY);
            }
        };
        checked(&bytes);
        for head in ["HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n", "HTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\n"] {
            let mut after = head.as_bytes().to_vec();
            after.extend_from_slice(&bytes);
            checked(&after);
        }
        // And through a whole exchange.
        let at = Url::parse("http://127.0.0.1:9/v1.0/me").unwrap();
        let asked = Request { method: "GET", headers: &[], body: None, patience: SOON, max_body: MAX_BODY };
        let _ = http::exchange(&mut Wire { input: bytes, at: 0 }, &at, &asked, true);
    }

    #[test]
    fn chunk_shaped_noise_never_panics_and_a_kept_answer_ended_where_it_said(
        chunks in proptest::collection::vec(prop_oneof![Just("0\r\n".to_string()), Just("\r\n".to_string()), "[0-9a-fA-F]{1,9}(;[a-z=]{0,6})?\r\n", "[ -~]{0,24}"], 0..16),
        trailer in "(X-T: [a-z]{0,4}\r\n)?(\r\n)?",
        after in "[ -~]{0,4}",
    ) {
        let text = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{}{trailer}{after}", chunks.concat());
        if let Some((reply, fit)) = kept_after(text.as_bytes()) {
            prop_assert!(reply.body.len() <= MAX_BODY);
            // Kept only when the answer ended on its empty line, with nothing after it.
            if fit {
                prop_assert!(text.ends_with('\n'));
            }
        }
    }
}
