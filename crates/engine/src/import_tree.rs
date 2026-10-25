// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Importing a whole tree of mbox files at once (`file.importFolderTree`, #420): the folders of
//! another mail program that keeps its mail as mbox files, brought in as subfolders of one new
//! top folder.
//!
//! Everything about the tree is hostile: how many files, how deep, how big, what they are called.
//! A file is an mbox by its first bytes, never by its name; links are not followed; a name that
//! would climb out of the tree (`..`) or hold control characters is made harmless; and what is
//! over a cap is left and said. The engine reads a directory only where there is a file system
//! (`path`); the browser build and agents hand the files over as `entries`.

use std::collections::HashSet;

use mailcraft_model::{AccountKind, FolderKind, Id, Mailbox};
use serde_json::{Value, json};

use crate::cmd::{base64_decode, fail};
use crate::{CmdError, Session, p};

/// Deepest a tree is walked (directories below the chosen one).
pub const MAX_TREE_DEPTH: usize = 12;
/// Most directory entries looked at.
pub const MAX_TREE_ENTRIES: usize = 20_000;
/// Most folders made.
pub const MAX_TREE_FOLDERS: usize = 5_000;
/// Most messages taken from one file.
pub const MAX_FILE_MESSAGES: usize = crate::io::MAX_IMPORT_MESSAGES;
/// Most messages taken in all.
pub const MAX_TREE_MESSAGES: usize = 500_000;
/// Most bytes of mbox files read in all (4 GB).
pub const MAX_TREE_BYTES: u64 = 4 << 30;
/// Most skipped files listed by name in the answer (the rest are counted).
const MAX_LISTED: usize = 50;
/// Longest a folder's name is kept, in characters.
const MAX_FOLDER_NAME: usize = 120;

/// Where a file's bytes are.
enum Source {
    Bytes(Vec<u8>),
    #[cfg(not(target_arch = "wasm32"))]
    File(std::path::PathBuf),
}

/// An mbox file found: the names of the folders above it, its own name, and its bytes.
struct Found {
    dirs: Vec<String>,
    name: String,
    source: Source,
}

#[derive(Default)]
struct Walk {
    found: Vec<Found>,
    skipped: Vec<(String, &'static str)>,
    notes: Vec<String>,
    bytes: u64,
}

impl Walk {
    fn skip(&mut self, path: String, why: &'static str) {
        self.skipped.push((path, why));
    }
}

/// Whether `head` (the first bytes of a file) starts like an mbox: a `From ` line, then a header.
pub fn looks_like_mbox(head: &[u8]) -> bool {
    if !head.starts_with(b"From ") {
        return false;
    }
    let mut lines = head.split(|b| *b == b'\n');
    let first = lines.next().unwrap_or_default();
    if first.len() < 6 || first.len() > 1024 {
        return false;
    }
    // A header line follows: a name, a colon (RFC 5322 field names are printable, no spaces).
    lines.next().is_some_and(|l| {
        let colon = l.iter().position(|b| *b == b':');
        colon.is_some_and(|c| c > 0 && l.get(..c).is_some_and(|n| n.iter().all(|b| b.is_ascii_graphic())))
    })
}

/// A name from disk as a folder name: no control characters or slashes, never `.` or `..`, cut to
/// size at a character boundary. `None` when nothing is left.
pub fn tidy_name(raw: &str) -> String {
    let s: String = raw.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\') { ' ' } else { c }).take(MAX_FOLDER_NAME).collect();
    let s = s.trim();
    if s.is_empty() || s.chars().all(|c| c == '.') {
        return "Folder".to_string();
    }
    mailcraft_model::clean_name(s).unwrap_or_else(|| "Folder".to_string())
}

/// The folder name a directory on disk stands for: `Inbox.sbd` holds the subfolders of `Inbox`.
fn dir_name(raw: &str) -> String {
    let stem = raw.get(raw.len().saturating_sub(4)..).filter(|e| e.eq_ignore_ascii_case(".sbd")).and_then(|_| raw.get(..raw.len() - 4));
    tidy_name(stem.unwrap_or(raw))
}

/// The folder name a file stands for: its name, without `.mbox`/`.mbx`.
fn file_name(raw: &str) -> String {
    let stem = raw
        .rsplit_once('.')
        .filter(|(stem, ext)| !stem.is_empty() && (ext.eq_ignore_ascii_case("mbox") || ext.eq_ignore_ascii_case("mbx")))
        .map_or(raw, |(stem, _)| stem);
    tidy_name(stem)
}

#[cfg(not(target_arch = "wasm32"))]
fn walk_dir(root: &std::path::Path, w: &mut Walk) -> Result<(), CmdError> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(root).map_err(|e| fail(format!("{}: {e}", root.display())))?;
    if !meta.is_dir() {
        return Err(fail(format!("{} is not a folder", root.display())));
    }
    let mut seen = 0usize;
    // (directory, names of the folders above it, depth)
    let mut stack = vec![(root.to_path_buf(), Vec::<String>::new(), 0usize)];
    while let Some((dir, dirs, depth)) = stack.pop() {
        let mut names: Vec<std::ffi::OsString> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.file_name()).collect(),
            Err(_) => {
                w.skip(dirs.join("/"), "can't be read");
                continue;
            }
        };
        names.sort();
        // Pushed in reverse so that the first directory is walked first.
        let mut subdirs = Vec::new();
        for os in names {
            seen += 1;
            if seen > MAX_TREE_ENTRIES {
                w.notes.push(format!("stopped looking after {MAX_TREE_ENTRIES} files and folders"));
                return Ok(());
            }
            let name = os.to_string_lossy().to_string();
            let shown = if dirs.is_empty() { name.clone() } else { format!("{}/{name}", dirs.join("/")) };
            let path = dir.join(&os);
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                w.skip(shown, "can't be read");
                continue;
            };
            let kind = meta.file_type();
            if kind.is_symlink() {
                w.skip(shown, "a link, not followed");
            } else if kind.is_dir() {
                if depth + 1 > MAX_TREE_DEPTH {
                    w.skip(shown, "nested too deeply");
                } else {
                    let mut below = dirs.clone();
                    below.push(dir_name(&name));
                    subdirs.push((path, below, depth + 1));
                }
            } else if kind.is_file() {
                if meta.len() > crate::io::MAX_FILE {
                    w.skip(shown, "too large");
                    continue;
                }
                let mut head = Vec::new();
                let sniffed = std::fs::File::open(&path).and_then(|f| f.take(2048).read_to_end(&mut head));
                if sniffed.is_err() {
                    w.skip(shown, "can't be read");
                } else if !looks_like_mbox(&head) {
                    w.skip(shown, "not a mailbox file");
                } else if w.bytes.saturating_add(meta.len()) > MAX_TREE_BYTES {
                    w.skip(shown, "over the size one import takes");
                } else {
                    w.bytes += meta.len();
                    w.found.push(Found { dirs: dirs.clone(), name, source: Source::File(path) });
                }
            } else {
                w.skip(shown, "not a file");
            }
        }
        stack.extend(subdirs.into_iter().rev());
    }
    Ok(())
}

/// Files handed over as `{"path": "Mail/Inbox.sbd/Sent", "data": base64}`.
fn from_entries(list: &[Value], w: &mut Walk) -> Result<(), CmdError> {
    for (i, e) in list.iter().enumerate() {
        if i >= MAX_TREE_ENTRIES {
            w.notes.push(format!("stopped looking after {MAX_TREE_ENTRIES} files"));
            break;
        }
        let raw = p::str(e, "path").unwrap_or("");
        // The path is only ever names to build folders from: `.` and `..` are no steps.
        let parts: Vec<&str> = raw.split(['/', '\\']).filter(|c| !c.is_empty() && *c != "." && *c != "..").collect();
        let Some((name, dirs)) = parts.split_last() else {
            w.skip(raw.chars().take(80).collect(), "has no name");
            continue;
        };
        if dirs.len() > MAX_TREE_DEPTH {
            w.skip(raw.chars().take(80).collect(), "nested too deeply");
            continue;
        }
        let Some(data) = p::str(e, "data") else {
            w.skip(raw.chars().take(80).collect(), "has no content");
            continue;
        };
        let Ok(bytes) = base64_decode(data) else {
            w.skip(raw.chars().take(80).collect(), "isn't readable");
            continue;
        };
        if !looks_like_mbox(bytes.get(..2048).unwrap_or(&bytes)) {
            w.skip(raw.chars().take(80).collect(), "not a mailbox file");
        } else if w.bytes.saturating_add(bytes.len() as u64) > MAX_TREE_BYTES || bytes.len() as u64 > crate::io::MAX_FILE {
            w.skip(raw.chars().take(80).collect(), "over the size one import takes");
        } else {
            w.bytes += bytes.len() as u64;
            w.found.push(Found { dirs: dirs.iter().map(|d| dir_name(d)).collect(), name: (*name).to_string(), source: Source::Bytes(bytes) });
        }
    }
    Ok(())
}

fn read(source: &Source) -> Option<Vec<u8>> {
    match source {
        Source::Bytes(b) => Some(b.clone()),
        #[cfg(not(target_arch = "wasm32"))]
        Source::File(path) => {
            use std::io::Read;
            let mut out = Vec::new();
            std::fs::File::open(path).ok()?.take(crate::io::MAX_FILE + 1).read_to_end(&mut out).ok()?;
            (out.len() as u64 <= crate::io::MAX_FILE).then_some(out)
        }
    }
}

/// The folder under `parent` called `name` (any case), made when `create` and it isn't there.
/// A folder that is an Outbox, or kept by a server, is never the one: its name is then made
/// different (`name (2)`).
fn child(mb: &mut Mailbox, account: Id, parent: Option<Id>, name: &str, create: bool, made: &mut usize) -> Result<Option<Id>, CmdError> {
    let usable = |mb: &Mailbox, n: &str| -> Result<Option<Option<Id>>, ()> {
        match mb.folders.iter().find(|f| f.account == account && f.parent == parent && f.name.to_lowercase() == n.to_lowercase()) {
            None => Ok(None),
            Some(f) if f.kind == FolderKind::Custom && !mb.is_send_queue(f.id) && f.remote.is_empty() => Ok(Some(Some(f.id))),
            Some(_) => Err(()),
        }
    };
    for n in 1..100 {
        let candidate = if n == 1 { name.to_string() } else { format!("{name} ({n})") };
        match usable(mb, &candidate) {
            Ok(Some(found)) => return Ok(found),
            Ok(None) => {
                if !create {
                    return Ok(None);
                }
                if *made >= MAX_TREE_FOLDERS {
                    return Err(fail(format!("more than {MAX_TREE_FOLDERS} folders")));
                }
                *made += 1;
                return mb.add_folder(account, parent, &candidate).map(Some).map_err(|e| fail(e.to_string()));
            }
            Err(()) => {}
        }
    }
    Err(fail(format!("no free name for the folder {name}")))
}

#[derive(Default)]
struct Tally {
    path: String,
    files: usize,
    found: usize,
    added: usize,
    duplicates: usize,
}

/// `file.importFolderTree`: see the command's params.
pub fn run(s: &mut Session, v: &Value) -> Result<Value, CmdError> {
    let dry = p::bool(v, "dryRun").unwrap_or(false);
    let mut w = Walk::default();
    let mut top = p::str(v, "name").map(tidy_name);
    if let Some(list) = v.get("entries").and_then(Value::as_array) {
        from_entries(list, &mut w)?;
    } else if let Some(path) = p::str(v, "path") {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let root = std::path::Path::new(path);
            walk_dir(root, &mut w)?;
            if top.is_none() {
                top = root.file_name().map(|n| tidy_name(&n.to_string_lossy()));
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            return Err(fail("the browser can't read folders by path; pass `entries`"));
        }
    } else {
        return Err(fail("name a folder (`path`) or hand over its files (`entries`)"));
    }
    let top = top.unwrap_or_else(|| "Imported mail".to_string());
    w.found.sort_by(|a, b| (&a.dirs, &a.name).cmp(&(&b.dirs, &b.name)));

    // Mail brought in from files lands in the local account, in folders of its own: never an
    // Outbox, never a folder a server keeps. A dry run only looks.
    let local = if dry { s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Local).map(|a| a.id) } else { Some(s.mailbox.local_account()) };
    let mut made = 0usize;
    let mut tallies: Vec<Tally> = Vec::new();
    let mut messages_total = 0usize;
    let mut stopped = false;
    let now = s.now();
    let top_id = match local {
        Some(account) => child(&mut s.mailbox, account, None, &top, !dry, &mut made)?,
        None => None,
    };
    let mut have: std::collections::HashMap<Option<Id>, HashSet<String>> = std::collections::HashMap::new();
    let found = std::mem::take(&mut w.found);
    for f in found {
        let mut names: Vec<String> = vec![top.clone()];
        names.extend(f.dirs.iter().cloned());
        names.push(file_name(&f.name));
        let shown = names.get(1..).unwrap_or_default().join("/");
        if messages_total >= MAX_TREE_MESSAGES {
            stopped = true;
            w.skip(shown, "over the number of messages one import takes");
            continue;
        }
        let Some(bytes) = read(&f.source) else {
            w.skip(shown, "can't be read");
            continue;
        };
        if !looks_like_mbox(bytes.get(..2048).unwrap_or(&bytes)) {
            w.skip(shown, "not a mailbox file");
            continue;
        }
        // The folder this file is (made only for a real import).
        let mut at = top_id;
        let mut failed_folder = None;
        if !dry && let Some(account) = local {
            for n in names.iter().skip(1) {
                match child(&mut s.mailbox, account, at, n, true, &mut made) {
                    Ok(id) => at = id,
                    Err(e) => {
                        failed_folder = Some(e.to_string());
                        break;
                    }
                }
            }
        } else if let Some(account) = local {
            for n in names.iter().skip(1) {
                at = match at {
                    Some(_) => child(&mut s.mailbox, account, at, n, false, &mut made)?,
                    None => None,
                };
            }
        } else {
            at = None;
        }
        if let Some(why) = failed_folder {
            w.notes.push(format!("{shown}: {why}"));
            w.skip(shown, "its folder couldn't be made");
            continue;
        }
        if !dry && at.is_none() {
            w.skip(shown, "its folder couldn't be made");
            continue;
        }
        let ids = have.entry(at).or_insert_with(|| {
            at.map(|f| s.mailbox.messages_in(f).map(|m| m.message_id.clone()).filter(|i| !i.is_empty()).collect()).unwrap_or_default()
        });
        let room = MAX_TREE_MESSAGES - messages_total;
        let limit = MAX_FILE_MESSAGES.min(room);
        let mut parsed: Vec<_> = mailcraft_mime::mbox_messages(&bytes).take(limit + 1).collect();
        let more = parsed.len() > limit;
        parsed.truncate(limit);
        if more {
            stopped = true;
            w.notes.push(format!("{shown}: stopped after {limit} messages"));
        }
        let path = names.get(1..).unwrap_or_default().join("/");
        let slot = match tallies.iter().position(|t| t.path == path) {
            Some(i) => i,
            None => {
                tallies.push(Tally { path, ..Default::default() });
                tallies.len() - 1
            }
        };
        let (mut found_n, mut added, mut dups) = (0, 0, 0);
        for mut m in parsed {
            found_n += 1;
            if !m.message_id.is_empty() && !ids.insert(m.message_id.clone()) {
                dups += 1;
                continue;
            }
            added += 1;
            if let (false, Some(folder)) = (dry, at) {
                m.id = 0;
                m.uid = 0;
                m.folder = folder;
                m.draft = false;
                if m.received == 0 {
                    m.received = if m.date != 0 { m.date } else { now };
                }
                s.mailbox.add_message(m).map_err(|e| fail(e.to_string()))?;
            }
        }
        messages_total += found_n;
        if let Some(t) = tallies.get_mut(slot) {
            t.files += 1;
            t.found += found_n;
            t.added += added;
            t.duplicates += dups;
        }
    }
    if let (false, Some(id)) = (dry, top_id) {
        s.view.folder = id;
        s.view.module = crate::Module::Mail;
        s.view.search.clear();
    }
    let added: usize = tallies.iter().map(|t| t.added).sum();
    let duplicates: usize = tallies.iter().map(|t| t.duplicates).sum();
    let listed: Vec<Value> = w.skipped.iter().take(MAX_LISTED).map(|(path, why)| json!({"path": path, "why": why})).collect();
    let folders: Vec<Value> = tallies
        .iter()
        .map(|t| json!({"path": t.path, "files": t.files, "messages": t.found, "added": t.added, "duplicates": t.duplicates}))
        .collect();
    Ok(json!({
        "dryRun": dry,
        "name": top,
        "folder": if dry { Value::Null } else { json!(top_id) },
        "folders": folders,
        "messages": added,
        "duplicates": duplicates,
        "skipped": listed,
        "skippedCount": w.skipped.len(),
        "notes": w.notes,
        "stopped": stopped,
    }))
}
