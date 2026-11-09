// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The bridge makes the program on the control port prove it holds the key before it sends the
//! key or any request (AU2-V6): a listener that only took the port gets neither.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use mailcraft_mcp::{Backend, Remote};
use serde_json::{Value, json};

const KEY: &str = "feedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedface";

/// A listener on a loopback port. Every line it receives goes to the channel; it answers a
/// challenge with `proof(nonce)` and everything else with `other`.
fn listener_with(
    proof: impl Fn(&str) -> Value + Send + 'static,
    other: impl Fn(&Value) -> Value + Send + 'static,
) -> (u16, Receiver<String>) {
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = l.local_addr().unwrap().port();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for stream in l.incoming().flatten() {
            let mut out = stream.try_clone().unwrap();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let _ = tx.send(line.clone());
                let req: Value = serde_json::from_str(&line).unwrap_or_default();
                let reply = if req["method"] == "auth.challenge" {
                    proof(req["params"]["nonce"].as_str().unwrap_or(""))
                } else {
                    other(&req)
                };
                let mut reply = reply;
                reply["id"] = req["id"].clone();
                // A failed write keeps the listener: the retry reconnects, and the next
                // connection must be answered too, so the error stays the size limit's.
                if writeln!(out, "{reply}").is_err() {
                    break;
                }
            }
        }
    });
    (port, rx)
}

/// A listener that answers a challenge with `proof(nonce)` and everything else with ok.
fn listener(proof: impl Fn(&str) -> Value + Send + 'static) -> (u16, Receiver<String>) {
    listener_with(proof, |_| json!({"ok": true, "result": "done"}))
}

fn seen(rx: &Receiver<String>) -> Vec<String> {
    std::thread::sleep(Duration::from_millis(200));
    rx.try_iter().collect()
}

fn refuses_and_sends_nothing(proof: impl Fn(&str) -> Value + Send + 'static) {
    let (port, rx) = listener(proof);
    let mut r = Remote::connect_with_key(&format!("127.0.0.1:{port}"), KEY).unwrap();
    let e = r.call("engine.execute", json!({"command": "mail.flag"})).unwrap_err();
    assert!(e.contains("did not prove"), "{e}");
    let lines = seen(&rx);
    assert!(!lines.is_empty() && lines.iter().all(|l| l.contains("auth.challenge")), "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains(KEY) && !l.contains("engine.execute")), "{lines:?}");
}

#[test]
fn a_listener_that_cannot_prove_the_key_gets_no_key_and_no_request() {
    // Says ok with a made-up proof.
    refuses_and_sends_nothing(|_| json!({"ok": true, "result": {"proof": "00".repeat(32)}}));
    // Proves with some other key (another run's, or a guess).
    refuses_and_sends_nothing(|n| json!({"ok": true, "result": {"proof": mailcraft_control_key::proof::proof("another-key", n)}}));
    // Does not know the method.
    refuses_and_sends_nothing(|_| json!({"ok": false, "error": "unauthorized"}));
    // Says ok and nothing else.
    refuses_and_sends_nothing(|_| json!({"ok": true, "result": {}}));
}

#[test]
fn the_app_that_proves_the_key_is_sent_the_key_and_the_request() {
    let (port, rx) = listener(|n| json!({"ok": true, "result": {"proof": mailcraft_control_key::proof::proof(KEY, n)}}));
    let mut r = Remote::connect_with_key(&format!("127.0.0.1:{port}"), KEY).unwrap();
    assert_eq!(r.call("mailbox.inspect", json!({})).unwrap(), json!("done"));
    let lines = seen(&rx);
    assert!(lines[0].contains("auth.challenge") && !lines[0].contains(KEY), "{lines:?}");
    let second: Value = serde_json::from_str(&lines[1]).unwrap();
    assert_eq!((second["method"].as_str(), second["key"].as_str()), (Some("mailbox.inspect"), Some(KEY)));
}

#[test]
fn an_over_cap_reply_is_refused_and_the_key_is_never_sent() {
    // A peer that answers the challenge with an unbounded line does not choose how much the
    // bridge buffers: the reply is refused at the cap, the connection is closed, and the key
    // and any request are never sent to it.
    let (port, rx) = listener(|_| json!({"ok": true, "result": {"proof": "x".repeat(2 << 20)}}));
    let mut r = Remote::connect_with_key(&format!("127.0.0.1:{port}"), KEY).unwrap();
    let e = r.call("engine.execute", json!({"command": "mail.flag"})).unwrap_err();
    // The listener stays up through the retry, so the error stays the size limit's, not a
    // connection failure that would mask a cap regression.
    assert!(e.contains("reply too long"), "{e}");
    let lines = seen(&rx);
    assert!(!lines.is_empty() && lines.iter().all(|l| l.contains("auth.challenge")), "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains(KEY) && !l.contains("engine.execute")), "{lines:?}");
}

#[test]
fn a_screenshot_sized_reply_from_the_proved_app_is_taken() {
    // The proved app answers a method with a reply past the strict line cap: `ui.screenshot`
    // answers the whole window as base64, and a 4K window's PNG is over 1 MiB of base64. The
    // bridge still takes it, bounded: the cap for a proved app is wider, not gone.
    let (port, _rx) = listener_with(
        |n| json!({"ok": true, "result": {"proof": mailcraft_control_key::proof::proof(KEY, n)}}),
        |_| json!({"ok": true, "result": {"data": "x".repeat((3 << 20) / 4 * 3)}}),
    );
    let mut r = Remote::connect_with_key(&format!("127.0.0.1:{port}"), KEY).unwrap();
    let reply = r.call("ui.screenshot", json!({})).unwrap();
    assert_eq!(reply["ok"], Value::Null, "{reply}");
    let data = reply["data"].as_str().unwrap_or_default();
    assert!(data.len() > (1 << 20), "a screenshot-sized reply came through whole");
}

#[test]
fn an_authenticated_reply_over_the_reply_cap_is_refused() {
    // Even the proved app does not choose how much the bridge buffers: a reply past the wide
    // cap is refused, the connection is closed, and the error names the size.
    let (port, rx) = listener_with(
        |n| json!({"ok": true, "result": {"proof": mailcraft_control_key::proof::proof(KEY, n)}}),
        |_| json!({"ok": true, "result": {"data": "x".repeat(1 << 24)}}),
    );
    let mut r = Remote::connect_with_key(&format!("127.0.0.1:{port}"), KEY).unwrap();
    let e = r.call("ui.screenshot", json!({})).unwrap_err();
    assert!(e.contains("reply too long"), "{e}");
    let lines = seen(&rx);
    assert!(lines.iter().any(|l| l.contains(KEY)), "{lines:?}");
}
