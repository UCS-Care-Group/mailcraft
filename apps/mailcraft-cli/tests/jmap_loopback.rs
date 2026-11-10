// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The crate's JMAP server in the process (`mailcraft_jmap::standin`), served over HTTP on this
//! computer's loopback with what a browser needs to call it from a page of another origin (CORS),
//! for trying the browser build against it by hand. It is not a test that runs by itself:
//!
//! ```text
//! cargo test -p mailcraft-cli --test jmap_loopback -- --ignored --nocapture
//! ```
//!
//! serves `http://127.0.0.1:8780/jmap/session` (another port with `MAILCRAFT_JMAP_PORT`), which
//! lets in the bearer token `standin-token`, until it is stopped. (The session names its other
//! addresses relative to itself, so they are the loopback's too.) Nothing here leaves this
//! computer; every name, address and message is invented.
#![cfg(not(target_arch = "wasm32"))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use mailcraft_jmap::standin::{SESSION, StandIn, TOKEN};

/// The most a request's header and body may take.
const MOST_HEADER: usize = 64 * 1024;
const MOST_BODY: usize = 16 * 1024 * 1024;
/// What a page of another origin may send and read.
const CORS: &str = "Access-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: Retry-After, Location\r\n";

#[test]
#[ignore = "serves until it is stopped: for trying the browser build by hand"]
fn serve_the_stand_in_on_loopback() {
    let port: u16 = std::env::var("MAILCRAFT_JMAP_PORT").ok().and_then(|port| port.parse().ok()).unwrap_or(8780);
    let server = filled();
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    println!("serving http://127.0.0.1:{port}/jmap/session; the bearer token is {TOKEN}");
    // A connection at a time each, so that one the browser opens and leaves idle holds up nothing.
    for stream in listener.incoming().flatten() {
        let server = server.clone();
        std::thread::spawn(move || {
            if let Err(e) = serve(&server, stream) {
                println!("a request went wrong: {e}");
            }
        });
    }
}

/// A stand-in with an Inbox and its fellows, a folder inside another, and a few messages.
fn filled() -> StandIn {
    let server = StandIn::new();
    server.with(|d| {
        let inbox = d.mailbox("Inbox", None, Some("inbox"));
        d.mailbox("Sent", None, Some("sent"));
        d.mailbox("Trash", None, Some("trash"));
        let projects = d.mailbox("Projects", None, None);
        let apollo = d.mailbox("Apollo", Some(&projects), None);
        for (n, subject) in ["Kickoff", "Budget for next year", "Agenda for Thursday", "Minutes of the review"].into_iter().enumerate() {
            let keywords: &[&str] = if n == 0 { &["$seen"] } else { &[] };
            d.deliver(&inbox, &mail(subject, n), keywords, &at(n));
        }
        d.deliver(&apollo, &mail("Launch plan", 9), &["$flagged"], &at(9));
    });
    server
}

/// A message as a mail program writes one.
fn mail(subject: &str, n: usize) -> Vec<u8> {
    format!(
        "From: Zoe Park <zoe@example.org>\r\nTo: Ada <ada@example.org>\r\nSubject: {subject}\r\nMessage-ID: <{n}@example.org>\r\nDate: Thu, 15 Oct 2026 09:{n:02}:00 +0200\r\n\r\nHello Ada,\r\n\r\nThis is about {subject}.\r\n\r\nZoe\r\n"
    )
    .into_bytes()
}

fn at(n: usize) -> String {
    format!("2026-10-15T07:{n:02}:00Z")
}

/// One request on `stream`, answered by the stand-in, and the connection closed.
fn serve(server: &StandIn, stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?.take(MOST_HEADER as u64));
    let (mut line, mut taken) = (String::new(), 0usize);
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or("/").to_string());
    let mut fields: Vec<(String, String)> = Vec::new();
    loop {
        let mut field = String::new();
        let read = reader.read_line(&mut field)?;
        taken += read;
        if read == 0 || field.trim().is_empty() || taken > MOST_HEADER {
            break;
        }
        if let Some((name, value)) = field.split_once(':') {
            fields.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let length = fields.iter().find(|(name, _)| name.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0usize);
    let mut body = vec![0; length.min(MOST_BODY)];
    // (Past the header, the body may take more than the header's share.)
    reader.get_mut().set_limit(MOST_BODY as u64);
    reader.read_exact(&mut body)?;
    let mut stream = stream;
    if method == "OPTIONS" {
        let allow = "Access-Control-Allow-Methods: GET, POST\r\nAccess-Control-Allow-Headers: Authorization, Content-Type, Accept\r\nAccess-Control-Max-Age: 600\r\n";
        return write!(stream, "HTTP/1.1 204 No Content\r\n{CORS}{allow}Content-Length: 0\r\nConnection: close\r\n\r\n");
    }
    let origin = SESSION.trim_end_matches("/jmap/session");
    let named: Vec<(&str, &str)> = fields.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
    let answer = server.answer(&method, &format!("{origin}{target}"), &named, &body);
    let Ok(answer) = answer else {
        return write!(stream, "HTTP/1.1 502 Bad Gateway\r\n{CORS}Content-Length: 0\r\nConnection: close\r\n\r\n");
    };
    let out = answer.body();
    let mut head = format!("HTTP/1.1 {} Answer\r\n{CORS}Content-Length: {}\r\nConnection: close\r\n", answer.status(), out.len());
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
