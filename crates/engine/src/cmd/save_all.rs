// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Save All Attachments: every file a message lists (not the pictures its text shows in place),
//! into a folder the owner picked, or handed back as `data` when no folder is named.
//!
//! The names are the sender's, and hostile: each is made safe as Save As makes one
//! ([`crate::opening::save_name`]: no folders of its own, no `..`, nothing hidden, no name Windows
//! keeps for a device), and nothing in the folder is ever written over: a name already taken
//! there, or by another file of the same message, gets a number (`report (2).pdf`), and each file
//! is created only if it isn't there yet.

use mailcraft_model::Id;
use serde_json::{Value, json};

use crate::cmd::{bad, fail};
use crate::{CmdError, CmdResult, Part, Session, p};

/// Most bytes handed back as `data` together (the files are base64 in one answer).
const MAX_DATA: usize = 64 << 20;
/// Most numbered names tried for one file before giving up on it.
const MAX_TRIES: u32 = 1000;

/// `mail.saveAllAttachments`.
pub(super) fn save_all(s: &mut Session, v: &Value) -> CmdResult {
    p::only(v, &["id", "path"])?;
    let id = p::id_strict(v, "id")?.or(s.sel.focus()).ok_or_else(|| fail("select a message first"))?;
    let m = s.mailbox.message(id).ok_or_else(|| failed!("no message with id {id}", id))?;
    let listed: Vec<(usize, String)> = m.listed_attachments().map(|(i, a)| (i, crate::opening::save_name(&a.name))).collect();
    if listed.is_empty() {
        return Err(fail("the message has no attachments to save"));
    }
    match v.get("path") {
        None | Some(Value::Null) => as_data(s, id, &listed),
        Some(Value::String(folder)) if !folder.trim().is_empty() => into_folder(s, id, &listed, folder),
        Some(_) => Err(bad("`path` is the folder to save the attachments in")),
    }
}

/// The files as base64 `data`, each under its safe name, numbered where two are the same.
fn as_data(s: &Session, id: Id, listed: &[(usize, String)]) -> CmdResult {
    let mut taken: Vec<String> = Vec::new();
    let mut files = Vec::new();
    let mut total = 0usize;
    for (index, name) in listed {
        let bytes = s.content(id, Part::Attachment(*index))?;
        total = total.saturating_add(bytes.len());
        if total > MAX_DATA {
            return Err(failed!("the attachments are larger than {mb} MB together: save them in a folder", mb = MAX_DATA >> 20));
        }
        let name = free_name(name, |n| taken.iter().any(|t| t.eq_ignore_ascii_case(n))).ok_or_else(|| fail("too many files of one name"))?;
        taken.push(name.clone());
        files.push(json!({"index": index, "name": name, "bytes": bytes.len(), "data": crate::cmd::base64_encode(&bytes)}));
    }
    Ok(json!({"saved": files.len(), "files": files}))
}

#[cfg(target_arch = "wasm32")]
fn into_folder(s: &Session, id: Id, listed: &[(usize, String)], _folder: &str) -> CmdResult {
    as_data(s, id, listed)
}

/// Each file written into `folder`, which must be there already, under a name nothing there has.
#[cfg(not(target_arch = "wasm32"))]
fn into_folder(s: &Session, id: Id, listed: &[(usize, String)], folder: &str) -> CmdResult {
    use std::io::Write;
    let dir = std::path::Path::new(folder);
    if !dir.is_dir() {
        return Err(fail(format!("{folder}: not a folder")));
    }
    let mut files = Vec::new();
    for (index, name) in listed {
        let bytes = s.content(id, Part::Attachment(*index))?;
        let mut written = None;
        for n in 1..=MAX_TRIES {
            let candidate = numbered(name, n);
            let path = dir.join(&candidate);
            // Created only if nothing is there: never over another file, nor through a link.
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    f.write_all(&bytes).map_err(|e| fail(format!("{}: {e}", path.display())))?;
                    written = Some((candidate, path));
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(saved_so_far(&files, fail(format!("{}: {e}", path.display())))),
            }
        }
        let Some((saved, path)) = written else {
            return Err(saved_so_far(&files, failed!("{name} couldn't be saved: the folder has too many files of that name", name)));
        };
        // A file from mail: marked as a browser marks a download.
        crate::mark::mark_from_internet(&path);
        files.push(json!({"index": index, "name": saved, "path": path.to_string_lossy(), "bytes": bytes.len()}));
    }
    Ok(json!({"saved": files.len(), "path": folder, "files": files}))
}

/// The error, saying how many files were saved before it.
#[cfg(not(target_arch = "wasm32"))]
fn saved_so_far(files: &[Value], e: CmdError) -> CmdError {
    if files.is_empty() { e } else { fail(format!("{e} ({} saved before it)", files.len())) }
}

/// The first of `name`, `name (2)`, `name (3)`… that `taken` says isn't taken.
fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    (1..=MAX_TRIES).map(|n| numbered(name, n)).find(|n| !taken(n))
}

/// `name` the `n`th time: as it is the first time, then `stem (n).ext`.
pub fn numbered(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}
