// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
use serde_json::json;

use crate::Session;
use crate::cmd::base64_decode;
use crate::print::{Labels, Request};

fn sample() -> Session {
    Session::new(crate::sample::mailbox(1_791_993_600, 0))
}

fn titled(s: &Session, subject: &str) -> u64 {
    s.mailbox.messages.values().find(|m| m.subject == subject).map(|m| m.id).unwrap()
}

fn pdf_of(r: &serde_json::Value) -> Vec<u8> {
    base64_decode(r["data"].as_str().unwrap()).unwrap()
}

#[test]
fn save_as_pdf_prints_the_selected_message() {
    let mut s = sample();
    let id = titled(&s, "Your October invoice is ready");
    s.sel.messages = vec![id];
    let r = s.run("file.saveAsPdf", &json!({})).unwrap();
    assert_eq!(r["name"], "Your October invoice is ready.pdf");
    assert_eq!(r["pages"], 1);
    assert!(pdf_of(&r).starts_with(b"%PDF-"));
}

#[test]
fn print_takes_the_messages_named_each_on_its_own_page() {
    let mut s = sample();
    let ids = [titled(&s, "Welcome to the Atlas project"), titled(&s, "Lunch next week?")];
    let r = s.run("file.print", &json!({"ids": ids})).unwrap();
    assert_eq!(r["messages"], 2);
    assert_eq!(r["pages"], 2);
    let docs = s.print_docs(&Request { ids: ids.to_vec(), ..Request::default() }).unwrap();
    assert_eq!(docs[0].title, "Welcome to the Atlas project");
    assert_eq!(docs[0].fields[0].0, "From:");
    assert!(docs[0].fields.iter().any(|(label, value)| label == "Sent:" && value.contains("2026")), "{:?}", docs[0].fields);
}

#[test]
fn the_window_s_labels_and_paper_are_used() {
    let mut s = sample();
    let id = titled(&s, "Lunch next week?");
    let labels = json!({"from": "Von:", "sent": "Gesendet:", "to": "An:", "page": "Seite {n} von {m}"});
    let docs = s.print_docs(&Request { ids: vec![id], labels: Labels::from_params(Some(&labels)), paper: None }).unwrap();
    assert_eq!(docs[0].fields[0].0, "Von:");
    let r = s.run("file.saveAsPdf", &json!({"id": id, "paper": "letter", "labels": labels})).unwrap();
    let text = String::from_utf8_lossy(&pdf_of(&r)).into_owned();
    assert!(text.contains("/MediaBox [0 0 612 792]"));
    assert!(s.run("file.saveAsPdf", &json!({"id": id, "paper": "a3"})).is_err());
}

#[test]
fn nothing_selected_or_too_much_is_said() {
    let mut s = sample();
    s.sel.messages.clear();
    s.sel.focus_row = None;
    assert!(s.run("file.print", &json!({})).is_err());
    assert!(s.run("file.print", &json!({"ids": [999_999]})).is_err());
    let many: Vec<u64> = (0..101).map(|_| titled(&s, "Lunch next week?")).collect();
    assert!(s.run("file.print", &json!({"ids": many})).is_err());
}

#[test]
fn printing_changes_nothing() {
    let mut s = sample();
    let id = titled(&s, "Lunch next week?");
    let before = serde_json::to_string(&s.mailbox).unwrap();
    s.run("file.print", &json!({"id": id})).unwrap();
    assert_eq!(serde_json::to_string(&s.mailbox).unwrap(), before);
}
