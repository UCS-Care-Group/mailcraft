// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening an attachment from the window: what is opened, what is asked first, what is refused,
//! and the browser's download in its place (#126).

use std::sync::{Arc, Mutex};

use mailcraft_engine::Session;
use mailcraft_model::Attachment;
use serde_json::json;

use crate::{MailApp, Services};

/// Wednesday 2026-10-14 16:00 UTC.
const NOW: i64 = 1_791_993_600;

type Opened = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

/// The sample mailbox with a message carrying `names`, and a host that notes what it is asked to
/// open. The message's id comes back too.
fn app(names: &[&str]) -> (MailApp, u64, Opened) {
    let mut app = MailApp::new(Session::at(mailcraft_engine::sample::mailbox(NOW, 0), NOW, 0), Services::default());
    app.autosave = false;
    let id = app.session.rows().first().map(|r| r.id).unwrap();
    let m = app.session.mailbox.message_mut(id).unwrap();
    m.attachments = names.iter().map(|n| Attachment::new(*n, "application/octet-stream", b"hello".to_vec())).collect();
    app.run("mail.select", json!({"id": id})).unwrap();
    let opened: Opened = Default::default();
    let sink = opened.clone();
    app.services.open_file = Some(Box::new(move |name, bytes| {
        sink.lock().unwrap().push((name.to_string(), bytes.to_vec()));
        Ok(())
    }));
    (app, id, opened)
}

#[test]
fn an_ordinary_file_is_opened_under_a_safe_name() {
    let (mut a, id, opened) = app(&["../../report.pdf"]);
    let r = a.run("mail.openAttachment", json!({"id": id, "index": 0})).unwrap();
    assert_eq!((r["name"].as_str(), r["opened"].as_bool()), (Some("report.pdf"), Some(true)));
    assert_eq!(*opened.lock().unwrap(), vec![("report.pdf".to_string(), b"hello".to_vec())]);
}

#[test]
fn programs_and_scripts_are_refused_and_nothing_is_opened() {
    let (mut a, id, opened) = app(&["setup.exe", "invoice.pdf.js", "run.sh", "README"]);
    for index in 0..4 {
        let e = a.run("mail.openAttachment", json!({"id": id, "index": index})).unwrap_err();
        assert!(e.contains("doesn't open") && e.contains("Save As"), "{e}");
        // Said in a box, as the owner did it by hand.
        assert!(crate::refusal_worth_a_box("mail.openAttachment", &e));
    }
    // A script asking without the window's question is refused too.
    assert!(a.execute("mail.openAttachment", json!({"id": id, "index": 0})).is_err());
    assert!(opened.lock().unwrap().is_empty());
    // Saving them is still the owner's choice.
    assert!(a.execute("mail.saveAttachment", json!({"id": id, "index": 0})).is_ok());
}

#[test]
fn a_kind_of_no_known_name_is_refused_where_the_desktop_types_by_content() {
    use mailcraft_engine::opening::Chooser;
    let (mut a, id, opened) = app(&["invoice.qzx"]);
    let r = a.run("mail.openAttachment", json!({"id": id, "index": 0}));
    if Chooser::THIS == Chooser::NameOrContent {
        let e = r.unwrap_err();
        assert!(e.contains("doesn't open invoice.qzx") && e.contains("Save As"), "{e}");
        assert!(opened.lock().unwrap().is_empty());
        assert!(a.execute("mail.saveAttachment", json!({"id": id, "index": 0})).is_ok());
    } else {
        assert_eq!(r.unwrap()["opened"], true);
    }
}

#[test]
fn macros_and_web_pages_are_asked_about_first() {
    let (mut a, id, opened) = app(&["budget.xlsm", "page.html", "notes.md", "key.asc", "mail.p7m"]);
    // Markdown, keys, certificates and signatures: the program that opens them acts on them.
    for index in 2..5 {
        let (_, text, _) = super::question(&a, &json!({"id": id, "index": index})).unwrap();
        assert!(text.contains("act on what is in them"), "{text}");
    }
    for index in 0..5 {
        let r = a.run("mail.openAttachment", json!({"id": id, "index": index})).unwrap();
        assert_eq!(r["pending"], "confirm");
        let Some(crate::dialogs::Dialog::Confirm { ask, .. }) = &a.dialog else { panic!("no question") };
        assert_eq!(ask.title, "Opening Mail Attachment");
        a.dialog = None;
    }
    assert!(opened.lock().unwrap().is_empty());
    // Once the owner said yes, it opens.
    a.run("mail.openAttachment", json!({"id": id, "index": 0, "confirmed": true})).unwrap();
    assert_eq!(opened.lock().unwrap().len(), 1);
}

/// Every way a command reaches the window (`execute`: the control channel and tests;
/// `ui_command`; `run` without its question's yes) refuses a kind that asks first until the yes is
/// said, before the host is asked to open anything; with it, the file opens.
#[test]
fn a_kind_that_asks_first_never_opens_without_the_yes_by_any_route() {
    let names = ["budget.xlsm", "page.html", "notes.md", "key.asc", "mail.p7s"];
    let (mut a, id, opened) = app(&names);
    for index in 0..names.len() {
        let p = json!({"id": id, "index": index});
        let e = a.execute("mail.openAttachment", p.clone()).unwrap_err();
        assert!(e.contains("asked about first"), "{e}");
        assert!(super::ui_command(&mut a, "mail.openAttachment", &p).unwrap().is_err());
        let p = json!({"id": id, "index": index, "confirmed": false});
        assert!(a.execute("mail.openAttachment", p).is_err());
        assert!(opened.lock().unwrap().is_empty(), "{index}: nothing reaches the host before the yes");
    }
    for index in 0..names.len() {
        a.execute("mail.openAttachment", json!({"id": id, "index": index, "confirmed": true})).unwrap();
    }
    assert_eq!(opened.lock().unwrap().len(), names.len());
}

#[test]
fn the_browser_downloads_instead_and_nothing_breaks_without_a_host() {
    let (mut a, id, _) = app(&["photo.jpg"]);
    a.services.open_file = None;
    // No host at all: said, not crashed.
    let e = a.run("mail.openAttachment", json!({"id": id, "index": 0})).unwrap_err();
    assert!(e.contains("Save As"), "{e}");
    let got: Opened = Default::default();
    let sink = got.clone();
    a.services.download = Some(Box::new(move |name, bytes| {
        sink.lock().unwrap().push((name.to_string(), bytes.to_vec()));
        Ok(())
    }));
    let r = a.run("mail.openAttachment", json!({"id": id, "index": 0})).unwrap();
    assert_eq!(r["downloaded"], true);
    assert_eq!(got.lock().unwrap().len(), 1);
}

#[test]
fn junk_params_are_refused() {
    let (mut a, id, opened) = app(&["report.pdf"]);
    for p in [
        json!({}),
        json!({"id": id}),
        json!({"id": id, "index": 7}),
        json!({"id": id, "index": -1}),
        json!({"id": id, "index": u64::MAX}),
        json!({"id": u64::MAX, "index": 0}),
        json!({"id": "x", "index": "y"}),
        json!(null),
        json!([1, 2]),
    ] {
        let _ = a.run("mail.openAttachment", p.clone());
        let _ = a.execute("mail.openAttachment", p);
    }
    assert!(opened.lock().unwrap().is_empty());
}
