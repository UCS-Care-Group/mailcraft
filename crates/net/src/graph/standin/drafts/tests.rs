// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stand-in's drafts and attachments, on the stand-in alone. Nothing here leaves this
//! machine; every message is invented.

use std::io::{Read, Write};
use std::net::TcpStream;

use mailcraft_charset::base64::STANDARD;
use serde_json::{Value, json};

use crate::graph::standin::{StandIn, TOKEN};

const NOW: i64 = 1_790_000_000;
const MARKER: &str = "String {6d1c4e3a-8f2b-4c5d-9e0a-1b2c3d4e5f60} Name MailCraftMarker";

/// One request: the status (0 when the connection closed without an answer), the headers (names
/// in lower case) and the body. `auth` false sends no Authorization; `kind` is the body's type.
fn raw(server: &StandIn, method: &str, target: &str, kind: &str, body: &[u8], extra: &str, auth: bool) -> (u16, Vec<(String, String)>, String) {
    let target = target.strip_prefix(&server.url()).unwrap_or(target);
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let auth = if auth { format!("Authorization: Bearer {TOKEN}\r\n") } else { String::new() };
    let typed = if body.is_empty() { String::new() } else { format!("Content-Type: {kind}\r\n") };
    let head = format!(
        "{method} {target} HTTP/1.1\r\nHost: x\r\n{auth}Prefer: IdType=\"ImmutableId\"\r\n{typed}{extra}Content-Length: {}\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).unwrap();
    stream.write_all(body).unwrap();
    let mut all = String::new();
    stream.read_to_string(&mut all).unwrap();
    let Some((head, body)) = all.split_once("\r\n\r\n") else { return (0, Vec::new(), all) };
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    (status, headers, body.to_string())
}

fn send(server: &StandIn, method: &str, target: &str, body: Option<&Value>) -> (u16, Value) {
    let body = body.map(Value::to_string).unwrap_or_default();
    let (status, _, answer) = raw(server, method, target, "application/json", body.as_bytes(), "", true);
    (status, serde_json::from_str(&answer).unwrap_or(Value::Null))
}

fn marker(value: &str) -> Value {
    json!([{"id": MARKER, "value": value}])
}

fn by_marker(server: &StandIn, value: &str) -> Vec<Value> {
    let filter = format!("singleValueExtendedProperties/any(ep: ep/id eq '{MARKER}' and ep/value eq '{value}')");
    let query = crate::http::form(&[("$filter", &filter), ("$select", "id,parentFolderId,changeKey")]);
    let (status, answer) = send(server, "GET", &format!("/v1.0/me/mailFolders/drafts/messages?{query}"), None);
    assert_eq!(status, 200, "{answer}");
    answer["value"].as_array().unwrap().clone()
}

#[test]
fn a_draft_made_from_json_is_in_drafts_and_found_by_its_marker() {
    let server = StandIn::start().unwrap();
    let draft = json!({
        "subject": "Plans for Friday",
        "body": {"contentType": "html", "content": "<p>Shall we meet?</p>"},
        "toRecipients": [{"emailAddress": {"address": "kit@example.org", "name": "Kit"}}],
        "bccRecipients": [{"emailAddress": {"address": "lee@example.org"}}],
        "importance": "high",
        "singleValueExtendedProperties": marker("mailcraft-draft-1"),
        "internetMessageHeaders": [{"name": "x-mailcraft-test", "value": "1"}],
    });
    let (status, answer) = send(&server, "POST", "/v1.0/me/messages", Some(&draft));
    assert_eq!(status, 201, "{answer}");
    assert_eq!((answer["isDraft"].clone(), answer["subject"].clone()), (json!(true), json!("Plans for Friday")));
    assert_eq!(answer["parentFolderId"], json!(server.state().well_known("drafts")));
    assert_eq!(answer["bccRecipients"][0]["emailAddress"]["address"], "lee@example.org");
    let found = by_marker(&server, "mailcraft-draft-1");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["id"], answer["id"]);
    assert!(by_marker(&server, "mailcraft-draft-2").is_empty());

    // Headers that aren't x- headers are Graph's to write; a property it hasn't is 400.
    for wrong in [json!({"internetMessageHeaders": [{"name": "In-Reply-To", "value": "<a@b>"}]}), json!({"isFancy": true}), json!({"subject": 3})] {
        let (status, answer) = send(&server, "POST", "/v1.0/me/messages", Some(&wrong));
        assert_eq!(status, 400, "{wrong} → {answer}");
    }
}

#[test]
fn a_draft_made_from_mime_keeps_its_headers() {
    let server = StandIn::start().unwrap();
    let mime = "From: owner@example.org\r\nTo: kit@example.org\r\nSubject: From MIME\r\nIn-Reply-To: <orig@example.org>\r\nMessage-ID: <mine@example.org>\r\nContent-Type: text/plain\r\n\r\nThe words.\r\n";
    let (status, _, answer) = raw(&server, "POST", "/v1.0/me/messages", "text/plain", STANDARD.encode(mime).as_bytes(), "", true);
    assert_eq!(status, 201, "{answer}");
    let state = server.state();
    let made = state.mailbox.messages.values().find(|m| m.subject == "From MIME").unwrap();
    assert!(made.draft && made.in_reply_to == "<orig@example.org>" && made.internet_id == "<mine@example.org>" && made.to == ["kit@example.org"]);
    drop(state);
    let (status, _, _) = raw(&server, "POST", "/v1.0/me/messages", "text/plain", b"not base64 at all!", "", true);
    assert_eq!(status, 400);
}

#[test]
fn an_answer_stays_in_its_conversation_and_a_forward_carries_the_attachments() {
    let server = StandIn::start().unwrap();
    let inbox = server.state().well_known("inbox");
    let source = server.state().mailbox.add(&inbox, "Invoice 42", NOW);
    server.state().mailbox.update(&source, |m| {
        m.attachments.push(super::Attached {
            id: "a1".into(),
            name: "invoice.pdf".into(),
            content_type: "application/pdf".into(),
            bytes: b"%PDF".to_vec(),
            ..Default::default()
        })
    });
    let body = json!({"message": {"singleValueExtendedProperties": marker("mailcraft-draft-r")}});
    let (status, reply) = send(&server, "POST", &format!("/v1.0/me/messages/{source}/createReply"), Some(&body));
    assert_eq!(status, 201, "{reply}");
    let state = server.state();
    let (orig, made) = (state.mailbox.messages[&source].clone(), state.mailbox.messages[reply["id"].as_str().unwrap()].clone());
    drop(state);
    assert_eq!(made.conversation_id(), orig.conversation_id());
    assert_eq!((made.subject.as_str(), made.in_reply_to.as_str()), ("RE: Invoice 42", orig.internet_id.as_str()));
    assert_eq!(made.to, std::slice::from_ref(&orig.from));
    assert!(made.draft && made.attachments.is_empty());
    assert_eq!(by_marker(&server, "mailcraft-draft-r").len(), 1);

    let (status, forward) = send(&server, "POST", &format!("/v1.0/me/messages/{source}/createForward"), None);
    assert_eq!(status, 201, "{forward}");
    let made = server.state().mailbox.messages[forward["id"].as_str().unwrap()].clone();
    assert!(made.subject == "FW: Invoice 42" && made.to.is_empty() && made.attachments.len() == 1);

    // A comment and a body both: 400; an answer to a message that isn't: 404.
    let both = json!({"comment": "Hi", "message": {"body": {"contentType": "text", "content": "x"}}});
    assert_eq!(send(&server, "POST", &format!("/v1.0/me/messages/{source}/createReplyAll"), Some(&both)).0, 400);
    let gone = "AAkALgAAAAAAHYQDEapmEc2byACqAC-EWg0A9999999999AAA=";
    assert_eq!(send(&server, "POST", &format!("/v1.0/me/messages/{gone}/createReply"), None).0, 404);
}

#[test]
fn a_draft_changes_in_place_and_a_received_message_doesnt() {
    let server = StandIn::start().unwrap();
    let (_, made) = send(&server, "POST", "/v1.0/me/messages", Some(&json!({"subject": "First"})));
    let id = made["id"].as_str().unwrap().to_string();
    let change = json!({
        "subject": "Second",
        "body": {"contentType": "text", "content": "Now with words."},
        "toRecipients": [{"emailAddress": {"address": "kit@example.org"}}],
        "ccRecipients": [{"emailAddress": {"address": "lee@example.org"}}],
    });
    let (status, changed) = send(&server, "PATCH", &format!("/v1.0/me/messages/{id}"), Some(&change));
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["id"], json!(id), "changed in place");
    assert_ne!(changed["changeKey"], made["changeKey"]);
    let m = server.state().mailbox.messages[&id].clone();
    assert!(m.subject == "Second" && m.body == "Now with words." && !m.html && m.cc == ["lee@example.org"]);
    // Headers are written when a draft is made only.
    let headers = json!({"internetMessageHeaders": [{"name": "x-a", "value": "1"}]});
    assert_eq!(send(&server, "PATCH", &format!("/v1.0/me/messages/{id}"), Some(&headers)).0, 400);
    let inbox = server.state().well_known("inbox");
    let received = server.state().mailbox.add(&inbox, "Received", NOW);
    assert_eq!(send(&server, "PATCH", &format!("/v1.0/me/messages/{received}"), Some(&json!({"subject": "Mine now"}))).0, 400);
    let bad_to = json!({"toRecipients": [{"emailAddress": {"address": "nobody"}}]});
    assert_eq!(send(&server, "PATCH", &format!("/v1.0/me/messages/{id}"), Some(&bad_to)).0, 400);
}

#[test]
fn attachments_are_added_listed_and_deleted_and_large_ones_go_by_upload_session() {
    let server = StandIn::start().unwrap();
    let (_, made) = send(&server, "POST", "/v1.0/me/messages", Some(&json!({"subject": "With files"})));
    let id = made["id"].as_str().unwrap().to_string();
    let small = json!({"@odata.type": "#microsoft.graph.fileAttachment", "name": "notes.txt", "contentType": "text/plain", "contentBytes": STANDARD.encode(b"Notes.")});
    let (status, added) = send(&server, "POST", &format!("/v1.0/me/messages/{id}/attachments"), Some(&small));
    assert_eq!(status, 201, "{added}");
    let (status, list) = send(&server, "GET", &format!("/v1.0/me/messages/{id}/attachments?$select=id,name,size,isInline"), None);
    assert_eq!(status, 200, "{list}");
    let listed = list["value"].as_array().unwrap();
    assert_eq!((listed.len(), listed[0]["name"].clone()), (1, json!("notes.txt")));
    assert!(listed[0]["size"].as_u64().unwrap() > 6, "Graph's size counts more than the bytes");
    assert!(listed[0].get("contentBytes").is_none());
    let aid = added["id"].as_str().unwrap();
    let (_, one) = send(&server, "GET", &format!("/v1.0/me/messages/{id}/attachments/{aid}"), None);
    assert_eq!(one["contentBytes"], json!(STANDARD.encode(b"Notes.")));
    assert_eq!(send(&server, "DELETE", &format!("/v1.0/me/messages/{id}/attachments/{aid}"), None).0, 204);
    assert_eq!(send(&server, "DELETE", &format!("/v1.0/me/messages/{id}/attachments/{aid}"), None).0, 404);

    // 3 MB or more: not directly, and an upload session for less is refused.
    let big = vec![7u8; 3 * 1024 * 1024 + 100];
    let direct = json!({"@odata.type": "#microsoft.graph.fileAttachment", "name": "big.bin", "contentBytes": STANDARD.encode(&big)});
    assert_eq!(send(&server, "POST", &format!("/v1.0/me/messages/{id}/attachments"), Some(&direct)).0, 413);
    let item = |size: usize| json!({"AttachmentItem": {"attachmentType": "file", "name": "big.bin", "size": size}});
    let (status, small_session) = send(&server, "POST", &format!("/v1.0/me/messages/{id}/attachments/createUploadSession"), Some(&item(1000)));
    assert_eq!((status, small_session["error"]["code"].clone()), (400, json!("ErrorAttachmentSizeShouldNotBeLessThanMinimumSize")));
    let (status, session) = send(&server, "POST", &format!("/v1.0/me/messages/{id}/attachments/createUploadSession"), Some(&item(big.len())));
    assert_eq!(status, 201, "{session}");
    let url = session["uploadUrl"].as_str().unwrap().to_string();
    let piece = |from: usize, to: usize, auth: bool| {
        let range = format!("Content-Range: bytes {from}-{to}/{}\r\n", big.len());
        raw(&server, "PUT", &url, "application/octet-stream", &big[from..=to], &range, auth)
    };
    let half = big.len() / 2;
    assert_eq!(piece(0, half - 1, true).0, 401, "an upload's address carries no Authorization");
    assert_eq!(piece(half, big.len() - 1, false).0, 416, "pieces come in order");
    let (status, _, answer) = piece(0, half - 1, false);
    assert_eq!(status, 200, "{answer}");
    assert_eq!(serde_json::from_str::<Value>(&answer).unwrap()["nextExpectedRanges"], json!([format!("{half}-")]));
    let (status, headers, answer) = piece(half, big.len() - 1, false);
    assert_eq!(status, 201, "{answer}");
    assert!(headers.iter().any(|(n, v)| n == "location" && v.contains("/attachments('")));
    let m = server.state().mailbox.messages[&id].clone();
    assert_eq!((m.attachments.len(), m.attachments[0].bytes.len()), (1, big.len()));
    assert!(m.attachments[0].bytes == big);
}

#[test]
fn an_answer_can_be_lost_after_the_draft_was_made() {
    let server = StandIn::start().unwrap();
    server.state().lose_next.push("POST /v1.0/me/messages".into());
    let body = json!({"subject": "Lost", "singleValueExtendedProperties": marker("mailcraft-draft-lost")});
    let (status, _) = send(&server, "POST", "/v1.0/me/messages", Some(&body));
    assert_eq!(status, 0, "no answer");
    assert_eq!(by_marker(&server, "mailcraft-draft-lost").len(), 1, "but the draft was made");
    // Once only.
    let (status, _) = send(&server, "POST", "/v1.0/me/messages", Some(&json!({"subject": "Kept"})));
    assert_eq!(status, 201);
}
