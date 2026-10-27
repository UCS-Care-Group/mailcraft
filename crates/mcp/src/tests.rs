// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! An MCP client doing realistic tasks through the protocol only, checking results via MCP.

use serde_json::{Value, json};

use crate::{Headless, Server};

fn call(s: &mut Server, id: u64, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
    let r = s.handle_line(&line).expect("reply");
    serde_json::from_str(&r).expect("json")
}

fn tool(s: &mut Server, name: &str, args: Value) -> Value {
    let r = call(s, 9, "tools/call", json!({"name": name, "arguments": args}));
    let res = &r["result"];
    assert_eq!(res["isError"], false, "{name}: {res}");
    let text = res["content"][0]["text"].as_str().unwrap_or("null");
    serde_json::from_str(text).unwrap_or(Value::String(text.to_string()))
}

#[test]
fn lifecycle_and_tools() {
    let mut s = Server::new(Box::new(Headless::default()));
    let init = call(&mut s, 1, "initialize", json!({"protocolVersion": "2025-06-18"}));
    assert_eq!(init["result"]["serverInfo"]["name"], "mailcraft");
    assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    let tools = call(&mut s, 2, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().len() >= 15);
    let bad = call(&mut s, 3, "nope", json!({}));
    assert!(bad["error"].is_object());
    assert!(s.handle_line("{not json").unwrap().contains("parse error"));
}

/// A frame's text without the frame.
fn plain(v: &Value) -> String {
    v.as_str().unwrap_or("").replace(crate::tools::UNTRUSTED_OPEN, "").replace(crate::tools::UNTRUSTED_CLOSE, "")
}

#[test]
fn agent_triages_mail_and_books_a_meeting() {
    // The owner allowed sending, and a mailbox of the agent's own.
    let allow = vec![mailcraft_engine::agent::Class::Send, mailcraft_engine::agent::Class::Mailbox];
    let mut s = Server::new(Box::new(Headless::new(mailcraft_engine::Session::empty(), allow)));
    tool(&mut s, "open_mailbox", json!({"sample": true}));
    let folders = tool(&mut s, "list_folders", json!({}));
    assert_eq!(folders["accounts"][0]["folders"][0]["name"], "Inbox");
    let unread = tool(&mut s, "list_messages", json!({"folder": "Inbox", "query": "is:unread"}));
    let messages = unread["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    let id = messages.iter().find(|m| m["subject"].as_str().unwrap().contains("roadmap")).unwrap()["id"].as_u64().unwrap();
    let msg = tool(&mut s, "read_message", json!({"id": id}));
    assert!(msg["body"].as_str().unwrap().contains("Atlas estimate"));
    // Flag it, file a to-do and put time on the calendar, in one batch.
    tool(
        &mut s,
        "batch",
        json!({"commands": [
            {"command": "mail.flag", "params": {"id": id}},
            {"command": "tasks.fromMessage", "params": {"id": id}},
            {"command": "calendar.new", "params": {"title": "Prepare the Atlas estimate", "start": "2030-01-07T10:00", "end": "2030-01-07T11:00"}},
        ]}),
    );
    let events = tool(&mut s, "list_events", json!({"from": "2030-01-07", "to": "2030-01-07"}));
    assert!(events["events"].as_array().unwrap().iter().any(|e| plain(&e["title"]) == "Prepare the Atlas estimate"));
    let tasks = tool(&mut s, "list_tasks", json!({"filter": "active"}));
    assert!(tasks["tasks"].as_array().unwrap().iter().any(|t| t["message"] == id));
    // Reply through the commands a person would use.
    tool(&mut s, "execute", json!({"command": "mail.reply", "params": {"id": id}}));
    tool(&mut s, "execute", json!({"command": "compose.set", "params": {"body": "Thursday works. I'll bring a range."}}));
    let sent = tool(&mut s, "execute", json!({"command": "compose.send"}));
    let reply = tool(&mut s, "read_message", json!({"id": sent["id"]}));
    assert_eq!(plain(&reply["subject"]), "RE: Q4 roadmap review moved to Thursday");
    assert!(tool(&mut s, "read_message", json!({"id": id}))["answered"].as_bool().unwrap());
    // A new message, and a bad one that leaves nothing open.
    tool(&mut s, "send_message", json!({"to": "maya@lumen.example", "subject": "Hello", "body": "Hi Maya"}));
    let r = call(&mut s, 10, "tools/call", json!({"name": "send_message", "arguments": {"to": "not an address", "subject": "x", "body": "y"}}));
    assert_eq!(r["result"]["isError"], true);
    let r = call(&mut s, 11, "tools/call", json!({"name": "execute", "arguments": {"command": "compose.send"}}));
    assert_eq!(r["result"]["isError"], true);
    assert_eq!(tool(&mut s, "list_contacts", json!({"query": "priya"}))["contacts"].as_array().unwrap().len(), 1);
    // The window's tools say what they need when there is no window.
    let r = call(&mut s, 12, "tools/call", json!({"name": "screenshot", "arguments": {}}));
    assert_eq!(r["result"]["isError"], true);
    assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("desktop app"));
    let p = tool(&mut s, "parity", json!({}));
    assert!(p["percent"].as_f64().unwrap() > 40.0);
    // Headless, the mailbox is open from the start (#519).
    assert_eq!(tool(&mut s, "status", json!({}))["mailbox"], "open");
}

#[test]
fn resources() {
    let mut s = Server::new(Box::new(Headless::default()));
    let l = call(&mut s, 1, "resources/list", json!({}));
    assert_eq!(l["result"]["resources"].as_array().unwrap().len(), 2);
    let r = call(&mut s, 2, "resources/read", json!({"uri": "mailcraft://mailbox"}));
    assert!(r["result"]["contents"][0]["text"].as_str().unwrap().contains("accounts"));
}
