// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The control port proves it holds the key to the MCP bridge before the bridge sends it (AU2-V6).

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use mailcraft_control_key::proof;
use serde_json::{Value, json};

use crate::control_server::serve_on;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn boot() -> (u16, std::sync::mpsc::Receiver<mailcraft_ui_egui::ControlRequest>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    (port, serve_on(listener, egui::Context::default(), KEY.to_string(), Duration::from_secs(30), false))
}

fn call(port: u16, line: &str) -> (Value, bool) {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    writeln!(&s, "{line}").unwrap();
    let mut reply = String::new();
    r.read_line(&mut reply).unwrap();
    let mut rest = String::new();
    let closed = matches!(r.read_line(&mut rest), Ok(0));
    (serde_json::from_str(&reply).unwrap_or_default(), closed)
}

#[test]
fn the_app_answers_a_challenge_with_a_proof_of_the_key() {
    let (port, rx) = boot();
    let nonce = proof::new_nonce().unwrap();
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    writeln!(&s, "{}", json!({"id": 0, "method": "auth.challenge", "params": {"nonce": nonce}})).unwrap();
    let mut line = String::new();
    r.read_line(&mut line).unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["ok"], true);
    let given = v["result"]["proof"].as_str().unwrap();
    assert!(proof::proof_ok(KEY, &nonce, given));
    assert!(!proof::proof_ok(KEY, &proof::new_nonce().unwrap(), given), "bound to the nonce");
    assert!(!line.contains(KEY), "the key itself is never sent");
    // It authorised nothing: a request without the key is still refused.
    line.clear();
    writeln!(&s, r#"{{"id":1,"method":"mailbox.inspect"}}"#).unwrap();
    r.read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["error"], "unauthorized");
    assert!(rx.try_recv().is_err(), "nothing reached the UI");
}

#[test]
fn a_bad_nonce_is_refused_and_closed() {
    let (port, rx) = boot();
    for params in [json!({}), json!({"nonce": 5}), json!({"nonce": "zz"}), json!({"nonce": "a".repeat(129)}), json!({"nonce": "not hex at all!!"})] {
        let (v, closed) = call(port, &json!({"id": 0, "method": "auth.challenge", "params": params}).to_string());
        assert_eq!(v["ok"], false, "{params}");
        assert!(v["result"].is_null() && closed, "{params}");
    }
    assert!(rx.try_recv().is_err());
}
