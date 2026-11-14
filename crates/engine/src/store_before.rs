// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The mailbox file as it was before a newer MailCraft first wrote it in a newer format, kept
//! beside it so that going back to the version before finds a file it can open.
//!
//! Before [`crate::store`] first writes a mailbox file in a newer format than the one on disk
//! (from this one, format 3, on: the JSON file of the forms before is kept as `.format1` or
//! `.format2` by the store itself, and every MailCraft that updates itself reads format 3), the
//! file is kept beside it as `<file>.before-<version>` (`mailbox.mailcraft.before-0.6.0`, the
//! version of the program that changed the format): a second name for the same bytes where the
//! system has them, a copy otherwise. One is kept: an earlier one goes when a later one is made.
//! Only the mailbox file is kept; its content folder is shared by both, and what the kept file
//! names there is never swept ([`crate::store::sweep`]).
//!
//! Going back to the version before ([`put_back`]), the kept file takes the mailbox file's place,
//! and the newer one is kept as `<file>.after-<version>`: nothing is deleted.

use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use crate::io::INDEX_MAGIC;
use crate::store::SPLIT_VERSION;

/// How the names of the files kept go on from the mailbox file's own (`<file>.<this>…`).
pub(crate) const BEFORE: &str = "before-";
pub(crate) const AFTER: &str = "after-";

/// The format the mailbox file at `path` is in on disk: an index file's number, or for the JSON
/// file of earlier forms (1 and 2, which never named a number above that) [`SPLIT_VERSION`].
/// `None`: there is none, or it can't be read.
pub fn format_on_disk(path: &Path) -> Option<u32> {
    let mut head = [0u8; 24];
    let mut file = fs::File::open(path).ok()?;
    let mut read = 0;
    while read < head.len() {
        match file.read(head.get_mut(read..)?) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    if read == 0 {
        return None;
    }
    if head.starts_with(&INDEX_MAGIC[..]) {
        let number: [u8; 4] = head.get(INDEX_MAGIC.len()..INDEX_MAGIC.len() + 4)?.try_into().ok()?;
        return Some(u32::from_le_bytes(number));
    }
    Some(SPLIT_VERSION)
}

/// `<file>.<what><version>` beside `path`.
fn named(path: &Path, what: &str, version: &str) -> Option<PathBuf> {
    let mut name = path.file_name()?.to_os_string();
    name.push(format!(".{what}{version}"));
    Some(path.with_file_name(name))
}

/// A version as it may go into a file's name: digits, letters, dots and dashes.
fn safe(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !version.starts_with('.')
}

/// Before the program of `version` first writes the mailbox file at `path` in format `writing`:
/// when the file on disk is in an older one, keep it beside as `<file>.before-<version>` (one
/// kept: earlier ones go). Where it went, or `None` when nothing was to be kept.
pub fn keep_before_newer_format(path: &Path, writing: u32, version: &str) -> io::Result<Option<PathBuf>> {
    if !safe(version) {
        return Err(io::Error::other("not a version"));
    }
    let Some(format) = format_on_disk(path) else { return Ok(None) };
    if format >= writing {
        return Ok(None);
    }
    let kept = named(path, BEFORE, version).ok_or_else(|| io::Error::other("no name"))?;
    // Kept already by this version (a save that failed after it): that is the file as it was.
    if fs::symlink_metadata(&kept).is_ok() {
        return Ok(Some(kept));
    }
    if fs::hard_link(path, &kept).is_err() {
        let part = named(path, BEFORE, &format!("{version}.part")).ok_or_else(|| io::Error::other("no name"))?;
        let copied =
            fs::copy(path, &part).and_then(|_| fs::OpenOptions::new().write(true).open(&part)?.sync_all()).and_then(|()| fs::rename(&part, &kept));
        if let Err(e) = copied {
            let _ = fs::remove_file(&part);
            return Err(e);
        }
    }
    for older in kept_beside(path, BEFORE) {
        if older != kept {
            let _ = fs::remove_file(older);
        }
    }
    Ok(Some(kept))
}

/// The files beside the mailbox file `path` whose names go on from its own with `what`.
fn kept_beside(path: &Path, what: &str) -> Vec<PathBuf> {
    let (Some(parent), Some(stem)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else { return Vec::new() };
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    let prefix = format!("{stem}.{what}");
    fs::read_dir(parent)
        .into_iter()
        .flatten()
        .flatten()
        .take(10_000)
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix)))
        .map(|e| e.path())
        .collect()
}

/// Going back from the program of `version`: when it kept the mailbox file at `path` before it
/// changed its format, that takes the file's place again and the newer file is kept as
/// `<file>.after-<version>`. Whether there was one to put back. Call it only once nothing will
/// write the mailbox any more.
pub fn put_back(path: &Path, version: &str) -> io::Result<bool> {
    if !safe(version) {
        return Ok(false);
    }
    let (Some(before), Some(after)) = (named(path, BEFORE, version), named(path, AFTER, version)) else { return Ok(false) };
    if !fs::symlink_metadata(&before).is_ok_and(|m| m.is_file()) {
        return Ok(false);
    }
    if fs::symlink_metadata(path).is_ok() {
        if fs::symlink_metadata(&after).is_ok() {
            return Err(io::Error::other("a newer file is kept already"));
        }
        fs::rename(path, &after)?;
    }
    if let Err(e) = fs::rename(&before, path) {
        let _ = fs::rename(&after, path);
        return Err(e);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use mailcraft_model::Mailbox;

    use super::*;
    use crate::store::{self, INDEX_VERSION};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mailcraft-before-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mailbox() -> Mailbox {
        crate::sample::mailbox(1_791_993_600, 0)
    }

    /// The file at `path` replaced by one with `bytes`, as the store replaces it: written beside
    /// and renamed over.
    fn put(path: &Path, bytes: &[u8]) {
        let part = path.with_extension("part");
        fs::write(&part, bytes).unwrap();
        fs::rename(&part, path).unwrap();
    }

    #[test]
    fn a_file_in_this_format_is_never_kept_and_one_of_the_forms_before_is_kept_as_it_always_was() {
        let dir = scratch("this");
        let path = dir.join("mailbox.mailcraft");
        store::save(&path, &mailbox()).unwrap();
        assert_eq!(format_on_disk(&path), Some(INDEX_VERSION));
        let mut opened = store::open(&path).unwrap();
        opened.mailbox.messages.pop_first();
        store::save(&path, &opened.mailbox).unwrap();
        assert!(kept_beside(&path, BEFORE).is_empty());
        // The JSON file of the forms before: kept as `.format1` by the store, and only so.
        let json = dir.join("old.mailcraft");
        fs::write(&json, crate::io::write_mailbox(&mailbox()).unwrap()).unwrap();
        assert_eq!(format_on_disk(&json), Some(SPLIT_VERSION));
        let mut opened = store::open(&json).unwrap();
        opened.mailbox.messages.pop_first();
        store::save(&json, &opened.mailbox).unwrap();
        assert!(dir.join("old.mailcraft.format1").is_file());
        assert!(kept_beside(&json, BEFORE).is_empty());
        // And the new file opens.
        assert_eq!(store::open(&json).unwrap().mailbox.messages.len(), opened.mailbox.messages.len());
        let _ = fs::remove_dir_all(&dir);
    }

    /// An index file of format `n`.
    fn index_of(n: u32) -> Vec<u8> {
        let mut index = INDEX_MAGIC.to_vec();
        index.extend_from_slice(&n.to_le_bytes());
        index.extend_from_slice(&[0; 4]);
        index
    }

    #[test]
    fn an_older_format_is_kept_once_and_one_is_kept() {
        let dir = scratch("one");
        let path = dir.join("m.mailcraft");
        assert_eq!(keep_before_newer_format(&path, 4, "0.6.0").unwrap(), None, "no file");
        put(&path, &index_of(3));
        assert_eq!(format_on_disk(&path), Some(3));
        assert_eq!(keep_before_newer_format(&path, 3, "0.6.0").unwrap(), None, "the same format");
        let first = keep_before_newer_format(&path, 4, "0.6.0").unwrap().unwrap();
        assert_eq!(first, dir.join("m.mailcraft.before-0.6.0"));
        // The newer program writes its form; then, a save that failed, it tries again: the file
        // kept stays the one from before its first write.
        put(&path, &index_of(4));
        assert_eq!(keep_before_newer_format(&path, 4, "0.6.0").unwrap(), None);
        put(&path, &index_of(3));
        assert_eq!(keep_before_newer_format(&path, 4, "0.6.0").unwrap(), Some(first.clone()));
        assert_eq!(fs::read(&first).unwrap(), index_of(3));
        // A later change of format keeps the file before it, and the earlier one goes.
        put(&path, &index_of(4));
        let later = keep_before_newer_format(&path, 5, "0.7.0").unwrap().unwrap();
        assert_eq!(fs::read(&later).unwrap(), index_of(4));
        assert_eq!(kept_beside(&path, BEFORE), [later]);
        assert!(keep_before_newer_format(&path, 9, "../x").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn going_back_puts_the_file_from_before_in_its_place() {
        let dir = scratch("back");
        let path = dir.join("m.mailcraft");
        fs::write(&path, b"old").unwrap();
        keep_before_newer_format(&path, 99, "0.6.0").unwrap().unwrap();
        put(&path, b"new");
        assert!(!put_back(&path, "0.5.0").unwrap(), "nothing kept by that version");
        assert!(put_back(&path, "0.6.0").unwrap());
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read(dir.join("m.mailcraft.after-0.6.0")).unwrap(), b"new");
        assert!(!put_back(&path, "0.6.0").unwrap(), "once");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_kept_file_names_is_never_swept() {
        let dir = scratch("sweep");
        let path = dir.join("mailbox.mailcraft");
        let mut big = mailbox();
        let text = "a body long enough to be kept in the content folder ".repeat(200);
        for m in big.messages.values_mut() {
            std::sync::Arc::make_mut(m).body.text = text.clone();
        }
        store::save(&path, &big).unwrap();
        // As a newer program would have kept it before it changed the format.
        fs::copy(&path, dir.join("mailbox.mailcraft.before-9.0.0")).unwrap();
        // Every message gone: the file names no content any more, the kept one does.
        let mut empty = store::open(&path).unwrap().mailbox;
        empty.messages.clear();
        store::save(&path, &empty).unwrap();
        let swept = store::sweep(&path, std::time::Duration::ZERO).unwrap();
        assert_eq!(swept.removed, 0, "{swept:?}");
        let _ = fs::remove_dir_all(&dir);
    }
}
