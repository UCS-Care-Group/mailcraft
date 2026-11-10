// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A held connection (#252, E4) against a server on this machine that, like Exchange with NTLM,
//! ties the sign-in to the connection: the legs only work on the one socket, one after another.
//! The tokens are opaque to this crate, so the server's are made up (`T1`, `CHALLENGE`, `T3`).
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

/// Reads one request (head and body) from `stream`; None when the client hung up.
fn read_request(stream: &mut impl Read) -> Option<(String, Vec<u8>)> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
        .unwrap_or(0);
    while data.len() < head_end + length {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    Some((head, data[head_end..head_end + length].to_vec()))
}

#[derive(PartialEq, Clone, Copy)]
enum Stage {
    Fresh,
    Challenged,
    SignedIn,
}

/// A server that wants NTLM, per connection: no `Authorization` is 401 `NTLM`; `NTLM T1` is 401
/// with a challenge; `NTLM T3` is 200 on the connection that was challenged, and 401 on any other.
struct Ntlm {
    port: u16,
    connections: Arc<AtomicUsize>,
    /// `(connection number, status)` of every answer, in order.
    log: Arc<Mutex<Vec<(usize, u16)>>>,
}

impl Ntlm {
    fn start(close_after_login: bool) -> Ntlm {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let log = Arc::new(Mutex::new(Vec::new()));
        let (count, record) = (connections.clone(), log.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let number = count.fetch_add(1, Ordering::SeqCst) + 1;
                let record = record.clone();
                std::thread::spawn(move || serve(stream, number, &record, close_after_login));
            }
        });
        Ntlm { port, connections, log }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/EWS/Exchange.asmx", self.port)
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn statuses(&self) -> Vec<(usize, u16)> {
        self.log.lock().unwrap().clone()
    }
}

fn serve(mut stream: TcpStream, number: usize, log: &Mutex<Vec<(usize, u16)>>, close_after_login: bool) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut stage = Stage::Fresh;
    while let Some((head, _body)) = read_request(&mut stream) {
        let authorization = head.lines().find_map(|l| l.strip_prefix("Authorization: ")).unwrap_or("").to_string();
        let mut close = false;
        let (status, headers, body) = match (authorization.as_str(), stage) {
            ("NTLM T1", _) => {
                stage = Stage::Challenged;
                (401, "WWW-Authenticate: NTLM CHALLENGE\r\n", "")
            }
            ("NTLM T3", Stage::Challenged) => {
                stage = Stage::SignedIn;
                close = close_after_login;
                (200, "", "welcome")
            }
            ("", Stage::SignedIn) => (200, "", "again"),
            ("", Stage::Fresh | Stage::Challenged) => (401, "WWW-Authenticate: Negotiate\r\nWWW-Authenticate: NTLM\r\n", ""),
            _ => (401, "WWW-Authenticate: NTLM\r\n", ""),
        };
        log.lock().unwrap().push((number, status));
        let reason = if status == 200 { "OK" } else { "Unauthorized" };
        let connection = if close { "Connection: close\r\n" } else { "" };
        let reply = format!("HTTP/1.1 {status} {reason}\r\n{headers}{connection}Content-Length: {}\r\n\r\n{body}", body.len());
        if stream.write_all(reply.as_bytes()).is_err() || close {
            break;
        }
    }
}

fn ask(connection: &mut Connection, url: &str, authorization: Option<&str>) -> Result<Response, NetError> {
    let fields: Vec<(&str, &str)> = authorization.map(|a| ("Authorization", a)).into_iter().collect();
    connection.send(&Request { method: "POST", url, fields: &fields, body: b"", max_response: 4096, trusted: "" })
}

#[test]
fn the_three_legs_and_the_request_after_them_go_over_one_connection() {
    let server = Ntlm::start(false);
    let url = server.url();
    let mut connection = Connection::open(&url, "").unwrap();
    let first = ask(&mut connection, &url, Some("NTLM T1")).unwrap();
    assert_eq!((first.status, first.challenges), (401, vec!["NTLM CHALLENGE".to_string()]));
    let signed_in = ask(&mut connection, &url, Some("NTLM T3")).unwrap();
    assert_eq!((signed_in.status, signed_in.body.as_slice()), (200, &b"welcome"[..]));
    // Later requests need no header: the connection is the sign-in.
    let later = ask(&mut connection, &url, None).unwrap();
    assert_eq!((later.status, later.body.as_slice()), (200, &b"again"[..]));
    assert!(connection.is_open());
    assert_eq!(server.connections(), 1);
    assert_eq!(server.statuses(), vec![(1, 401), (1, 200), (1, 200)]);
}

#[test]
fn a_new_connection_in_the_middle_of_the_handshake_is_not_signed_in() {
    let server = Ntlm::start(false);
    let url = server.url();
    let mut one = Connection::open(&url, "").unwrap();
    assert_eq!(ask(&mut one, &url, Some("NTLM T1")).unwrap().status, 401);
    // The third leg on another connection is refused ...
    let mut other = Connection::open(&url, "").unwrap();
    assert_eq!(ask(&mut other, &url, Some("NTLM T3")).unwrap().status, 401);
    assert_eq!(server.connections(), 2);
    // ... and on the connection that was challenged it works.
    assert_eq!(ask(&mut one, &url, Some("NTLM T3")).unwrap().status, 200);
}

#[test]
fn a_connection_the_server_closes_says_so_and_is_not_used_again() {
    let server = Ntlm::start(true);
    let url = server.url();
    let mut connection = Connection::open(&url, "").unwrap();
    assert_eq!(ask(&mut connection, &url, Some("NTLM T1")).unwrap().status, 401);
    assert_eq!(ask(&mut connection, &url, Some("NTLM T3")).unwrap().status, 200);
    assert!(!connection.is_open());
    let error = ask(&mut connection, &url, None).unwrap_err();
    assert!(is_closed(&error), "{error:?}");
    assert_eq!(server.connections(), 1);
}

#[test]
fn a_server_that_hangs_up_between_requests_fails_the_next_one_as_closed() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = read_request(&mut stream);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi");
            // And hangs up.
        }
    });
    let url = format!("http://127.0.0.1:{port}/");
    let mut connection = Connection::open(&url, "").unwrap();
    assert_eq!(ask(&mut connection, &url, None).unwrap().body, b"hi");
    std::thread::sleep(Duration::from_millis(100));
    let error = ask(&mut connection, &url, None).unwrap_err();
    assert!(is_closed(&error), "{error:?}");
    assert!(!connection.is_open());
}

#[test]
fn a_held_connection_carries_requests_to_its_own_server_alone() {
    let server = Ntlm::start(false);
    let mut connection = Connection::open(&server.url(), "").unwrap();
    let elsewhere = ask(&mut connection, "http://127.0.0.1:1/EWS/Exchange.asmx", Some("NTLM T1"));
    assert!(matches!(elsewhere, Err(NetError::Config(_))), "{elsewhere:?}");
    let other_host = ask(&mut connection, "https://mail.example.org/EWS/Exchange.asmx", Some("NTLM T1"));
    assert!(matches!(other_host, Err(NetError::Config(_))), "{other_host:?}");
    // Nothing went; the connection is still good.
    assert_eq!(ask(&mut connection, &server.url(), Some("NTLM T1")).unwrap().status, 401);
}

#[test]
fn only_this_machine_is_spoken_to_unencrypted_and_a_bad_fingerprint_is_refused() {
    assert!(matches!(Connection::open("http://mail.example.org/EWS/Exchange.asmx", ""), Err(NetError::Config(_))));
    assert!(matches!(Connection::open("https://mail.example.org/", "not a fingerprint"), Err(NetError::Config(_))));
}

#[test]
fn a_plain_connection_has_no_certificate_to_bind_to_and_shows_nothing() {
    let server = Ntlm::start(false);
    let connection = Connection::open(&server.url(), "").unwrap();
    assert_eq!(connection.end_point_hash(), None);
    let shown = format!("{connection:?}");
    assert!(!shown.contains("127.0.0.1") && shown.contains("open"));
}

#[test]
fn the_client_challenge_is_random() {
    assert_ne!(client_challenge().unwrap(), client_challenge().unwrap());
}

#[test]
fn a_tls_connection_is_held_trusted_by_fingerprint_and_names_its_certificates_hash() {
    let (config, der) = self_signed(&["localhost"], year() - 1, year() + 1);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            count.fetch_add(1, Ordering::SeqCst);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let Ok(connection) = rustls::ServerConnection::new(config.clone()) else { continue };
            let mut tls = rustls::StreamOwned::new(connection, stream);
            while read_request(&mut tls).is_some() {
                if tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok").is_err() || tls.flush().is_err() {
                    break;
                }
            }
        }
    });
    let url = format!("https://localhost:{port}/EWS/Exchange.asmx");
    // Untrusted and unnamed: refused before a byte goes, as for `send`.
    assert!(matches!(Connection::open(&url, ""), Err(NetError::Certificate(_))));
    let mut connection = Connection::open(&url, &cert::fingerprint(&der)).unwrap();
    assert_eq!(ask(&mut connection, &url, Some("NTLM T1")).unwrap().body, b"ok");
    assert_eq!(ask(&mut connection, &url, None).unwrap().body, b"ok");
    // The binding is the hash of the certificate the server showed (SHA-256 for this signature).
    let hash = connection.end_point_hash().unwrap();
    assert_eq!(hash, ring::digest::digest(&ring::digest::SHA256, &der).as_ref().to_vec());
    // The refused attempt connected too, so two connections in all; the two requests shared one.
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}

#[test]
fn a_challenge_header_of_any_realistic_length_arrives_whole() {
    // A minimal NTLM challenge is about 300 characters; one with the most target information
    // read (16 KiB) is about 22,000. Neither is cut or tidied on the way.
    for len in [320usize, 22_000] {
        let token: String = "TlRMTVNTUAAC".chars().cycle().take(len).collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let header = format!("NTLM {token}");
        let sent = header.clone();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = read_request(&mut stream);
                let reply =
                    format!("HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Negotiate\r\nWWW-Authenticate: {sent}\r\nContent-Length: 0\r\n\r\n");
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let mut connection = Connection::open(&url, "").unwrap();
        let answer = ask(&mut connection, &url, Some("NTLM T1")).unwrap();
        assert_eq!(answer.challenges, vec!["Negotiate".to_string(), header]);
    }
}
