// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The host's side of Find in File › Info › Calendars and Contacts (#209, D7), on a thread of its
//! own, once the owner pressed Find and not before.
//!
//! The search is `mailcraft_net::dav_find`'s: it decides where the password may go. This only
//! carries its question to the window (`DavFindEvent::Question`), waits for the owner's answer,
//! and tells the window how it ended. A window that closed (the dialog went away) stops it at the
//! next question, and nothing more is told. Nothing here is logged.

use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use mailcraft_model::{DavFindFailure, DavFindQuestion, DavFound};
use mailcraft_net::dav_find::DavFindAsk;
use mailcraft_ui_egui::dav_find::{DavFindEvent, DavFindJob};

/// The search itself: the real one, or a test's.
pub(crate) type Search<'a> = &'a dyn Fn(&DavFindAsk<'_>, &mut dyn FnMut(&str, DavFindQuestion) -> Option<bool>) -> Result<DavFound, DavFindFailure>;

/// Waits for the owner's answer; `None` when the owner gave up.
fn answer(job: &DavFindJob) -> Option<bool> {
    loop {
        match job.answers.recv_timeout(Duration::from_millis(100)) {
            Ok(yes) => return Some(yes),
            Err(RecvTimeoutError::Timeout) if job.cancel.load(Ordering::Relaxed) => return None,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// Runs a job to its end, telling `emit` of the owner's questions and, last, how it ended (nothing,
/// when the owner has given up).
pub(crate) fn find_with(search: Search<'_>, job: &DavFindJob, emit: &mut dyn FnMut(DavFindEvent)) {
    let ask = DavFindAsk {
        email: &job.email,
        mail_host: &job.mail_host,
        username: &job.username,
        password: &job.password,
        calendars: &job.calendars,
        contacts: &job.contacts,
    };
    let end = search(&ask, &mut |host, why| {
        if job.cancel.load(Ordering::Relaxed) {
            return None;
        }
        emit(DavFindEvent::Question { host: host.to_owned(), why });
        answer(job)
    });
    if !job.cancel.load(Ordering::Relaxed) {
        emit(DavFindEvent::Finished(end));
    }
}

/// Runs a job on a thread of its own, waking the window for each thing it tells.
pub(crate) fn run(job: DavFindJob, wake: &egui::Context) -> Receiver<DavFindEvent> {
    let (tx, rx) = channel();
    let wake = wake.clone();
    let spawned = std::thread::Builder::new().name("calendar server search".into()).spawn(move || {
        find_with(&|ask, question| mailcraft_net::dav_find::find(ask, question), &job, &mut |event| {
            // Nobody is listening any more (the dialog closed): stop.
            if tx.send(event).is_err() {
                job.cancel.store(true, Ordering::Relaxed);
            }
            wake.request_repaint();
        });
    });
    // A dropped sender reads as nothing found in the window.
    if let Err(e) = spawned {
        log::error!("couldn't start searching for a calendar server: {e}");
    }
    rx
}
