// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The phone's mailbox on disk: the same file and content folder the desktop keeps
//! (`mailcraft_engine::store`: an append-only log, content beside it, atomic writes, the same
//! budgets), in a private directory the app names. Everything here runs on the session's thread,
//! which is the mailbox's one writer, so a save always sees a whole state.
//!
//! - Opening restores what is there; a first start begins with an empty mailbox. A file that
//!   can't be read is renamed aside (`mailbox.mailcraft.unreadable-<millis>`, never written
//!   over) and a fresh mailbox starts, with a `StorageNotice` event for the owner. When even
//!   that rename fails nothing is written at all and saving says so.
//! - Saving: after a change once a few seconds have passed ([`EVERY`]), before a send (the
//!   host's `persist`), at `save_now` and when the core ends. A failed save is reported (once
//!   per streak) and tried again ([`RETRY`]); the engine leaves the last good file as it was.
//! - Removing the data deletes everything in the directory.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use mailcraft_engine::Session;
use mailcraft_model::Mailbox;

use crate::CoreError;
use crate::events::{Event, EventSink, StorageNotice};

/// The mailbox file's name (the content folder is `<name>.blobs` beside it). The same as the
/// first Kotlin keeper used, so a phone that has a mailbox keeps it.
pub(crate) const FILE_NAME: &str = "mailbox.mailcraft";
/// How long after a change the autosave waits, and how long after a failed save.
pub(crate) const EVERY: Duration = if cfg!(test) { Duration::from_millis(60) } else { Duration::from_secs(3) };
pub(crate) const RETRY: Duration = if cfg!(test) { Duration::from_millis(150) } else { Duration::from_secs(10) };
/// The longest a directory name may be.
const MAX_DIR_BYTES: usize = 4096;

struct State {
    /// The revision the file holds; `None` before the first save of a mailbox that had no file.
    saved: Option<u64>,
    /// When the autosave is due, once the mailbox has changed.
    due: Option<Instant>,
    failing: bool,
    /// `false` when the file couldn't be read and couldn't be kept aside: nothing is written.
    writable: bool,
}

pub(crate) struct Keep {
    dir: PathBuf,
    path: PathBuf,
    sink: EventSink,
    state: Mutex<State>,
}

impl Keep {
    /// The storage in `dir`, an absolute path made if it isn't there.
    pub(crate) fn prepare(dir: &str, sink: EventSink) -> Result<Arc<Keep>, CoreError> {
        let bad = |why: &str| CoreError::Init { reason: format!("the mailbox's folder can't be used: {why}") };
        if dir.is_empty() || dir.len() > MAX_DIR_BYTES || dir.contains('\0') {
            return Err(bad("its name is empty, too long or not a name"));
        }
        let dir = PathBuf::from(dir);
        if !dir.is_absolute() {
            return Err(bad("it is not an absolute path"));
        }
        mailcraft_engine::store::create_private_dir_all(&dir).map_err(|_| bad("it can't be made"))?;
        if !fs::metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Err(bad("it is not a folder"));
        }
        let path = dir.join(FILE_NAME);
        let state = State { saved: None, due: None, failing: false, writable: true };
        Ok(Arc::new(Keep { dir, path, sink, state: Mutex::new(state) }))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The mailbox the file holds (or a new one). Never fails: what can't be read is put aside.
    pub(crate) fn open(&self) -> Mailbox {
        match fs::symlink_metadata(&self.path) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Mailbox::with_local_account("", ""),
            Err(e) => {
                // The file may be there and can't be looked at now: nothing is written over it
                // until it can be read or set aside (a restart tries again).
                log::error!("the mailbox file couldn't be looked at: {e}");
                self.lock().writable = false;
                self.sink.push(Event::StorageNotice { notice: StorageNotice::NotKept, detail: String::new() });
                return Mailbox::with_local_account("", "");
            }
        }
        let opened = catch_unwind(AssertUnwindSafe(|| mailcraft_engine::store::open(&self.path)))
            .unwrap_or_else(|_| Err("the file crashed the reader".to_string()));
        match opened {
            Ok(o) => {
                if let Some(detail) = o.notice() {
                    log::warn!("the mailbox file: {detail}");
                    self.sink.push(Event::StorageNotice { notice: StorageNotice::PartlyRead, detail });
                }
                o.mailbox
            }
            Err(e) => {
                log::error!("the mailbox file couldn't be read: {e}");
                match self.set_aside() {
                    Some(name) => self.sink.push(Event::StorageNotice { notice: StorageNotice::SetAside, detail: name }),
                    None => {
                        self.lock().writable = false;
                        self.sink.push(Event::StorageNotice { notice: StorageNotice::NotKept, detail: String::new() });
                    }
                }
                Mailbox::with_local_account("", "")
            }
        }
    }

    /// Rename the file to `mailbox.mailcraft.unreadable-<millis>`; the name, or `None` when it can't be.
    fn set_aside(&self) -> Option<String> {
        let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        for n in 0..20u32 {
            let name = if n == 0 { format!("{FILE_NAME}.unreadable-{millis}") } else { format!("{FILE_NAME}.unreadable-{millis}-{n}") };
            let aside = self.dir.join(&name);
            if fs::symlink_metadata(&aside).is_ok() {
                continue;
            }
            return fs::rename(&self.path, &aside).ok().map(|()| name);
        }
        None
    }

    /// The session is built: what it holds is what the file holds.
    pub(crate) fn opened(&self, session: &Session) {
        let existed = fs::symlink_metadata(&self.path).is_ok();
        self.lock().saved = existed.then(|| session.rev());
    }

    /// Write the mailbox now, whatever the debounce. An error leaves the last good file.
    pub(crate) fn save(&self, session: &Session) -> Result<(), String> {
        let result = self.write(session);
        if let Err(e) = &result {
            let mut st = self.lock();
            st.due = Some(Instant::now() + RETRY);
            if !std::mem::replace(&mut st.failing, true) {
                self.sink.push(Event::SaveFailed { reason: e.clone() });
            }
        }
        result
    }

    fn write(&self, session: &Session) -> Result<(), String> {
        let rev = session.rev();
        {
            let st = self.lock();
            if !st.writable {
                return Err("the mailbox file couldn't be read or kept aside, so nothing is saved".to_string());
            }
            if st.saved == Some(rev) {
                return Ok(());
            }
        }
        let saved = catch_unwind(AssertUnwindSafe(|| mailcraft_engine::store::save(&self.path, &session.mailbox)))
            .unwrap_or_else(|_| Err("saving crashed; the last good file is kept".to_string()));
        saved?;
        let mut st = self.lock();
        st.saved = Some(rev);
        st.due = None;
        st.failing = false;
        Ok(())
    }

    /// After a job: save when the autosave is due. Gives how long to wait for the next one when
    /// the mailbox is changed and not yet written.
    pub(crate) fn after_job(&self, session: &Session) -> Option<Duration> {
        let wait = {
            let mut st = self.lock();
            if !st.writable || st.saved == Some(session.rev()) {
                st.due = None;
                return None;
            }
            let now = Instant::now();
            let due = *st.due.get_or_insert(now + EVERY);
            due.checked_duration_since(now)
        };
        match wait {
            Some(wait) if !wait.is_zero() => Some(wait),
            _ => self.save(session).err().map(|_| RETRY),
        }
    }

    /// Delete everything the directory holds (the mailbox, its content, what was kept aside), then
    /// forget what the engine knows of it. Entries that are links are removed, never followed.
    pub(crate) fn delete_all(&self) -> Result<(), String> {
        let mut failed = 0usize;
        match fs::read_dir(&self.dir) {
            Ok(entries) => {
                for entry in entries {
                    let Ok(entry) = entry else {
                        failed += 1;
                        continue;
                    };
                    let p = entry.path();
                    let real_dir = fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_dir());
                    let done = if real_dir { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
                    if done.is_err() {
                        failed += 1;
                    }
                }
            }
            Err(_) => failed += 1,
        }
        mailcraft_engine::store::forget_ahead(&self.path);
        mailcraft_engine::store::clear_cache();
        if failed > 0 { Err(format!("{failed} files of the phone's mail couldn't be removed")) } else { Ok(()) }
    }

    /// The mailbox is a fresh one now and has been written as such (there is no file).
    pub(crate) fn reset(&self, session: &Session) {
        let mut st = self.lock();
        st.saved = Some(session.rev());
        st.due = None;
        st.failing = false;
        st.writable = true;
    }
}
