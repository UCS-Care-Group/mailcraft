// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's sending, on the stand-in alone. Nothing here leaves this machine.

use std::io::{Read, Write};
use std::net::TcpStream;

use mailcraft_charset::base64::STANDARD;
use serde_json::{Value, json};

use crate::graph::standin::{StandIn, TOKEN};

fn send(server: &StandIn, target: &str, kind: &str, body: &str) -> u16 {
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    write!(
        stream,
        "POST {target} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut all = String::new();
    stream.read_to_string(&mut all).unwrap();
    all.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0)
}

#[test]
fn send_mail_files_a_copy_in_sent_items_and_send_takes_drafts_only() {
    let server = StandIn::start().unwrap();
    let json = json!({"message": {"subject": "By JSON", "toRecipients": [{"emailAddress": {"address": "kit@example.org"}}]}});
    assert_eq!(send(&server, "/v1.0/me/sendMail", "application/json", &json.to_string()), 202);
    let mime = "To: kit@example.org\r\nSubject: By MIME\r\nIn-Reply-To: <a@example.org>\r\n\r\nWords.\r\n";
    assert_eq!(send(&server, "/v1.0/me/sendMail", "text/plain", &STANDARD.encode(mime)), 202);
    let nobody = json!({"message": {"subject": "Nobody"}});
    assert_eq!(send(&server, "/v1.0/me/sendMail", "application/json", &nobody.to_string()), 400);
    let sent_items = server.state().well_known("sentitems");
    assert_eq!(server.state().mailbox.counts(&sent_items).0, 2);
    let sent: Vec<(String, String)> = server.state().sent.iter().map(|s| (s.subject.clone(), s.in_reply_to.clone())).collect();
    assert_eq!(sent, [("By JSON".to_string(), String::new()), ("By MIME".to_string(), "<a@example.org>".to_string())]);

    // A message that isn't a draft isn't sent; a draft is, and leaves Drafts.
    let inbox = server.state().well_known("inbox");
    let received = server.state().mailbox.add(&inbox, "Received", 1_790_000_000);
    assert_eq!(send(&server, &format!("/v1.0/me/messages/{received}/send"), "application/json", ""), 400);
    let draft = json!({"subject": "Draft", "toRecipients": [{"emailAddress": {"address": "kit@example.org"}}]});
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let body = draft.to_string();
    write!(stream, "POST /v1.0/me/messages HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
    let mut all = String::new();
    stream.read_to_string(&mut all).unwrap();
    let made: Value = serde_json::from_str(all.split_once("\r\n\r\n").unwrap().1).unwrap();
    let id = made["id"].as_str().unwrap();
    assert_eq!(send(&server, &format!("/v1.0/me/messages/{id}/send"), "application/json", ""), 202);
    let drafts = server.state().well_known("drafts");
    assert_eq!(server.state().mailbox.counts(&drafts).0, 0);
    assert_eq!(server.state().mailbox.counts(&sent_items).0, 3);
    assert_eq!(server.state().sent.last().map(|s| s.how.clone()).as_deref(), Some("send"));
}
