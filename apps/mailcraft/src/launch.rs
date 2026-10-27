// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mailbox the window starts with, read on a thread of its own so that the window is there
//! at once (#519; `mailcraft_ui_egui::opening`). What the thread reads is counted for the opening
//! screen; a panic on it is caught and is a mailbox that couldn't be read.

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use mailcraft_engine::io::OpenProgress;
use mailcraft_model::Mailbox;
use mailcraft_ui_egui::opening::{Arrival, Trouble};

/// Do `job` on a thread of its own and hand what it read through the receiver, then `wake` the
/// window. A panic on the way is a mailbox that couldn't be read ([`Trouble::Stopped`]).
pub fn start(job: impl FnOnce() -> Arrival + Send + 'static, wake: impl Fn() + Send + 'static) -> Receiver<Arrival> {
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new().name("opening the mailbox".into()).spawn(move || {
        let arrival = std::panic::catch_unwind(AssertUnwindSafe(job)).unwrap_or_else(|_| {
            log::error!("opening the mailbox stopped unexpectedly; MailCraft starts with an empty mailbox and leaves the file as it is");
            Arrival::unreadable(Trouble::Stopped)
        });
        let _ = tx.send(arrival);
        wake();
    });
    // Without the thread the window hears nothing, and says the mailbox couldn't be opened.
    if let Err(e) = spawned {
        log::error!("couldn't start opening the mailbox: {e}");
    }
    rx
}

/// The mailbox to start with and the file it is kept in (none for the sample, or without
/// preferences), counting what is read of the file in `progress`, with what the owner is told
/// first when the file couldn't be read, was damaged or was repaired.
pub fn first_mailbox(sample: bool, chosen: Option<PathBuf>, progress: &OpenProgress) -> Arrival {
    let now = mailcraft_time::now();
    if sample {
        return Arrival::opened(mailcraft_engine::sample::mailbox(now, mailcraft_time::local_offset_minutes(now)), None);
    }
    let ours = chosen.is_none();
    let path = chosen.or_else(|| crate::default_mailbox_path().filter(|_| crate::prefs_enabled()));
    let Some(path) = path else { return Arrival::opened(Mailbox::with_local_account("", ""), None) };
    if !path.exists() {
        // A first start: the settings folder has to be there for the mailbox to be saved in it.
        if ours && let Some(dir) = path.parent() {
            let _ = mailcraft_engine::store::create_private_dir_all(dir);
        }
        return Arrival::opened(Mailbox::with_local_account("", ""), Some(path));
    }
    match mailcraft_engine::store::open_with(&path, progress) {
        Ok(opened) => {
            if let Some(notice) = opened.notice() {
                log::warn!("{}: {notice}", path.display());
            }
            let damaged = opened.lost_saves > 0 || opened.unfinished > 0 || opened.lost_records > 0;
            let trouble = if damaged {
                Some(Trouble::Damaged)
            } else if opened.repaired > 0 {
                Some(Trouble::Repaired(opened.repaired))
            } else {
                None
            };
            Arrival { unavailable: opened.unavailable, damaged, trouble, ..Arrival::opened(opened.mailbox, Some(path)) }
        }
        Err(e) => {
            // Never write over a mailbox that couldn't be read: start empty, keep nothing, say why.
            log::error!("{e}");
            Arrival::unreadable(Trouble::Unreadable(e))
        }
    }
}

#[cfg(test)]
#[path = "launch_tests.rs"]
mod launch_tests;
