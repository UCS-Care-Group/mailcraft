// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! One-click unsubscribe (RFC 8058): what is sent, and what is never followed or reached.
//!
//! Against a fake server on this machine; the hosts are invented.

use std::net::SocketAddr;
use std::time::Duration;

use super::{Limits, Reach, unsubscribe, unsubscribe_with};
use crate::fake::FakeHttp;

fn at(server: &FakeHttp) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], server.port))
}

fn quick() -> Limits {
    Limits { patience: Duration::from_secs(5), connect: Duration::from_secs(3), ..Limits::default() }
}

fn reply(status: &str, headers: &str) -> Vec<u8> {
    format!("HTTP/1.1 {status}\r\n{headers}Content-Length: 2\r\nConnection: close\r\n\r\nok").into_bytes()
}

#[test]
fn the_post_says_one_click_and_nothing_of_its_sender() {
    let server = FakeHttp::answering(reply("200 OK", "Set-Cookie: who=you\r\n"));
    let allowed = [at(&server)];
    let reach = Reach { lookup: &|_: &str, _: u16, _: Duration| Vec::new(), stand_ins: &allowed, dial: None };
    assert_eq!(unsubscribe_with(&server.url("/u/7f3a?l=news"), &reach, &quick()), Ok(()));
    let sent = server.request(0);
    assert!(sent.starts_with("POST /u/7f3a?l=news HTTP/1.1\r\n"), "{sent}");
    assert!(sent.ends_with("\r\n\r\nList-Unsubscribe=One-Click"), "{sent}");
    let headers: Vec<String> =
        sent.lines().skip(1).take_while(|l| !l.is_empty()).filter_map(|l| l.split_once(": ")).map(|(n, _)| n.to_ascii_lowercase()).collect();
    assert_eq!(headers, ["host", "user-agent", "accept", "accept-encoding", "content-type", "content-length", "connection"]);
}

#[test]
fn a_redirect_is_not_followed_and_a_refusal_is_said() {
    let server = FakeHttp::answering(reply("302 Found", "Location: https://elsewhere.example/\r\n"));
    let allowed = [at(&server)];
    let reach = Reach { lookup: &|_: &str, _: u16, _: Duration| Vec::new(), stand_ins: &allowed, dial: None };
    assert!(unsubscribe_with(&server.url("/u"), &reach, &quick()).is_err_and(|e| e.contains("302")));
    assert_eq!(server.asked(), 1, "one request, nothing followed");

    let server = FakeHttp::answering(reply("404 Not Found", ""));
    let allowed = [at(&server)];
    let reach = Reach { lookup: &|_: &str, _: u16, _: Duration| Vec::new(), stand_ins: &allowed, dial: None };
    assert!(unsubscribe_with(&server.url("/u"), &reach, &quick()).is_err_and(|e| e.contains("404")));
}

#[test]
fn only_a_public_https_address_is_posted_to() {
    // Refused before anything is looked up: not https, no address at all.
    for url in ["http://lists.example/u", "ftp://lists.example/u", "mailto:leave@lists.example", "", "https://user:pw@lists.example/u"] {
        assert!(unsubscribe(url).is_err(), "{url}");
    }
    // This computer, without the tests' stand-ins: never reached.
    let server = FakeHttp::answering(reply("200 OK", ""));
    let reach = Reach { lookup: &|_: &str, _: u16, _: Duration| Vec::new(), stand_ins: &[], dial: None };
    assert!(unsubscribe_with(&server.url("/u"), &reach, &quick()).is_err());
    assert_eq!(server.asked(), 0);
}
