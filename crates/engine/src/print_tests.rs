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
    let docs = s.print_docs(&Request { ids: vec![id], labels: Labels::from_params(Some(&labels)), ..Request::default() }).unwrap();
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
    assert!(system <= crate::system_fonts::fallback_paths().len());
    // A group whose first file isn't a face it prints with takes the next file of the group.
    let junk = std::env::temp_dir().join("mailcraft-print-not-a-font.ttf");
    std::fs::write(&junk, b"not a font").unwrap();
    let inter = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fonts/Inter-Regular.ttf");
    assert_eq!(crate::print::fallback_faces(&[vec![junk.clone(), inter.clone()]]).len(), 1);
    assert_eq!(crate::print::fallback_faces(&[vec![junk.clone()]]).len(), 0);
    assert_eq!(crate::print::fallback_faces(&[vec![inter.clone()], vec![inter]]).len(), 1, "one file, once");
    let _ = std::fs::remove_file(junk);
}

fn page_text(s: &Session, request: &Request) -> String {
    let docs = s.print_docs(request).unwrap();
    mailcraft_print::page_text(&docs, &s.print_fonts().unwrap(), &mailcraft_print::Options::default()).unwrap().join("\n")
}

fn calendar(from: &str, to: &str, style: crate::print_views::CalendarStyle) -> Request {
    Request {
        what: crate::print::What::Calendar {
            from: mailcraft_time::Date::parse_iso(from),
            to: mailcraft_time::Date::parse_iso(to),
            style: Some(style),
        },
        ..Request::default()
    }
}

#[test]
fn the_calendar_prints_as_a_month_or_day_by_day() {
    use crate::print_views::CalendarStyle;
    let s = sample();
    let month = page_text(&s, &calendar("2026-10-01", "2026-10-31", CalendarStyle::Month));
    assert!(month.contains("October 2026") && month.contains("Team") && month.contains("Budget"), "{month}");
    let days = page_text(&s, &calendar("2026-10-12", "2026-10-16", CalendarStyle::Agenda));
    for want in ["Monday, October 12, 2026", "9:30 AM – 9:45 AM", "Budget sync", "Room Oak", "Friday, October 16, 2026"] {
        assert!(days.contains(want), "{want:?} missing from:\n{days}");
    }
    assert!(days.find("Monday").unwrap() < days.find("Friday").unwrap());
    let empty = page_text(&s, &calendar("2031-03-01", "2031-03-01", CalendarStyle::Agenda));
    assert!(empty.contains("Nothing planned"), "{empty}");
}

#[test]
fn contacts_and_tasks_print_what_their_lists_show() {
    let mut s = sample();
    let contacts = page_text(&s, &Request { what: crate::print::What::Contacts, ..Request::default() });
    assert!(contacts.contains("Contacts") && contacts.contains("Priya Nair") && contacts.contains("priya@lumen.example"), "{contacts}");
    let tasks = page_text(&s, &Request { what: crate::print::What::Tasks, ..Request::default() });
    for want in ["Tasks", "Subject", "Due", "Importance", "Not completed", "High"] {
        assert!(tasks.contains(want), "{want:?} missing from:\n{tasks}");
    }
    s.view.module = crate::Module::Tasks;
    let r = s.run("file.saveAsPdf", &json!({})).unwrap();
    assert_eq!(r["name"], "Tasks.pdf");
}

#[test]
fn printing_the_module_in_view_and_what_is_refused() {
    let mut s = sample();
    s.view.module = crate::Module::Calendar;
    let r = s.run("file.print", &json!({"labels": {"calendar": "Kalender"}})).unwrap();
    assert_eq!(r["name"], "Kalender.pdf");
    assert!(s.run("file.print", &json!({"what": "notes"})).is_err());
    assert!(s.run("file.print", &json!({"what": "calendar", "style": "year"})).is_err());
    assert!(s.run("file.print", &json!({"what": "calendar", "from": "2026-01-01", "to": "2028-01-01"})).is_err());
    assert!(s.run("file.print", &json!({"what": "calendar", "from": "2026-02-01", "to": "2026-01-01"})).is_err());
    assert!(s.run("file.print", &json!({"what": "contacts", "ids": [999_999]})).is_err());
}

#[test]
fn the_month_in_view_prints_that_month_alone() {
    let mut s = sample();
    s.view.module = crate::Module::Calendar;
    s.view.calendar = crate::CalView::Month;
    s.view.date = mailcraft_time::Date::parse_iso("2026-10-14").unwrap();
    let text = page_text(&s, &Request { what: crate::print::What::Calendar { from: None, to: None, style: None }, ..Request::default() });
    assert!(text.contains("October 2026"), "{text}");
    assert!(!text.contains("September 2026"), "a grid of its own for the days before: {text}");
}

#[test]
fn an_item_over_several_days_is_in_each_of_them_in_the_month() {
    let mut s = sample();
    s.run("calendar.new", &json!({"title": "Holiday week", "start": "2026-09-28", "end": "2026-10-04", "allDay": true})).unwrap();
    let text = page_text(&s, &calendar("2026-10-01", "2026-10-31", crate::print_views::CalendarStyle::Month));
    assert!(text.matches("Holiday week").count() >= 3, "{text}");
}

#[test]
fn ids_name_messages_whatever_is_in_view() {
    let mut s = sample();
    let id = titled(&s, "Lunch next week?");
    s.view.module = crate::Module::Calendar;
    let r = s.run("file.saveAsPdf", &json!({"id": id})).unwrap();
    assert_eq!(r["name"], "Lunch next week_.pdf");
    let e = s.run("file.print", &json!({"what": "contacts", "ids": [0]})).unwrap_err().to_string();
    assert!(e.contains("contact ids"), "{e}");
    assert!(s.run("file.print", &json!({"what": "calendar", "id": id})).is_err());
}

#[test]
fn tasks_arranged_by_categories_print_under_their_categories() {
    let mut s = sample();
    s.view.task_filter = crate::TaskFilter::All;
    s.view.task_sort = crate::TaskSort::Categories;
    let text = page_text(&s, &Request { what: crate::print::What::Tasks, ..Request::default() });
    assert!(text.contains("No category") || s.mailbox.tasks.values().all(|t| !t.categories.is_empty()), "{text}");
    for c in s.mailbox.tasks.values().flat_map(|t| t.categories.clone()) {
        assert!(text.contains(&c), "{c} missing: {text}");
    }
}
