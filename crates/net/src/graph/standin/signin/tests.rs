// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's token endpoint, asked by this crate's own sign-in code, and what a process may
//! be routed to. Nothing here leaves this machine.

use std::io::{Read, Write};
use std::net::TcpStream;

use mailcraft_model::{OAuth, OAuthProvider};

use crate::graph::standin::{REFRESH, StandIn, TOKEN, route_this_process};
use crate::oauth::{Endpoints, Resource, refresh_for_at};

fn endpoints(server: &StandIn) -> Endpoints {
    let oauth = OAuth { provider: OAuthProvider::Microsoft, client_id: "client-of-the-test".into(), ..Default::default() };
    let plain = Endpoints::for_resource(&oauth, Resource::GraphMail);
    Endpoints { token: format!("{}/common/oauth2/v2.0/token", server.url()), ..plain }
}

fn post(server: &StandIn, kind: &str, form: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    write!(stream, "POST /common/oauth2/v2.0/token HTTP/1.1\r\nHost: x\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n{form}", form.len())
        .unwrap();
    let mut all = String::new();
    stream.read_to_string(&mut all).unwrap();
    let status = all.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    (status, all.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or_default())
}

#[test]
fn a_refresh_at_the_stand_in_gives_its_token_and_names_the_owner() {
    let server = StandIn::start().unwrap();
    let oauth = OAuth { provider: OAuthProvider::Microsoft, client_id: "client-of-the-test".into(), ..Default::default() };
    let tokens = refresh_for_at(&endpoints(&server), &oauth, "any-refresh", Resource::GraphMail).unwrap();
    assert_eq!((tokens.access.as_str(), tokens.refresh.as_str(), tokens.email.as_str()), (TOKEN, REFRESH, "owner@example.org"));
    assert_eq!(tokens.name, "Stand-in Owner");
}

#[test]
fn a_form_that_is_no_grant_it_takes_is_refused_as_the_endpoint_refuses() {
    let server = StandIn::start().unwrap();
    let form = "application/x-www-form-urlencoded";
    let cases = [
        (form, "grant_type=password&client_id=c&username=u", "unsupported_grant_type"),
        (form, "grant_type=authorization_code&client_id=c&code=x", "invalid_grant"),
        (form, "grant_type=refresh_token&client_id=c", "invalid_grant"),
        (form, "grant_type=refresh_token&refresh_token=r", "invalid_request"),
        ("application/json", "{}", "invalid_request"),
    ];
    for (kind, body, error) in cases {
        let (status, answer) = post(&server, kind, body);
        assert_eq!(status, 400, "{body}");
        assert!(answer.contains(&format!("\"error\":\"{error}\"")), "{body}: {answer}");
    }
    let (status, answer) = post(&server, form, "grant_type=authorization_code&client_id=c&code=x&code_verifier=v%2Dw");
    assert_eq!(status, 200, "{answer}");
}

#[test]
fn a_process_is_routed_to_a_stand_in_on_loopback_and_nowhere_else() {
    // (Only refusals: one that took would route every test after it in this process.)
    for base in [
        "https://graph.microsoft.com",
        "http://example.org:8780",
        "http://127.0.0.1",
        "http://127.0.0.1:0",
        "http://127.0.0.1:port",
        "127.0.0.1:8780",
    ] {
        assert!(!route_this_process(base), "{base}");
    }
}
