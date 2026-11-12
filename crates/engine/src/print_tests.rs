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
