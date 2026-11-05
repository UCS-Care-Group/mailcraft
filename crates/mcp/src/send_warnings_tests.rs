// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Send warnings without the window (#509): nobody can be asked, so a send with one is refused
//! unless the call says to send anyway (`"confirmed": true`), and says why as data.
//!
//! The people and domains are invented.

use mailcraft_engine::Session;
use mailcraft_engine::agent::Class;
use serde_json::{Value, json};

use crate::{Headless, Server};

fn call(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("reply")).expect("json")
}

fn tool(s: &mut Server, name: &str, args: Value) -> (bool, String) {
    let r = call(s, "tools/call", json!({"name": name, "arguments": args}));
    (r["result"]["isError"] == true, r["result"]["content"][0]["text"].as_str().unwrap_or("").to_string())
}

/// Avery's local account at Lumen, who keeps Lin at Harbor Works as a contact; sending allowed.
fn server() -> Server {
    let mut s = Session::empty();
    s.run("account.add", &json!({"email": "avery@lumen.example", "displayName": "Avery Quinn"})).unwrap();
    s.run("people.new", &json!({"displayName": "Lin Zhao", "email": "lin.zhao@harborworks.example"})).unwrap();
    Server::new(Box::new(Headless::new(s, vec![Class::Send])))
}

#[test]
fn a_headless_send_with_a_warning_is_refused_unless_it_says_send_anyway() {
    let mut s = server();
    let message = json!({"to": "lin.zhao@harborwork.example", "subject": "Invoice", "body": "Attached"});
    let (failed, text) = tool(&mut s, "send_message", message.clone());
    assert!(
        failed && text.contains("lookalikeRecipient lin.zhao@harborwork.example (like harborworks.example)") && text.contains("\"confirmed\": true"),
        "{text}"
    );
    let (failed, text) = tool(&mut s, "list_messages", json!({"folder": "Sent Items"}));
    assert!(!failed && !text.contains("Invoice"), "nothing went: {text}");
    // Said: it goes.
    let mut anyway = message;
    anyway["confirmed"] = json!(true);
    let (failed, text) = tool(&mut s, "send_message", anyway);
    assert!(!failed, "{text}");
    // The same through `execute` (harborwork.example is written to now: another lookalike).
    tool(&mut s, "execute", json!({"command": "mail.new", "params": {"to": "pat@harborwerks.example", "subject": "Hi"}}));
    let (failed, text) = tool(&mut s, "execute", json!({"command": "compose.send"}));
    assert!(failed && text.contains("lookalikeRecipient pat@harborwerks.example"), "{text}");
    let (failed, text) = tool(&mut s, "execute", json!({"command": "compose.send", "params": {"confirmed": true}}));
    assert!(!failed, "{text}");
}

#[test]
fn a_headless_send_without_a_warning_goes_as_before() {
    let mut s = server();
    let (failed, text) = tool(&mut s, "send_message", json!({"to": "lin.zhao@harborworks.example", "subject": "Hello", "body": "Hi"}));
    assert!(!failed, "{text}");
}
