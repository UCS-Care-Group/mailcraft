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

fn png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image_png(&mut out);
    out.into_inner()
}

fn image_png(out: &mut std::io::Cursor<Vec<u8>>) {
    use std::io::Write;
    // A real PNG is not needed: the engine takes a picture by what its bytes start with, and the
    // print crate decodes it.
    out.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
    out.write_all(&[7u8; 4096]).unwrap();
}

fn with_pictures(s: &mut Session, html: String, parts: Vec<mailcraft_model::Attachment>) -> u64 {
    let inbox = s.mailbox.inbox().unwrap();
    let mut m = mailcraft_model::Message { folder: inbox, subject: "Pictures".into(), ..Default::default() };
    m.body.html = Some(html);
    m.attachments = parts;
    s.mailbox.add_message(m).unwrap()
}

#[test]
fn a_picture_is_taken_once_however_its_address_is_spelled() {
    let mut s = sample();
    let html = r#"<img src="cid:abc@x"><img src="cid:ABC@X"><img src="cid:%61bc@x"><img src="cid:<abc@x>">"#.repeat(500);
    let logo =
        mailcraft_model::Attachment { content_id: "abc@x".into(), inline: true, ..mailcraft_model::Attachment::new("logo.png", "image/png", png()) };
    let id = with_pictures(&mut s, html, vec![logo]);
    let docs = s.print_docs(&Request { ids: vec![id], ..Request::default() }).unwrap();
    let pictures: Vec<_> = docs[0].pictures.values().collect();
    assert_eq!(pictures.len(), 4, "every spelling that names it");
    assert!(pictures.windows(2).all(|w| std::sync::Arc::ptr_eq(w[0], w[1])), "one copy of its bytes");
}

#[test]
fn only_pictures_within_the_caps_are_taken() {
    let mut s = sample();
    let big = mailcraft_model::Attachment {
        content_id: "big@x".into(),
        ..mailcraft_model::Attachment::new(
            "big.png",
            "image/png",
            [b"\x89PNG\r\n\x1a\n".to_vec(), vec![0; mailcraft_print::MAX_PICTURE_BYTES]].concat(),
        )
    };
    let file =
        mailcraft_model::Attachment { content_id: "file@x".into(), ..mailcraft_model::Attachment::new("a.zip", "application/zip", vec![1; 4096]) };
    let id = with_pictures(&mut s, r#"<img src="cid:big@x"><img src="cid:file@x">"#.into(), vec![big, file]);
    let docs = s.print_docs(&Request { ids: vec![id], ..Request::default() }).unwrap();
    assert!(docs[0].pictures.is_empty(), "too large, and no picture");
}

#[test]
fn a_message_not_downloaded_yet_is_refused_not_printed_from_its_preview() {
    let mut s = sample();
    let inbox = s.mailbox.inbox().unwrap();
    let server = mailcraft_model::sync::ServerMessage::default();
    let mut m = mailcraft_model::Message { folder: inbox, subject: "Later".into(), unfetched: true, server: Some(server), ..Default::default() };
    m.body.text = "Only the preview".into();
    let id = s.mailbox.add_message(m).unwrap();
    let e = s.run("file.saveAsPdf", &json!({"id": id})).unwrap_err().to_string();
    assert!(e.contains("isn't on this computer yet"), "{e}");
}

#[test]
fn the_system_s_faces_are_used_unless_the_host_names_others() {
    let mut s = sample();
    s.print_fallbacks = Some(Vec::new());
    assert!(s.print_fonts().unwrap().fallbacks.is_empty());
    s.print_fallbacks = None;
    let system = s.print_fonts().unwrap().fallbacks.len();
    assert_eq!(
        system,
        crate::system_fonts::fallback_paths().iter().filter(|p| mailcraft_print::Face::new(std::fs::read(p).unwrap(), 0).is_some()).count()
    );
}
