// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The documents a calendar server sent, kept as they came so that a later edit can keep what
//! MailCraft doesn't model (#209, ADR 0026): in `<mailbox file>.dav` beside the mailbox, a folder
//! for each account and a file for each resource, named by a hash of the resource's address (never
//! by anything the server wrote as a name). Only read when an edit needs one.
//!
//! A document over [`MAX_RAW`] bytes is not kept (its event is read-only); an account keeps at most
//! [`MAX_ACCOUNT_RAW`] bytes: beyond that the oldest past events' documents are let go of first
//! (their events become read-only), and a document that still has no room is not kept. A file that
//! no event names any more is swept. Without a mailbox file (the browser, a mailbox that lives in
//! memory) nothing is kept and every event is read-only.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub use mailcraft_model::dav_down::{MAX_ACCOUNT_RAW, MAX_RAW};

use mailcraft_model::Id;

/// The name a resource's document is kept under.
pub fn name_of(href: &str) -> String {
    crate::store::Digest::of(href.as_bytes()).hex()
}

fn folder(mailbox_file: &Path, account: Id) -> PathBuf {
    crate::store::dav_dir(mailbox_file).join(account.to_string())
}

/// The folder of an account's documents as it stands during one taking-in: listed once, with what
/// is kept in it and how much, so that a thousand changes are a thousand writes and not a thousand
/// looks at the folder.
pub struct Store {
    dir: PathBuf,
    held: HashMap<String, u64>,
    total: u64,
    made: bool,
}

impl Store {
    /// Lists the account's folder (once). A folder that isn't there is empty.
    pub fn open(mailbox_file: &Path, account: Id) -> Store {
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
        Store { dir, held, total, made: false }
    }

    /// Lets go of the document of `href`.
    pub fn drop_one(&mut self, href: &str) {
        let name = name_of(href);
        if let Some(size) = self.held.remove(&name) {
            self.total = self.total.saturating_sub(size);
            let _ = std::fs::remove_file(self.dir.join(&name));
        }
    }

    /// Keeps `raw` for `href`, when it is small enough and there is room. `ends` is asked for only
    /// when there is not: the end of each event of the account that has a kept document, by name
    /// and as the events are now, to let the oldest go first (and room is made for a good many
    /// documents at once, so that this is not asked for at every one). Whether it was kept.
    pub fn keep(&mut self, href: &str, raw: &str, ends: &dyn Fn() -> HashMap<String, i64>) -> bool {
        if raw.len() > MAX_RAW {
            return false;
        }
        let name = name_of(href);
        let needed = raw.len() as u64;
        let own = self.held.get(&name).copied().unwrap_or(0);
        if self.total.saturating_sub(own).saturating_add(needed) > MAX_ACCOUNT_RAW {
            let ends = ends();
            // The oldest past events first (a file no event names, before them), down to a little
            // under the cap.
            let target = MAX_ACCOUNT_RAW - MAX_ACCOUNT_RAW / 16;
            let mut order: Vec<(String, u64)> = self.held.iter().filter(|(file, _)| **file != name).map(|(f, s)| (f.clone(), *s)).collect();
            order.sort_by_key(|(file, _)| ends.get(file).copied().unwrap_or(i64::MIN));
            for (file, size) in order {
                if self.total.saturating_sub(own).saturating_add(needed) <= target {
                    break;
                }
                if std::fs::remove_file(self.dir.join(&file)).is_ok() {
                    self.held.remove(&file);
                    self.total = self.total.saturating_sub(size);
                }
            }
            if self.total.saturating_sub(own).saturating_add(needed) > MAX_ACCOUNT_RAW {
                return false;
            }
        }
        if !self.made {
            if crate::store::create_private_dir_all(&self.dir).is_err() {
                return false;
            }
            self.made = true;
        }
        if crate::store::write_atomic(&self.dir.join(&name), raw.as_bytes()).is_err() {
            return false;
        }
        self.total = self.total.saturating_sub(own).saturating_add(needed);
        self.held.insert(name, needed);
        true
    }
}

/// The document kept for `href`, if one is.
pub fn read(mailbox_file: &Path, account: Id, href: &str) -> Option<String> {
    let bytes = std::fs::read(folder(mailbox_file, account).join(name_of(href))).ok()?;
    if bytes.len() > MAX_RAW {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Whether a document is kept for `href`.
pub fn has(mailbox_file: &Path, account: Id, href: &str) -> bool {
    std::fs::metadata(folder(mailbox_file, account).join(name_of(href))).is_ok_and(|m| m.is_file())
}

/// Lets go of the document of `href`.
pub fn drop_one(mailbox_file: &Path, account: Id, href: &str) {
    let _ = std::fs::remove_file(folder(mailbox_file, account).join(name_of(href)));
}

/// Lets go of every document of the account that `hrefs` doesn't name.
pub fn sweep(mailbox_file: &Path, account: Id, hrefs: &HashSet<String>) {
    let dir = folder(mailbox_file, account);
    let keep: HashSet<String> = hrefs.iter().map(|h| name_of(h)).collect();
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten().take(200_000) {
        if let Some(file) = entry.file_name().to_str()
            && !keep.contains(file)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Lets go of everything kept for the account.
pub fn drop_all(mailbox_file: &Path, account: Id) {
    let _ = std::fs::remove_dir_all(folder(mailbox_file, account));
}
