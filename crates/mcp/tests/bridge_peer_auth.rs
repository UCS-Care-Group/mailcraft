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
/// challenge with `proof(nonce)` and everything else with ok.
fn listener(proof: impl Fn(&str) -> Value + Send + 'static) -> (u16, Receiver<String>) {
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
                    json!({"ok": true, "result": "done"})
                };
                let mut reply = reply;
                reply["id"] = req["id"].clone();
                if writeln!(out, "{reply}").is_err() {
                    return;
                }
            }
        }
    });
    (port, rx)
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
