// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! File › Print (#32): the dialog opens for the selection, sends the job the owner set up to the
//! printer they picked, and without a printer opens the PDF to be printed from the viewer.

use std::sync::{Arc, Mutex};

use egui::PointerButton;
use serde_json::json;

use crate::people_view::testing::{self, Window, click, text};
use crate::printing::{Host, Job, Printer, valid_range, with_labels};

/// A message of the sample mailbox.
const INVOICE: u64 = 15;

type Jobs = Arc<Mutex<Vec<Job>>>;

fn window(printers: bool) -> (Window, Jobs, Arc<Mutex<Vec<String>>>) {
    let mut h = testing::window("view.mail");
    let app = h.state_mut();
    let _ = app.execute("ui.dismissReminder", json!({"all": true}));
    let jobs: Jobs = Default::default();
    if printers {
        let sink = jobs.clone();
        app.services.print = Some(Host {
            printers: std::sync::Arc::new(|| {
                vec![
                    Printer { name: "Office_Laser".into(), label: "Laser".into(), default: false },
                    Printer { name: "Home_Inkjet".into(), label: "Inkjet".into(), default: true },
                ]
            }),
            submit: std::sync::Arc::new(move |job| {
                sink.lock().unwrap().push(job);
                Ok(())
            }),
        });
    }
    let opened: Arc<Mutex<Vec<String>>> = Default::default();
    let sink = opened.clone();
    app.services.open_file = Some(Box::new(move |name, bytes| {
        assert!(bytes.starts_with(b"%PDF-"));
        sink.lock().unwrap().push(name.to_string());
        Ok(())
    }));
    app.run("mail.select", json!({"id": INVOICE})).unwrap();
    h.run_steps(3);
    (h, jobs, opened)
}

/// The frames it takes the dialog to lay the pages out and list the printers.
fn settle(h: &mut Window) {
    for _ in 0..3000 {
        h.run_steps(1);
        let busy = matches!(&h.state().dialog, Some(crate::dialogs::Dialog::Print(d)) if d.busy());
        if !busy {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn press(h: &mut Window, kind: &str) {
    let r = h.state().hit.rect_in(0, kind, 0).unwrap_or_else(|| panic!("no {kind}"));
    click(h, r.center(), PointerButton::Primary, 1);
    h.run_steps(2);
}

#[test]
fn the_job_goes_to_the_default_printer_as_the_owner_set_it_up() {
    let (mut h, jobs, _) = window(true);
    h.state_mut().run("file.print", json!({})).unwrap();
    settle(&mut h);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("print"));
    press(&mut h, "printSomePages");
    press(&mut h, "printRange");
    text(&mut h, "1");
    press(&mut h, "printTwoSided");
    press(&mut h, "printLetter");
    settle(&mut h);
    press(&mut h, "printGo");
    settle(&mut h);
    assert!(h.state().dialog.is_none());
    let jobs = jobs.lock().unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0];
    assert_eq!(job.printer, "Home_Inkjet");
    assert_eq!(job.title, "Your October invoice is ready");
    assert_eq!((job.copies, job.pages.as_deref(), job.two_sided, job.color, job.paper), (1, Some("1"), true, true, "Letter"));
    assert!(job.pdf.starts_with(b"%PDF-") && String::from_utf8_lossy(&job.pdf).contains("/MediaBox [0 0 612 792]"));
    let said = h.state().status_msg.as_ref().map(|(s, _)| s.clone()).unwrap_or_default();
    assert!(said.contains("Inkjet"), "{said}");
}

#[test]
fn without_a_printer_the_pdf_opens_in_the_viewer() {
    let (mut h, _, opened) = window(false);
    h.state_mut().run("file.print", json!({})).unwrap();
    settle(&mut h);
    press(&mut h, "printGo");
    assert_eq!(*opened.lock().unwrap(), ["Your October invoice is ready.pdf"]);
}

#[test]
fn nothing_selected_opens_no_dialog() {
    let (mut h, jobs, _) = window(true);
    h.state_mut().session.sel.messages.clear();
    let _ = h.state_mut().run("file.print", json!({}));
    h.run_steps(2);
    assert!(!matches!(h.state().dialog.as_ref().map(|d| d.name()), Some("print")));
    assert!(jobs.lock().unwrap().is_empty());
}

#[test]
fn page_ranges_and_labels() {
    for ok in ["1", "1-3", "1-3, 5", " 2 ,4-4 "] {
        assert!(valid_range(ok), "{ok}");
    }
    for bad in ["", "0", "3-1", "a", "1-", "-2", "1,,2", &"1,".repeat(40)] {
        assert!(!valid_range(bad), "{bad}");
    }
    let p = with_labels("file.saveAsPdf", json!({"id": 3}));
    assert_eq!(p["labels"]["from"], "From:");
    assert_eq!(p["id"], 3);
    assert_eq!(with_labels("file.saveAsPdf", json!({"labels": {"from": "Von:"}}))["labels"]["from"], "Von:");
    assert_eq!(with_labels("mail.flag", json!({"id": 3})), json!({"id": 3}));
}

#[test]
fn the_dialog_prints_what_it_counted_though_the_selection_moves_on() {
    let (mut h, jobs, _) = window(true);
    h.state_mut().run("file.print", json!({})).unwrap();
    settle(&mut h);
    let other = h.state().session.mailbox.messages.keys().copied().find(|id| *id != INVOICE).unwrap();
    h.state_mut().session.sel.messages = vec![other];
    let Some(crate::dialogs::Dialog::Print(d)) = &h.state().dialog else { panic!("no dialog") };
    assert_eq!(d.ids(), [INVOICE]);
    press(&mut h, "printGo");
    settle(&mut h);
    assert_eq!(jobs.lock().unwrap()[0].title, "Your October invoice is ready");
}

#[test]
fn a_printer_that_says_no_is_said_once_and_the_dialog_stays() {
    let (mut h, _, _) = window(true);
    if let Some(host) = h.state_mut().services.print.as_mut() {
        host.submit = Arc::new(|_| Err(crate::printing::PrintError::NoAnswer));
    }
    h.state_mut().run("file.print", json!({})).unwrap();
    settle(&mut h);
    press(&mut h, "printGo");
    settle(&mut h);
    assert_eq!(h.state().dialog.as_ref().map(|d| d.name()), Some("print"));
}

#[test]
fn an_agent_s_print_comes_back_as_data_without_a_dialog() {
    let (mut h, _, _) = window(true);
    h.state_mut().session.agent = Some(Vec::new());
    let r = h.state_mut().execute("file.print", json!({})).unwrap();
    h.state_mut().session.agent = None;
    assert!(r["data"].as_str().is_some_and(|d| !d.is_empty()), "{r}");
    assert!(h.state().dialog.is_none());
}

#[test]
fn the_calendar_prints_in_the_style_chosen() {
    let (mut h, jobs, _) = window(true);
    h.state_mut().run("view.calendar", json!({})).unwrap();
    h.run_steps(2);
    h.state_mut().run("file.print", json!({})).unwrap();
    settle(&mut h);
    assert!(h.state().hit.rect_in(0, "printMonth", 0).is_some(), "the calendar asks for its style");
    press(&mut h, "printMonth");
    settle(&mut h);
    press(&mut h, "printGo");
    settle(&mut h);
    let jobs = jobs.lock().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].title, "Calendar");
}

#[test]
fn the_dialog_keeps_the_days_and_the_style_it_was_asked_for() {
    let (mut h, _, _) = window(true);
    h.state_mut().run("file.print", json!({"what": "calendar", "from": "2026-10-12", "to": "2026-10-16", "style": "agenda"})).unwrap();
    settle(&mut h);
    let Some(crate::dialogs::Dialog::Print(d)) = &h.state().dialog else { panic!("no dialog") };
    assert_eq!(d.month(), Some(false));
    let what = d.request().what;
    let day = |s: &str| mailcraft_time::Date::parse_iso(s);
    assert_eq!(
        what,
        mailcraft_engine::print::What::Calendar {
            from: day("2026-10-12"),
            to: day("2026-10-16"),
            style: Some(mailcraft_engine::print_views::CalendarStyle::Agenda)
        }
    );
}
