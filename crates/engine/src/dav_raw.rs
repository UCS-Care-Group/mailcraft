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

/// Keeps `raw` for `href`, when it is small enough and there is room (`ends`: the end of each
/// event of the account that has a kept document, by name, to let the oldest go first). Whether it
/// was kept.
pub fn keep(mailbox_file: &Path, account: Id, href: &str, raw: &str, ends: &HashMap<String, i64>) -> bool {
    if raw.len() > MAX_RAW {
        return false;
    }
    let dir = folder(mailbox_file, account);
    if crate::store::create_private_dir_all(&dir).is_err() {
        return false;
    }
    let name = name_of(href);
    let mut held: Vec<(String, u64)> = Vec::new();
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten().take(200_000) {
            let (Some(file), Ok(meta)) = (entry.file_name().to_str().map(str::to_owned), entry.metadata()) else { continue };
            if meta.is_file() && file != name {
                total = total.saturating_add(meta.len());
                held.push((file, meta.len()));
            }
        }
    }
    let needed = raw.len() as u64;
    if total.saturating_add(needed) > MAX_ACCOUNT_RAW {
        // The oldest past events first (a file no event names, before them).
        held.sort_by_key(|(file, _)| ends.get(file).copied().unwrap_or(i64::MIN));
        for (file, size) in held {
            if total.saturating_add(needed) <= MAX_ACCOUNT_RAW {
                break;
            }
            if std::fs::remove_file(dir.join(&file)).is_ok() {
                total = total.saturating_sub(size);
            }
        }
        if total.saturating_add(needed) > MAX_ACCOUNT_RAW {
            return false;
        }
    }
    crate::store::write_atomic(&dir.join(&name), raw.as_bytes()).is_ok()
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
