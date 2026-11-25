// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of Find (#209, D7): the search's question goes to the window, the owner's
//! answer comes back to it, and a window that went away stops it quietly. The search is a
//! stand-in here; `mailcraft_net::dav_find`'s own tests drive the real one against `mailcraft-dav`'s
//! stand-in server.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mailcraft_model::{DavFindFailure, DavFindQuestion, DavFound};
use mailcraft_net::dav_find::DavFindAsk;
use mailcraft_ui_egui::dav_find::{DavFindEvent, DavFindJob};

use crate::dav_find::find_with;

fn job() -> (DavFindJob, Sender<bool>) {
    let (tx, answers) = channel();
    let job = DavFindJob {
        email: "ada@example.org".into(),
        mail_host: "imap.example.org".into(),
        username: String::new(),
        password: "test-only-not-a-secret".into(),
        calendars: String::new(),
        contacts: String::new(),
        cancel: Arc::new(AtomicBool::new(false)),
        answers,
    };
    (job, tx)
}

/// A search that asks once about `dav.example.net` and finds a calendar there on a yes.
fn asking(ask: &DavFindAsk<'_>, question: &mut dyn FnMut(&str, DavFindQuestion) -> Option<bool>) -> Result<DavFound, DavFindFailure> {
    assert_eq!(ask.email, "ada@example.org");
    match question("dav.example.net", DavFindQuestion::Elsewhere) {
        Some(true) => Ok(DavFound { calendar_homes: vec!["https://dav.example.net/cal/ada/".into()], calendars: 1, ..Default::default() }),
        _ => Err(DavFindFailure::Declined),
    }
}

#[test]
fn the_question_goes_to_the_window_and_the_answer_comes_back() {
    for yes in [true, false] {
        let (job, answers) = job();
        let (events, seen) = channel();
        let told = std::thread::scope(|scope| {
            scope.spawn(move || find_with(&asking, &job, &mut |event| events.send(event).unwrap()));
            let first = seen.recv_timeout(Duration::from_secs(10)).expect("the owner is asked");
            answers.send(yes).unwrap();
            let last = seen.recv_timeout(Duration::from_secs(10)).expect("and told how it ended");
            vec![first, last]
        });
        assert_eq!(told[0], DavFindEvent::Question { host: "dav.example.net".into(), why: DavFindQuestion::Elsewhere });
        match &told[1] {
            DavFindEvent::Finished(Ok(found)) => assert!(yes && found.calendars == 1),
            DavFindEvent::Finished(Err(failure)) => assert!(!yes && *failure == DavFindFailure::Declined),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn a_window_that_went_away_while_the_question_is_open_ends_the_search_quietly() {
    let (job, _answers) = job();
    let cancel = job.cancel.clone();
    let told: Arc<Mutex<Vec<DavFindEvent>>> = Arc::default();
    std::thread::scope(|scope| {
        let (sink, seen) = (told.clone(), told.clone());
        let handle = scope.spawn(move || find_with(&asking, &job, &mut |event| sink.lock().unwrap().push(event)));
        for _ in 0..1_000 {
            if !seen.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    });
    let told = told.lock().unwrap();
    assert_eq!(told.len(), 1, "the question, and no ending");
    assert!(matches!(told[0], DavFindEvent::Question { .. }));
}

#[test]
fn a_search_without_questions_tells_its_end_once() {
    let (job, _answers) = job();
    let mut told = Vec::new();
    find_with(&|_, _| Err(DavFindFailure::NotFound), &job, &mut |event| told.push(event));
    assert_eq!(told, vec![DavFindEvent::Finished(Err(DavFindFailure::NotFound))]);
}
