// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `read_message` says what the account's own server wrote of a message's SPF, DKIM and DMARC
//! checks (#1472), and `list_commands` finds `mail.viewSource` for `header`. Synthetic data.
use mailcraft_engine::Session;
use mailcraft_engine::model::{Security, Server};
use serde_json::{Value, json};

use crate::Headless;
use crate::tools::{Policy, call_tool};

const OURS: &str = "mx.example.org; spf=pass smtp.mailfrom=sender.example; dkim=pass header.d=sender.example; dmarc=fail header.from=sender.example";
const FORGED: &str = "mx.example.org; spf=fail; dkim=fail; dmarc=pass";
const STRANGER: &str = "mx.evil.example; spf=pass; dkim=pass header.d=sender.example; dmarc=pass";

fn read(h: &mut Headless, id: u64) -> Value {
    let r = call_tool(h, &Policy::default(), "read_message", &json!({"id": id}));
    serde_json::from_str(r.content[0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn read_message_gives_the_topmost_authentication_results_only() {
    let mut session = Session::new(mailcraft_engine::sample::mailbox(1_791_993_600, 0));
    let account = session.mailbox.accounts[0].id;
    session.mailbox.accounts[0].incoming = Some(Server { host: "imap.example.org".into(), port: 993, security: Security::Tls });
    let ids: Vec<u64> = session.mailbox.messages.keys().copied().take(3).collect();
    let cases: [&[&str]; 3] = [&[OURS, FORGED], &[STRANGER, OURS], &[]];
    for (id, headers) in ids.iter().zip(cases) {
        let m = std::sync::Arc::make_mut(session.mailbox.messages.get_mut(id).unwrap());
        m.uid = 5;
        m.account = account;
        m.headers = headers.iter().map(|v| ("Authentication-Results".to_string(), (*v).to_string())).collect();
    }
    let mut h = Headless::new(session, Vec::new());
    let v = read(&mut h, ids[0]);
    assert_eq!(v["authentication"], json!({"spf": "pass", "dkim": "pass", "dkimDomain": "sender.example", "dmarc": "fail"}));
    // A header another server wrote, and none at all: no field, no guess.
    assert!(read(&mut h, ids[1]).get("authentication").is_none());
    assert!(read(&mut h, ids[2]).get("authentication").is_none());
}

#[test]
fn read_message_names_view_source_for_the_full_headers() {
    let defs = crate::tools::tool_definitions();
    let read = defs.as_array().unwrap().iter().find(|t| t["name"] == "read_message").unwrap();
    assert!(read["description"].as_str().unwrap().contains("mail.viewSource"));
}

#[test]
fn list_commands_finds_view_source_for_header() {
    let mut h = Headless::default();
    for query in ["header", "HEADER", "viewSource", "view source"] {
        let r = call_tool(&mut h, &Policy::default(), "list_commands", &json!({"query": query}));
        let v: Value = serde_json::from_str(r.content[0]["text"].as_str().unwrap()).unwrap();
        assert!(v.as_array().unwrap().iter().any(|c| c["id"] == "mail.viewSource"), "{query}");
    }
    let r = call_tool(&mut h, &Policy::default(), "list_commands", &json!({"query": "no-such-thing-xyz"}));
    assert_eq!(r.content[0]["text"], "[]");
}
