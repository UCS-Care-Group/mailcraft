//! `file.print`: a message, the calendar, contacts and tasks as a PDF.

use mailcraft_time::{Date, DateTime};
use serde_json::{Value, json};

use super::document;
use crate::{Module, Session, sample};

fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

fn session() -> Session {
    Session::at(sample::mailbox(now(), 0), now(), 0)
}

fn pdf(r: &Value) -> Vec<u8> {
    super::super::base64_decode(r["data"].as_str().unwrap()).unwrap()
}

#[test]
fn a_message_prints_in_memo_style() {
    let mut s = session();
    let m = s.mailbox.messages.values().find(|m| !m.to.is_empty() && !m.subject.is_empty()).unwrap().clone();
    let r = s.run("file.print", &json!({"what": "message", "id": m.id})).unwrap();
    assert_eq!((r["what"].as_str(), r["style"].as_str()), (Some("message"), Some("memo")));
    assert!(r["pages"].as_u64().unwrap() >= 1);
    assert!(r["name"].as_str().unwrap().ends_with(".pdf"));
    let bytes = pdf(&r);
    assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
    let doc = document(&s, &json!({"id": m.id})).unwrap();
    let text = doc.text();
    assert!(text.contains("From:") && text.contains(&m.from.email), "{text}");
    assert!(text.contains(&m.subject));
    let owner = &s.mailbox.account(m.account).unwrap().name;
    assert!(doc.pages.iter().all(|p| p.text().starts_with(owner.as_str())));
    // Printing changes nothing.
    assert!(!s.dirty);
}

#[test]
fn the_selected_message_or_the_module_in_front_is_printed() {
    let mut s = session();
    assert!(s.run("file.print", &json!({})).is_err(), "nothing selected");
    let id = s.rows()[0].id;
    s.run("mail.select", &json!({"id": id})).unwrap();
    assert_eq!(s.run("file.print", &json!({})).unwrap()["what"], "message");
    for (module, what) in [(Module::Calendar, "calendar"), (Module::People, "contacts"), (Module::Tasks, "tasks")] {
        s.view.module = module;
        let r = s.run("file.print", &json!({})).unwrap();
        assert_eq!(r["what"], what);
        assert!(pdf(&r).starts_with(b"%PDF"));
    }
}

#[test]
fn the_calendar_prints_in_each_style_over_a_range() {
    let mut s = session();
    let start = Date::new(2026, 10, 14).unwrap().start_of_week(s.prefs.week_start);
    let range = json!({"from": start.iso(), "to": start.add_days(6).iso()});
    let pages = |s: &mut Session, style: &str| {
        s.run("file.print", &json!({"what": "calendar", "style": style, "range": range})).unwrap()["pages"].as_u64().unwrap()
    };
    assert_eq!(pages(&mut s, "day"), 7);
    assert_eq!(pages(&mut s, "week"), 1);
    assert_eq!(pages(&mut s, "workWeek"), 1);
    assert_eq!(pages(&mut s, "month"), 1);
    // What the sample calendar has that week is on the pages.
    let doc = document(&s, &json!({"what": "calendar", "style": "week", "range": range})).unwrap();
    let titled = super::super::calendar::occurrences(&s, start, start.add_days(6));
    let some = titled.iter().filter_map(|o| s.mailbox.event(o.event)).find(|e| !e.title.is_empty() && e.title.len() < 12);
    if let Some(e) = some {
        assert!(doc.text().contains(&e.title), "{} isn't printed", e.title);
    }
    assert!(s.run("file.print", &json!({"what": "calendar", "style": "fortnight"})).is_err());
    assert!(s.run("file.print", &json!({"what": "calendar", "range": {"from": "2026-10-18", "to": "2026-10-12"}})).is_err());
    assert!(s.run("file.print", &json!({"what": "calendar", "range": "next week"})).is_err());
}

#[test]
fn contacts_and_tasks_print() {
    let mut s = session();
    let doc = document(&s, &json!({"what": "contacts"})).unwrap();
    let c = s.mailbox.contacts.values().next().unwrap().clone();
    assert!(doc.text().contains(c.primary_email().unwrap_or(&c.name())));
    let doc = document(&s, &json!({"what": "tasks"})).unwrap();
    let t = s.mailbox.tasks.values().next().unwrap().clone();
    assert!(doc.text().contains("Subject") && doc.text().contains(&t.title.chars().take(10).collect::<String>()));
    let r = s.run("file.print", &json!({"what": "tasks", "paper": "a4"})).unwrap();
    assert!(String::from_utf8_lossy(&pdf(&r)).contains("/MediaBox [0 0 595.28 841.89]"));
    assert!(s.run("file.print", &json!({"what": "tasks", "paper": "tabloid"})).is_err());
    assert!(s.run("file.print", &json!({"what": "faxes"})).is_err());
}

#[test]
fn a_print_is_written_where_it_is_asked() {
    let mut s = session();
    let dir = std::env::temp_dir().join(format!("mailcraft-print-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tasks.pdf");
    let r = s.run("file.print", &json!({"what": "tasks", "path": path.to_string_lossy()})).unwrap();
    assert!(r.get("data").is_none());
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"%PDF-1.7"));
    assert_eq!(r["bytes"].as_u64().unwrap() as usize, bytes.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_picture_on_the_web_is_never_fetched_for_printing() {
    let mut s = session();
    let id = s.mailbox.messages.keys().next().copied().unwrap();
    let mut m = (**s.mailbox.messages.get(&id).unwrap()).clone();
    m.body.html = Some("<p>Hi</p><img src=\"https://tracker.example/pixel.png\" width=300 height=100>".into());
    s.mailbox.messages.insert(id, std::sync::Arc::new(m));
    let doc = document(&s, &json!({"id": id})).unwrap();
    assert!(doc.pictures.is_empty());
    assert!(doc.text().contains("not downloaded"));
}
