// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The documents a calendar server sent, kept as they came so that a later edit can keep what
//! MailCraft doesn't model (#209, ADR 0026): in `<mailbox file>.dav` beside the mailbox, a folder
//! for each account and a file for each resource, named by a hash of the resource's address (never
//! by anything the server wrote as a name). Only read when an edit needs one.
//!
//! A document over [`MAX_RAW`] bytes is not kept (its event is read-only); an account keeps at most
//! [`MAX_ACCOUNT_RAW`] bytes: beyond that the documents of the events that ended longest ago are
//! let go of first (their events become read-only), and a document that still has no room is not
//! kept. A file that no event names any more is swept. Without a mailbox file (the browser, a
//! mailbox that lives in memory) nothing is kept and every event is read-only.
//!
//! **These files are a cache, not the record.** The events are in the mailbox file, which is
//! saved durably; a document here is written without waiting for the disk (the folder is flushed
//! once for a whole taking-in), so a crash or a full disk may leave one missing, or the folder may
//! be cleared by hand. That is expected: [`read`] and [`has`] answer "none", and whatever edits a
//! server's event (D6) must then treat it as read-only or read the document from the server again
//! before it writes. Nothing may assume that an event from a calendar server has its document.
//!
//! **The disk work is not done on the caller's thread** (the window's, when a read is taken in):
//! writes, removals, the sweep and the flush go, in order, to one thread of their own
//! ([`writer`]); [`Store::open`], [`read`] and [`has`] first wait for what is queued, so that they
//! see the folder as it is meant to be.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use mailcraft_model::dav_down::{MAX_ACCOUNT_RAW, MAX_RAW};

use mailcraft_model::Id;

/// The name a resource's document is kept under.
pub fn name_of(href: &str) -> String {
    crate::store::Digest::of(href.as_bytes()).hex()
}

/// One thread that does the documents' disk work in the order it was asked for.
mod writer {
    use std::sync::mpsc::{Sender, channel};
    use std::sync::{Condvar, Mutex, OnceLock};

    type Job = Box<dyn FnOnce() + Send>;

    struct Queue {
        tx: Mutex<Option<Sender<Job>>>,
        /// Jobs asked for and not yet done.
        pending: Mutex<usize>,
        idle: Condvar,
    }

    fn queue() -> &'static Queue {
        static QUEUE: OnceLock<Queue> = OnceLock::new();
        QUEUE.get_or_init(|| Queue { tx: Mutex::new(None), pending: Mutex::new(0), idle: Condvar::new() })
    }

    fn done(q: &Queue) {
        let mut n = q.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *n = n.saturating_sub(1);
        if *n == 0 {
            q.idle.notify_all();
        }
    }

    /// Runs `job` on the documents' thread (or here, when that thread can't be had).
    pub fn run(job: Job) {
        let q = queue();
        *q.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
        let mut tx = q.tx.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if tx.is_none() {
            let (send, receive) = channel::<Job>();
            let spawned = std::thread::Builder::new().name("dav-documents".into()).spawn(move || {
                for job in receive {
                    // (A job that fails is a document not kept: the cache's way.)
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    done(queue());
                }
            });
            if spawned.is_ok() {
                *tx = Some(send);
            }
        }
        let job = match tx.as_ref() {
            Some(send) => match send.send(job) {
                Ok(()) => return,
                Err(back) => {
                    *tx = None;
                    back.0
                }
            },
            None => job,
        };
        drop(tx);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        done(q);
    }

    /// Waits until everything asked for is done.
    pub fn settle() {
        let q = queue();
        let mut n = q.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        while *n > 0 {
            n = q.idle.wait(n).unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

pub use writer::settle;

fn folder(mailbox_file: &Path, account: Id) -> PathBuf {
    crate::store::dav_dir(mailbox_file).join(account.to_string())
}

// How many times a folder was listed (for tests, which count work instead of timing it).
#[cfg(test)]
thread_local! {
    pub(crate) static LISTINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The folder of an account's documents as it stands during one taking-in: listed once, with what
/// is kept in it and how much, so that a thousand changes are a thousand writes and not a thousand
/// looks at the folder. When there is no room, the order documents go in is worked out once (from
/// the events' ends, asked for once) and kept up to date as documents come and go, so that a
/// thousand documents that find no room are not a thousand looks at the events.
pub struct Store {
    dir: PathBuf,
    held: HashMap<String, u64>,
    total: u64,
    /// Something was written: the folder is flushed once, when the store goes.
    wrote: bool,
    /// The documents by the end of their events, the earliest first: built at the first need.
    /// An entry whose end is no longer the one in `ends` (the document was kept again since) is
    /// passed over when it comes up.
    order: Option<BinaryHeap<Reverse<(i64, String)>>>,
    ends: HashMap<String, i64>,
}

impl Store {
    /// Lists the account's folder (once), as it is when what was queued is done. A folder that
    /// isn't there is empty.
    pub fn open(mailbox_file: &Path, account: Id) -> Store {
        settle();
        #[cfg(test)]
        LISTINGS.with(|n| n.set(n.get() + 1));
        let dir = folder(mailbox_file, account);
        let mut held: HashMap<String, u64> = HashMap::new();
        let mut total: u64 = 0;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten().take(200_000) {
                let (Some(file), Ok(meta)) = (entry.file_name().to_str().map(str::to_owned), entry.metadata()) else { continue };
                if meta.is_file() {
                    total = total.saturating_add(meta.len());
                    held.insert(file, meta.len());
                }
            }
        }
        Store { dir, held, total, wrote: false, order: None, ends: HashMap::new() }
    }

    /// The bytes the account's documents take, as far as this store knows.
    pub fn bytes(&self) -> u64 {
        self.total
    }

    /// Removes the file `name` (on the documents' thread) and lets go of it here.
    fn remove(&mut self, name: &str) {
        let path = self.dir.join(name);
        writer::run(Box::new(move || {
            let _ = std::fs::remove_file(path);
        }));
        if let Some(size) = self.held.remove(name) {
            self.total = self.total.saturating_sub(size);
        }
        self.ends.remove(name);
    }

    /// Lets go of the document of `href`, whether or not it was there when the folder was listed.
    pub fn drop_one(&mut self, href: &str) {
        let name = name_of(href);
        self.remove(&name);
    }

    /// Keeps `raw` for `href`, when it is small enough and there is room. `end` is when its events
    /// end (a series that never does: `i64::MAX`); `ends` is asked, at most once in the store's
    /// life, for the end of each document already kept, by name. When there is no room, the
    /// documents whose events ended first go first, never one that ends later than this one: a
    /// document older than everything held has no room made for it. Whether it was kept.
    pub fn keep(&mut self, href: &str, raw: &str, end: i64, ends: &dyn Fn() -> HashMap<String, i64>) -> bool {
        if raw.len() > MAX_RAW {
            return false;
        }
        let name = name_of(href);
        let needed = raw.len() as u64;
        let own = self.held.get(&name).copied().unwrap_or(0);
        let after = |total: u64| total.saturating_sub(own).saturating_add(needed);
        if after(self.total) > MAX_ACCOUNT_RAW {
            if self.order.is_none() {
                let known = ends();
                let mut order = BinaryHeap::with_capacity(self.held.len());
                for file in self.held.keys() {
                    // (A file no event names goes before them all.)
                    let at = known.get(file).copied().unwrap_or(i64::MIN);
                    self.ends.insert(file.clone(), at);
                    order.push(Reverse((at, file.clone())));
                }
                self.order = Some(order);
            }
            // Down to a little under the cap, so that the next ones find room too.
            let target = MAX_ACCOUNT_RAW - MAX_ACCOUNT_RAW / 16;
            while after(self.total) > target {
                let Some(Reverse((at, file))) = self.order.as_mut().and_then(BinaryHeap::pop) else { break };
                if at > end {
                    // (Back in its place: it is later than this one, and so is all that follows.)
                    if let Some(order) = self.order.as_mut() {
                        order.push(Reverse((at, file)));
                    }
                    break;
                }
                if file == name || self.ends.get(&file) != Some(&at) {
                    continue;
                }
                self.remove(&file);
            }
            if after(self.total) > MAX_ACCOUNT_RAW {
                return false;
            }
        }
        let (dir, file, bytes) = (self.dir.clone(), name.clone(), raw.as_bytes().to_vec());
        writer::run(Box::new(move || {
            if crate::store::create_private_dir_all(&dir).is_ok() {
                let _ = write_cached(&dir, &file, &bytes);
            }
        }));
        self.wrote = true;
        self.total = after(self.total);
        self.held.insert(name.clone(), needed);
        if let Some(order) = self.order.as_mut() {
            order.push(Reverse((end, name.clone())));
            self.ends.insert(name, end);
        }
        true
    }
}

impl Drop for Store {
    /// The folder is flushed once for everything written into it (the files themselves are not:
    /// they are a cache, see the module's note).
    fn drop(&mut self) {
        #[cfg(unix)]
        if self.wrote {
            let dir = self.dir.clone();
            writer::run(Box::new(move || {
                if let Ok(dir) = std::fs::File::open(&dir) {
                    let _ = dir.sync_all();
                }
            }));
        }
    }
}

/// Writes `bytes` as the file `name` in `dir`: to a file beside it, renamed over it when whole, so
/// that a reader sees the old document or the new one and never half of one. Not flushed to the
/// disk ([`Store`]'s drop flushes the folder once).
fn write_cached(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let tmp = dir.join(format!("{name}.tmp"));
    let written = crate::store::create_private_replacing(&tmp).and_then(|mut file| file.write_all(bytes));
    let done = written.and_then(|()| std::fs::rename(&tmp, dir.join(name)));
    if done.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    done
}

/// The document kept for `href`, if one is. None is an answer to expect (module's note).
pub fn read(mailbox_file: &Path, account: Id, href: &str) -> Option<String> {
    settle();
    let bytes = std::fs::read(folder(mailbox_file, account).join(name_of(href))).ok()?;
    if bytes.len() > MAX_RAW {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Whether a document is kept for `href`.
pub fn has(mailbox_file: &Path, account: Id, href: &str) -> bool {
    settle();
    std::fs::metadata(folder(mailbox_file, account).join(name_of(href))).is_ok_and(|m| m.is_file())
}

/// Lets go of the document of `href`.
pub fn drop_one(mailbox_file: &Path, account: Id, href: &str) {
    let path = folder(mailbox_file, account).join(name_of(href));
    writer::run(Box::new(move || {
        let _ = std::fs::remove_file(path);
    }));
}

/// Lets go of every document of the account that `hrefs` doesn't name (and of any half-written
/// one a run left behind).
pub fn sweep(mailbox_file: &Path, account: Id, hrefs: &HashSet<String>) {
    let dir = folder(mailbox_file, account);
    let keep: HashSet<String> = hrefs.iter().map(|h| name_of(h)).collect();
    writer::run(Box::new(move || {
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        for entry in entries.flatten().take(200_000) {
            if let Some(file) = entry.file_name().to_str()
                && !keep.contains(file)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }));
}

/// Lets go of everything kept for the account.
pub fn drop_all(mailbox_file: &Path, account: Id) {
    let dir = folder(mailbox_file, account);
    writer::run(Box::new(move || {
        let _ = std::fs::remove_dir_all(dir);
    }));
}
