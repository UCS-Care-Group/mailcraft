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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use mailcraft_model::{AccountKind, FolderKind, Id, Mailbox, Message};
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

/// How far a scan or an import has come, and whether its owner has had enough. Shared between the
/// thread that does the work and the window that shows it: counts only go up, and `cancel` is
/// looked at between files and between messages.
#[derive(Default)]
pub struct Progress {
    /// Folders looked into.
    pub folders: AtomicUsize,
    /// Mailbox files found.
    pub files: AtomicUsize,
    /// Messages found so far.
    pub messages: AtomicUsize,
    cancelled: AtomicBool,
}

impl Progress {
    /// Ask the work to stop; it ends with an error and changes nothing.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// (folders, files, messages) so far.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.folders.load(Ordering::Relaxed), self.files.load(Ordering::Relaxed), self.messages.load(Ordering::Relaxed))
    }

    fn check(&self) -> Result<(), CmdError> {
        if self.is_cancelled() { Err(fail("cancelled")) } else { Ok(()) }
    }
}

/// A mailbox file read and parsed, off the window's thread, waiting to be put into the mailbox.
pub struct ParsedFile {
    dirs: Vec<String>,
    name: String,
    /// Every message's Message-ID, in order ("" when it has none).
    ids: Vec<String>,
    /// The messages themselves; empty when only counting (a preview).
    messages: Vec<Message>,
}

/// A whole tree read and parsed: what [`load_path`] and [`load_entries`] hand to the window, and
/// [`finish`] puts into the mailbox in one go.
pub struct Parsed {
    top: String,
    files: Vec<ParsedFile>,
    skipped: Vec<(String, &'static str)>,
    notes: Vec<String>,
    stopped: bool,
    count_only: bool,
}

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
fn walk_dir(root: &std::path::Path, w: &mut Walk, progress: &Progress) -> Result<(), CmdError> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(root).map_err(|e| fail(format!("{}: {e}", root.display())))?;
    if !meta.is_dir() {
        return Err(fail(format!("{} is not a folder", root.display())));
    }
    let mut seen = 0usize;
    // (directory, names of the folders above it, depth)
    let mut stack = vec![(root.to_path_buf(), Vec::<String>::new(), 0usize)];
    while let Some((dir, dirs, depth)) = stack.pop() {
        progress.check()?;
        progress.folders.fetch_add(1, Ordering::Relaxed);
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
            progress.check()?;
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
                    progress.files.fetch_add(1, Ordering::Relaxed);
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
fn from_entries(list: &[Value], w: &mut Walk, progress: &Progress) -> Result<(), CmdError> {
    for (i, e) in list.iter().enumerate() {
        progress.check()?;
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
            progress.files.fetch_add(1, Ordering::Relaxed);
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

/// Read and parse what the walk found. This is the heavy part (every file is read and every
/// message parsed), which is why it takes no `Session`: a host runs it on a thread.
fn parse_found(mut w: Walk, top: String, count_only: bool, progress: &Progress) -> Result<Parsed, CmdError> {
    w.found.sort_by(|a, b| (&a.dirs, &a.name).cmp(&(&b.dirs, &b.name)));
    let found = std::mem::take(&mut w.found);
    let mut files = Vec::new();
    let mut messages_total = 0usize;
    let mut stopped = false;
    for f in found {
        progress.check()?;
        let fname = file_name(&f.name);
        let shown = f.dirs.iter().map(String::as_str).chain(std::iter::once(fname.as_str())).collect::<Vec<_>>().join("/");
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
        let limit = MAX_FILE_MESSAGES.min(MAX_TREE_MESSAGES - messages_total);
        let mut ids = Vec::new();
        let mut messages = Vec::new();
        let mut more = false;
        for m in mailcraft_mime::mbox_messages(&bytes) {
            if ids.len() >= limit {
                more = true;
                break;
            }
            progress.check()?;
            progress.messages.fetch_add(1, Ordering::Relaxed);
            ids.push(m.message_id.clone());
            if !count_only {
                messages.push(m);
            }
        }
        if more {
            stopped = true;
            w.notes.push(format!("{shown}: stopped after {limit} messages"));
        }
        messages_total += ids.len();
        files.push(ParsedFile { dirs: f.dirs, name: fname, ids, messages });
    }
    Ok(Parsed { top, files, skipped: w.skipped, notes: w.notes, stopped, count_only })
}

/// Walk the folder at `path`, read every mbox file in it and parse the messages: no `Session`, so
/// a host can do it on a thread. `count_only` keeps only the Message-IDs (for a preview).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_path(path: &str, name: Option<&str>, count_only: bool, progress: &Progress) -> Result<Parsed, CmdError> {
    let root = std::path::Path::new(path);
    let mut w = Walk::default();
    walk_dir(root, &mut w, progress)?;
    let top =
        name.map(tidy_name).or_else(|| root.file_name().map(|n| tidy_name(&n.to_string_lossy()))).unwrap_or_else(|| "Imported mail".to_string());
    parse_found(w, top, count_only, progress)
}

/// The same for files handed over as `entries`.
pub fn load_entries(list: &[Value], name: Option<&str>, count_only: bool, progress: &Progress) -> Result<Parsed, CmdError> {
    let mut w = Walk::default();
    from_entries(list, &mut w, progress)?;
    let top = name.map(tidy_name).unwrap_or_else(|| "Imported mail".to_string());
    parse_found(w, top, count_only, progress)
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
    let name = p::str(v, "name");
    let progress = Progress::default();
    let parsed = if p::bool(v, "staged").unwrap_or(false) {
        // Read and parsed by the window, off its thread, and handed over by `Session::stage_tree`.
        s.staged_tree.take().ok_or_else(|| fail("nothing is waiting to be imported"))?
    } else if let Some(list) = v.get("entries").and_then(Value::as_array) {
        load_entries(list, name, dry, &progress)?
    } else if let Some(path) = p::str(v, "path") {
        #[cfg(not(target_arch = "wasm32"))]
        {
            load_path(path, name, dry, &progress)?
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            return Err(fail("the browser can't read folders by path; pass `entries`"));
        }
    } else {
        return Err(fail("name a folder (`path`) or hand over its files (`entries`)"));
    };
    finish(s, parsed, dry)
}

/// Put a tree read by [`load_path`] or [`load_entries`] into the mailbox (or, `dry`, say what
/// would go in). Nothing here reads a file: it is quick enough for the window's thread.
pub fn finish(s: &mut Session, parsed: Parsed, dry: bool) -> Result<Value, CmdError> {
    if !dry && parsed.count_only {
        return Err(fail("that was only looked at, not read in full"));
    }
    let Parsed { top, files, skipped, notes, stopped, .. } = parsed;
    let mut w = Walk { skipped, notes, ..Walk::default() };
    // Mail brought in from files lands in the local account, in folders of its own: never an
    // Outbox, never a folder a server keeps. A dry run only looks.
    let local = if dry { s.mailbox.accounts.iter().find(|a| a.kind == AccountKind::Local).map(|a| a.id) } else { Some(s.mailbox.local_account()) };
    let mut made = 0usize;
    let mut tallies: Vec<Tally> = Vec::new();
    let now = s.now();
    let top_id = match local {
        Some(account) => child(&mut s.mailbox, account, None, &top, !dry, &mut made)?,
        None => None,
    };
    let mut have: std::collections::HashMap<Option<Id>, HashSet<String>> = std::collections::HashMap::new();
    for f in files {
        let mut names: Vec<String> = vec![top.clone()];
        names.extend(f.dirs.iter().cloned());
        names.push(f.name.clone());
        let shown = names.get(1..).unwrap_or_default().join("/");
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
        let slot = match tallies.iter().position(|t| t.path == shown) {
            Some(i) => i,
            None => {
                tallies.push(Tally { path: shown, ..Default::default() });
                tallies.len() - 1
            }
        };
        let (mut found_n, mut added, mut dups) = (0, 0, 0);
        let mut messages = f.messages.into_iter();
        for id in f.ids {
            found_n += 1;
            let m = messages.next();
            if !id.is_empty() && !ids.insert(id) {
                dups += 1;
                continue;
            }
            added += 1;
            if let (false, Some(folder), Some(mut m)) = (dry, at, m) {
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
