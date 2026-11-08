// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The stdio stream is read within bounds (AU3-R4): a line over 1 MiB is answered with an error,
//! its rest is read past, and the next line is served; what an error gives back of the client's
//! own words is short.

use std::io::Cursor;

use serde_json::{Value, json};

use crate::server::MAX_LINE;
use crate::{Headless, Server};

fn replies(input: Vec<u8>) -> Vec<Value> {
    let mut s = Server::new(Box::new(Headless::default()));
    let mut out = Vec::new();
    s.serve(Cursor::new(input), &mut out).unwrap();
    String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn an_overlong_line_is_answered_and_the_next_one_served() {
    let big = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"ping","params":{{"pad":"{}"}}}}"#, "x".repeat(2 << 20));
    let mut input = big.into_bytes();
    input.push(b'\n');
    input.extend_from_slice(br#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
    input.push(b'\n');
    let r = replies(input);
    assert_eq!(r.len(), 2, "{r:?}");
    assert_eq!(r[0]["error"]["code"], -32600);
    assert!(r[0]["error"]["message"].as_str().unwrap().contains("line too long"));
    assert!(r[0].to_string().len() < 300);
    assert_eq!(r[1], json!({"jsonrpc": "2.0", "id": 2, "result": {}}));
}

#[test]
fn a_line_at_the_bound_is_served_and_one_past_it_is_not() {
    let line = |pad: usize| {
        let head = r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":{"pad":""#;
        let tail = r#""}}"#;
        let mut l = format!("{head}{}{tail}", "x".repeat(pad - head.len() - tail.len()));
        l.push('\n');
        l.into_bytes()
    };
    // The newline counts: `MAX_LINE` bytes with it.
    let r = replies(line(MAX_LINE - 1));
    assert_eq!(r[0]["result"], json!({}), "{}", r[0]);
    let mut input = line(MAX_LINE + 1);
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
    let r = replies(input);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["error"]["code"], -32600);
    assert_eq!(r[1]["id"], 2);
    // A last line without its newline, too long: answered, and the stream ends.
    let r = replies(vec![b'x'; 3 << 20]);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["error"]["code"], -32600);
}

#[test]
fn a_line_that_is_not_utf8_is_answered_and_serving_goes_on() {
    let mut input = b"\xff\xfe\n".to_vec();
    input.extend_from_slice(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n");
    let r = replies(input);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0]["error"]["code"], -32700);
    assert_eq!(r[1]["id"], 2);
}

#[test]
fn errors_give_back_little_of_the_clients_words() {
    let mut s = Server::new(Box::new(Headless::default()));
    let long = "m".repeat(100_000);
    for (method, params) in [(long.as_str(), json!({})), ("resources/read", json!({"uri": long}))] {
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        let r: Value = serde_json::from_str(&s.handle_line(&line).unwrap()).unwrap();
        let message = r["error"]["message"].as_str().unwrap();
        assert!(message.chars().count() < 260, "{} characters", message.len());
    }
    // A tool's name and a command's id, too.
    for (name, args) in [(long.as_str(), json!({})), ("execute", json!({"command": long}))] {
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": args}}).to_string();
        let r: Value = serde_json::from_str(&s.handle_line(&line).unwrap()).unwrap();
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.chars().count() < 260, "{} characters", text.len());
    }
    assert_eq!(crate::server::clip("short"), "short");
    assert_eq!(crate::server::clip(&"é".repeat(300)).chars().count(), 201);
}
