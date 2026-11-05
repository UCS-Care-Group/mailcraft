// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Importing a .pst file (`file.importPst`, #815; a preview until every kind of item comes
//! across): the mail of every folder the owner sees in it, Deleted Items and Junk E-mail too,
//! as ordinary folders on this computer, and a report that accounts for every item found.
//!
//! The file is read by `mailcraft-pst` from [MS-PST], over `Read + Seek`, never whole into
//! memory; each item is read into a message from [MS-OXCMSG], [MS-OXPROPS] and [MS-OXOMSG]
//! ([`item`]). Mail lands in local folders alone, never in an account on a server, and nothing
//! of it leaves the computer. Calendar items, contacts, tasks, notes and journal entries are
//! counted and named "not yet brought in": a later part brings them.
//!
//! The work ([`read`]) takes no `Session`, so that a host runs it on a thread: it hands what it
//! has read over in batches, which go into the mailbox as they come ([`apply`]); cancelling
//! keeps what is in, and another run of the same file brings the rest, as every item brought in
//! remembers where in the file it was ([`mailcraft_mime::PST_ITEM_HEADER`]: the file's identity
//! and the item's node id) and is counted "already here".
//!
//! The import fails unless every item found is accounted for: brought in, already here, left
//! out (with what it is, its folder and why) or not yet brought in.

pub mod item;
pub mod report;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use mailcraft_model::{AccountKind, Id, Mailbox, Message, Rgb};
use mailcraft_pst::{Error as PstError, FolderKind, Limits, Nid, PS_PUBLIC_STRINGS, PropName, PropValue, Pst, node_types};
use serde_json::{Value, json};

use crate::cmd::{base64_decode, fail};
use crate::import_tree::{child_within, tidy_name};
use crate::{CmdError, Session, p};
use item::{Named, Text, ends_import, item_ref, later_kind};
use report::{Entry, Report, Row};

pub use mailcraft_mime::PST_ITEM_HEADER;

/// Most messages handed over in one batch.
pub const BATCH_MESSAGES: usize = if cfg!(test) { 3 } else { 500 };
/// Most bytes (text and attachments) of one batch.
pub const BATCH_BYTES: u64 = 64 << 20;
/// Most folders one import makes.
pub const MAX_FOLDERS: usize = 10_001;
/// Most categories of the file's list taken in.
const MAX_CATEGORIES: usize = 200;
/// Most bytes of the file's list of categories read.
const MAX_CATEGORY_XML: u64 = 1 << 20;
/// The message class of the file's list of categories (an associated item).
const CATEGORY_LIST: &str = "IPM.Configuration.CategoryList";

/// How far an import has come, and whether its owner has had enough: shared between the thread
/// that reads and the window that shows it.
#[derive(Default)]
pub struct Progress {
    /// Items dealt with, and found, in all.
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    /// The folder being read: its items dealt with and found, and which of how many it is.
    pub folder_done: AtomicUsize,
    pub folder_total: AtomicUsize,
    pub folders_done: AtomicUsize,
    pub folders_total: AtomicUsize,
    folder: Mutex<String>,
    cancelled: AtomicBool,
}

impl Progress {
    /// Ask the work to stop: it hands over what it has read and ends.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// The folder being read (its path in the file).
    pub fn folder(&self) -> String {
        self.folder.lock().map(|f| f.clone()).unwrap_or_default()
    }

    fn set_folder(&self, path: &str, total: usize) {
        if let Ok(mut f) = self.folder.lock() {
            path.clone_into(&mut f);
        }
        self.folder_done.store(0, Ordering::Relaxed);
        self.folder_total.store(total, Ordering::Relaxed);
        self.folders_done.fetch_add(1, Ordering::Relaxed);
    }

    fn step(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
        self.folder_done.fetch_add(1, Ordering::Relaxed);
    }
}

/// Where the mail goes: a new folder named after the file (the default), or under a folder on
/// this computer the owner picked. Never an account on a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    New(String),
    Under(Id),
}

/// What one import is asked to do.
pub struct Job {
    pub password: Option<String>,
    pub dry_run: bool,
    /// The file's own name (for the report) and the folder it goes into by default.
    pub file: String,
    pub name: String,
    /// The items already brought in from .pst files ([`known_keys`]).
    pub known: HashSet<String>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the password.
        f.debug_struct("Job").field("file", &self.file).field("dry_run", &self.dry_run).finish_non_exhaustive()
    }
}

/// Messages read, to go into the mailbox: each with the path of its folder below the top.
#[derive(Default)]
pub struct Batch {
    /// Folders to make even when nothing lands in them (the file's empty mail folders).
    pub folders: Vec<Vec<String>>,
    pub messages: Vec<(Vec<String>, Message)>,
    bytes: u64,
}

impl Batch {
    fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.messages.is_empty()
    }
}

/// What a whole read came to: the report, and the file's list of categories.
pub struct Outcome {
    pub report: Report,
    pub categories: Vec<(String, Rgb)>,
}

/// What the window has waiting for `file.importPst` with `staged`.
pub enum Staged {
    Batch(Batch),
    Done(Box<Outcome>),
}

/// What the thread of an import hands the window, in turn.
pub enum Event {
    Batch(Batch),
    Done(Result<Box<Outcome>, String>),
}

/// The keys of every message brought in from a .pst file ([`PST_ITEM_HEADER`]).
pub fn known_keys(mb: &Mailbox) -> HashSet<String> {
    mb.messages.values().filter_map(|m| m.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(PST_ITEM_HEADER)).map(|(_, v)| v.clone())).collect()
}

/// The identity of a file: its provider's id (PidTagRecordKey), which every copy of it keeps;
/// for a file without one, its size and its name inside.
fn identity<R: Read + Seek>(pst: &Pst<R>, store: &mailcraft_pst::Store) -> String {
    match &store.record_key {
        Some(k) if !k.is_empty() => k.iter().take(32).map(|b| format!("{b:02x}")).collect(),
        _ => {
            let name = store.display_name.clone().unwrap_or_default();
            format!("size{}-{:08x}", pst.sizes().1, mailcraft_pst::crc32(name.as_bytes()))
        }
    }
}

fn key(identity: &str, nid: u32) -> String {
    format!("{identity}:{nid:08x}")
}

/// The folder name a .pst file is brought into by default: its own name, without `.pst`.
pub fn folder_name(file: &str) -> String {
    let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let stem = base.rsplit_once('.').filter(|(stem, ext)| !stem.is_empty() && ext.eq_ignore_ascii_case("pst")).map_or(base, |(stem, _)| stem);
    tidy_name(stem)
}

/// What the window asks of a file before an import: whether it is a .pst file, which kind, and
/// whether it has a password. Reads the header and the message store only.
pub struct Look {
    pub needs_password: bool,
    pub format: &'static str,
}

/// [`Look`] at a file on disk.
#[cfg(not(target_arch = "wasm32"))]
pub fn look(path: &str) -> Result<Look, String> {
    let file = crate::io::regular_file(std::path::Path::new(path)).map_err(|e| e.to_string())?;
    let pst = Pst::open(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    Ok(Look { needs_password: pst.password_required(), format: format_name(pst.format()) })
}

/// Whether `password` opens the file at `path` (a file without one takes any).
#[cfg(not(target_arch = "wasm32"))]
pub fn password_opens(path: &str, password: &str) -> bool {
    let Ok(file) = crate::io::regular_file(std::path::Path::new(path)) else { return false };
    Pst::open(std::io::BufReader::new(file)).is_ok_and(|pst| pst.check_password(password))
}

fn format_name(f: mailcraft_pst::Format) -> &'static str {
    match f {
        mailcraft_pst::Format::Ansi => "ANSI",
        _ => "Unicode",
    }
}

fn pst_error(e: &PstError) -> CmdError {
    match e {
        PstError::NotPst => fail("this isn't a .pst file"),
        PstError::PasswordRequired => fail("this .pst file has a password: give it to import the file"),
        PstError::WrongPassword => fail("that isn't the password of this .pst file: nothing was imported"),
        other => failed!("the .pst file couldn't be read: {why}", why = other.to_string()),
    }
}

/// A folder of the file that is brought: where it goes, and what kind it is.
struct Brought {
    id: Nid,
    path: Vec<String>,
    shown: String,
    /// Made even when empty: a folder of mail (not of calendar items or contacts, which come
    /// with a later part).
    mail: bool,
}

/// Read a .pst file over `reader` and hand over its mail in batches through `sink` (nothing,
/// for a dry run): see the module. `limits` are the reader's caps ([`Limits::default`] for an
/// import).
pub fn read<R: Read + Seek>(
    reader: R,
    job: &Job,
    limits: Limits,
    progress: &Progress,
    sink: &mut dyn FnMut(Batch) -> Result<(), CmdError>,
) -> Result<Outcome, CmdError> {
    let mut pst = Pst::open_with(reader, limits.clone()).map_err(|e| pst_error(&e))?;
    if pst.password_required() {
        let Some(password) = job.password.as_deref() else { return Err(pst_error(&PstError::PasswordRequired)) };
        pst.unlock(password).map_err(|e| pst_error(&e))?;
    }
    let store = pst.store().map_err(|e| pst_error(&e))?;
    let identity = identity(&pst, &store);
    let mut report = Report {
        file: job.file.clone(),
        name: job.name.clone(),
        dry_run: job.dry_run,
        format: format_name(pst.format()).into(),
        ..Default::default()
    };

    let tree = pst.walk_folders().map_err(|e| pst_error(&e))?;
    let store_charset = store.properties.int(0x3FFD).and_then(item::code_page);
    let named = named(&mut pst, store_charset, &mut report)?;

    // Every item of the node tree, by its folder: what the folders' lists are checked against.
    let mut nodes: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut counted = 0usize;
    let mut past = 0usize;
    let walked = pst.for_each_node(|n| {
        if n.id.kind() == node_types::NORMAL_MESSAGE {
            if counted < limits.max_items {
                counted += 1;
                nodes.entry(n.parent.0).or_default().push(n.id.0);
            } else {
                past += 1;
            }
        }
        !progress.is_cancelled()
    });
    match walked {
        Ok(()) => {}
        Err(e) if ends_import(&e) => return Err(pst_error(&e)),
        Err(e) => report.damaged(format!("the file's list of items: {e}")),
    }

    // Which folders are brought, and where they go.
    let ipm = store.ipm_subtree;
    let mut brought: Vec<Brought> = Vec::new();
    let mut paths: HashMap<Nid, Vec<String>> = HashMap::new();
    let mut internal: HashMap<u32, String> = HashMap::new();
    let mut used: HashSet<(Vec<String>, String)> = HashSet::new();
    for entry in &tree.folders {
        let f = &entry.folder;
        if !entry.in_ipm_subtree {
            let name = if f.name.trim().is_empty() { "(the root)".to_string() } else { report::short(&f.name) };
            report.not_brought.push(Entry {
                what: name.clone(),
                folder: String::new(),
                why: "the file's own folder (its root, searches or views)".into(),
            });
            internal.insert(f.id.0, name);
            continue;
        }
        if f.kind == FolderKind::Search {
            let name = report::short(&f.name);
            report.not_brought.push(Entry {
                what: name.clone(),
                folder: String::new(),
                why: "a search folder: its items are in other folders".into(),
            });
            internal.insert(f.id.0, name);
            continue;
        }
        let path = if Some(f.id) == ipm {
            Vec::new()
        } else {
            let Some(mut path) = f.parent.and_then(|p| paths.get(&p)).cloned() else {
                internal.insert(f.id.0, report::short(&f.name));
                continue;
            };
            // Two folders of one name side by side stay two.
            let base = tidy_name(&f.name);
            let mut name = base.clone();
            let mut n = 1u32;
            while !used.insert((path.clone(), name.to_lowercase())) {
                n = n.saturating_add(1);
                name = format!("{base} ({n})");
            }
            path.push(name);
            path
        };
        paths.insert(f.id, path.clone());
        let mail = matches!(f.kind, FolderKind::Mail | FolderKind::Other(_));
        brought.push(Brought { id: f.id, shown: path.join("/"), path, mail });
    }
    for l in &tree.left_out {
        if l.property.is_some() {
            report.damaged(format!("a property of folder {:#x}: {}", l.id.0, l.reason));
        } else {
            report.damaged(format!("folder {:#x}: {}", l.id.0, l.reason));
        }
    }

    let total: usize = brought.iter().map(|b| nodes.get(&b.id.0).map_or(0, Vec::len)).sum();
    progress.total.store(total, Ordering::Relaxed);
    progress.folders_total.store(brought.len(), Ordering::Relaxed);

    let mut batch = Batch { folders: brought.iter().filter(|b| b.mail).map(|b| b.path.clone()).collect(), ..Default::default() };
    let categories = if job.dry_run { Vec::new() } else { category_list(&mut pst, &tree, &mut report)? };
    let mut claimed: HashSet<u32> = HashSet::new();
    let mut cancelled = false;

    'folders: for b in &brought {
        let mut row = Row { path: b.shown.clone(), ..Default::default() };
        let listed = match pst.items(b.id) {
            Ok(l) => l,
            Err(e) if ends_import(&e) => return Err(pst_error(&e)),
            Err(e) => {
                report.damaged(format!("the list of items of {}: {e}", report::short(&b.shown)));
                mailcraft_pst::ItemList::default()
            }
        };
        // Its items: those its list names, and those the node tree puts in it that the list
        // doesn't (a damaged list).
        let mut queue: Vec<(u32, Option<String>, String)> = Vec::new();
        for i in &listed.items {
            if claimed.insert(i.id.0) {
                let subject = i.properties.get(0x0037).map(|v| summary_text(v, &named)).unwrap_or_default();
                queue.push((i.id.0, i.class.clone(), subject));
            }
        }
        for &nid in nodes.get(&b.id.0).into_iter().flatten() {
            if claimed.insert(nid) {
                queue.push((nid, None, String::new()));
            }
        }
        row.found = queue.len().saturating_add(listed.past_cap);
        if listed.past_cap > 0 {
            row.left_out = listed.past_cap;
            report.leave_out(&format!("{} items", listed.past_cap), &b.shown, "past the number of items one import takes");
        }
        progress.set_folder(&b.shown, queue.len());

        for (nid, class, subject) in queue {
            if progress.is_cancelled() {
                cancelled = true;
                report.rows.push(row);
                break 'folders;
            }
            progress.step();
            let what = |kind: &str| if subject.is_empty() { kind.to_string() } else { format!("{kind} \"{subject}\"") };
            if job.known.contains(&key(&identity, nid)) {
                row.already = row.already.saturating_add(1);
                continue;
            }
            if let Some(kind) = class.as_deref().and_then(later_kind) {
                row.later = row.later.saturating_add(1);
                report.count_later(kind);
                continue;
            }
            if job.dry_run && class.is_some() {
                row.added = row.added.saturating_add(1);
                continue;
            }
            let m = match pst.message(&item_ref(nid)) {
                Ok(m) => m,
                Err(e) if ends_import(&e) => return Err(pst_error(&e)),
                Err(e) => {
                    row.left_out = row.left_out.saturating_add(1);
                    report.leave_out(&what("item"), &b.shown, &e.to_string());
                    continue;
                }
            };
            if let Some(kind) = m.class.as_deref().and_then(later_kind) {
                row.later = row.later.saturating_add(1);
                report.count_later(kind);
                continue;
            }
            if job.dry_run {
                row.added = row.added.saturating_add(1);
                continue;
            }
            let built = item::build(&mut pst, &m, &named).map_err(|e| pst_error(&e))?;
            let mut message = built.message;
            message.headers.push((PST_ITEM_HEADER.to_string(), key(&identity, nid)));
            let shown = if message.subject.is_empty() { "message".to_string() } else { format!("message \"{}\"", message.subject) };
            for n in &built.notes {
                report.note(&shown, &b.shown, n);
            }
            row.added = row.added.saturating_add(1);
            batch.bytes = batch.bytes.saturating_add(built.bytes);
            batch.messages.push((b.path.clone(), message));
            if batch.messages.len() >= BATCH_MESSAGES || batch.bytes >= BATCH_BYTES {
                sink(std::mem::take(&mut batch))?;
            }
        }
        report.rows.push(row);
    }
    if !batch.is_empty() && !job.dry_run {
        sink(std::mem::take(&mut batch))?;
    }

    if !cancelled {
        // Items of the node tree in no folder that was brought: in the file's own folders, in
        // folders that couldn't be read, or in none.
        let unreadable: HashMap<u32, String> =
            tree.left_out.iter().filter(|l| l.property.is_none()).map(|l| (l.id.0, l.reason.to_string())).collect();
        let mut rest: Vec<(&u32, &Vec<u32>)> = nodes.iter().collect();
        rest.sort_unstable_by_key(|(parent, _)| **parent);
        for (parent, items) in rest {
            let left: Vec<u32> = items.iter().copied().filter(|n| !claimed.contains(n)).collect();
            if left.is_empty() {
                continue;
            }
            let (path, why) = match (internal.get(parent), unreadable.get(parent)) {
                (Some(name), _) => (format!("({name})"), format!("in the file's own folder {name}, which isn't brought")),
                (None, Some(reason)) => ("(a folder that couldn't be read)".to_string(), format!("its folder couldn't be read: {reason}")),
                (None, None) => ("(no folder)".to_string(), "its folder isn't in the file".to_string()),
            };
            for nid in &left {
                report.leave_out(&format!("item {nid:#x}"), &path, &why);
            }
            report.rows.push(Row { path, found: left.len(), left_out: left.len(), ..Default::default() });
        }
        if past > 0 {
            report.leave_out(&format!("{past} items"), "", "past the number of items one import takes");
            report.rows.push(Row { path: "(past the limit)".into(), found: past, left_out: past, ..Default::default() });
        }
    }
    for d in pst.damage() {
        let pages = if d.pages > 1 { format!(" ({} pages)", d.pages) } else { String::new() };
        report.damaged(format!("{} at {:#x}{pages}: {}", d.what, d.offset, d.reason));
    }
    report.cancelled = cancelled;
    // Every item found is accounted for, or the import fails.
    if !cancelled && let Some(row) = report.rows.iter().find(|r| !r.adds_up()) {
        return Err(failed!("the import couldn't account for every item of {path}", path = row.path.clone()));
    }
    Ok(Outcome { report, categories })
}

/// A value of an item's summary as text (the subject a list gives).
fn summary_text(v: &PropValue, named: &Named) -> String {
    let text = Text { charset: named.charset.unwrap_or("windows-1252") };
    match v {
        PropValue::String(s) => report::short(s),
        PropValue::String8(b) => report::short(&text.decode(b)),
        _ => String::new(),
    }
}

/// The ids the file gives the named properties the import reads.
fn named<R: Read + Seek>(pst: &mut Pst<R>, charset: Option<&'static str>, report: &mut Report) -> Result<Named, CmdError> {
    match pst.named_properties() {
        Ok(map) => Ok(Named {
            keywords: map.find(&PS_PUBLIC_STRINGS, &PropName::Name("Keywords".into())),
            due: map.find(&item::PSETID_TASK, &PropName::Id(0x8105)),
            charset,
        }),
        Err(e) if ends_import(&e) => Err(pst_error(&e)),
        Err(e) => {
            report.damaged(format!("the file's named properties (categories, due dates): {e}"));
            Ok(Named { charset, ..Named::default() })
        }
    }
}

/// The file's list of categories, with their colours: the associated item that keeps it, in
/// whichever of the owner's folders has it.
fn category_list<R: Read + Seek>(pst: &mut Pst<R>, tree: &mailcraft_pst::FolderTree, report: &mut Report) -> Result<Vec<(String, Rgb)>, CmdError> {
    for entry in tree.folders.iter().filter(|e| e.in_ipm_subtree && e.folder.kind != FolderKind::Search) {
        let items = match pst.associated_items(entry.folder.id) {
            Ok(l) => l.items,
            Err(e) if ends_import(&e) => return Err(pst_error(&e)),
            Err(_) => continue,
        };
        for i in items.iter().filter(|i| i.class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(CATEGORY_LIST))) {
            let r = item_ref(i.id.0);
            let xml = match pst.read_property(&r, 0x7C08, MAX_CATEGORY_XML) {
                Ok(Some(PropValue::Binary(b))) => b,
                Ok(_) => continue,
                Err(e) if ends_import(&e) => return Err(pst_error(&e)),
                Err(e) => {
                    report.damaged(format!("the file's list of categories: {e}"));
                    continue;
                }
            };
            let list = categories(&mailcraft_charset::guess(xml.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&xml)));
            if !list.is_empty() {
                return Ok(list);
            }
        }
    }
    Ok(Vec::new())
}

/// The categories of the XML a .pst file keeps its list of categories in: each `<category>`'s
/// `name` and `color` (0 to 24, as Outlook numbers its colours; grey for none).
pub fn categories(xml: &str) -> Vec<(String, Rgb)> {
    let mut out: Vec<(String, Rgb)> = Vec::new();
    let mut rest = xml;
    while let Some(at) = rest.find("<category") {
        let after = rest.get(at.saturating_add(9)..).unwrap_or_default();
        let end = after.find('>').unwrap_or(after.len());
        let tag = after.get(..end).unwrap_or_default();
        rest = after.get(end..).unwrap_or_default();
        if !tag.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(name) = attribute(tag, "name").and_then(|n| mailcraft_model::clean_name(&n)) else { continue };
        let color = attribute(tag, "color").and_then(|c| c.trim().parse::<i64>().ok()).and_then(mailcraft_model::preset_category_color);
        if !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            out.push((name, color.unwrap_or(mailcraft_model::GREY_CATEGORY)));
        }
        if out.len() >= MAX_CATEGORIES {
            break;
        }
    }
    out
}

/// The value of an attribute of a tag, with XML's references undone.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    loop {
        let at = rest.find(name)?;
        let before = rest.get(..at).and_then(|b| b.chars().last());
        let after = rest.get(at.saturating_add(name.len())..)?.trim_start();
        rest = rest.get(at.saturating_add(name.len())..)?;
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let Some(after) = after.strip_prefix('=') else { continue };
        let after = after.trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let body = after.get(1..)?;
        let value = body.get(..body.find(quote)?)?;
        return Some(unescape(value));
    }
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let after = rest.get(at.saturating_add(1)..).unwrap_or_default();
        let Some(semi) = after.find(';').filter(|i| *i <= 10) else {
            out.push('&');
            rest = after;
            continue;
        };
        let entity = after.get(..semi).unwrap_or_default();
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e => e
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => out.push(c),
            None => {
                out.push('&');
                out.push_str(entity);
                out.push(';');
            }
        }
        rest = after.get(semi.saturating_add(1)..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// Where the mail goes, from `places` (`{"mail": "new" | <folder id>}`): never an account on a
/// server.
pub fn place_of(s: &Session, v: &Value, name: &str) -> Result<Place, CmdError> {
    let mail = v.get("places").and_then(|p| p.get("mail"));
    match mail {
        None | Some(Value::Null) => Ok(Place::New(name.to_string())),
        Some(Value::String(t)) if t == "new" => Ok(Place::New(name.to_string())),
        Some(Value::Number(n)) => {
            let id = n.as_u64().ok_or_else(|| crate::cmd::bad("`places.mail` is a folder id or \"new\""))?;
            let mb = &s.mailbox;
            let folder = mb.folder(id).ok_or_else(|| failed!("no folder with id {id}", id = id))?;
            let local = mb.accounts.iter().any(|a| a.id == folder.account && a.kind == AccountKind::Local);
            if !local || mb.is_send_queue(id) {
                return Err(fail("a .pst file is imported into a folder on this computer only, never an account on a server or an Outbox"));
            }
            Ok(Place::Under(id))
        }
        Some(_) => Err(crate::cmd::bad("`places.mail` is a folder id or \"new\"")),
    }
}

/// Put a batch of read messages into the mailbox, under the place asked for. Quick: no file is
/// read here.
pub fn apply(s: &mut Session, place: &Place, batch: Batch) -> Result<usize, CmdError> {
    let account = match place {
        Place::Under(id) => s.mailbox.folder(*id).map(|f| f.account).ok_or_else(|| failed!("no folder with id {id}", id = *id))?,
        Place::New(_) => s.mailbox.local_account(),
    };
    let mut made = 0usize;
    let top = match place {
        Place::New(name) => child_within(&mut s.mailbox, account, None, name, true, &mut made, MAX_FOLDERS)?,
        Place::Under(id) => Some(*id),
    }
    .ok_or_else(|| fail("the folder for the import couldn't be made"))?;
    let mut cache: HashMap<Vec<String>, Id> = HashMap::new();
    let mut folder_of = |mb: &mut Mailbox, path: &[String]| -> Result<Id, CmdError> {
        if let Some(id) = cache.get(path) {
            return Ok(*id);
        }
        let mut at = top;
        for (i, name) in path.iter().enumerate() {
            let prefix = path.get(..=i).unwrap_or_default().to_vec();
            at = match cache.get(&prefix) {
                Some(id) => *id,
                None => {
                    let id = child_within(mb, account, Some(at), name, true, &mut made, MAX_FOLDERS)?
                        .ok_or_else(|| failed!("the folder {name} couldn't be made", name = name.clone()))?;
                    cache.insert(prefix, id);
                    id
                }
            };
        }
        Ok(at)
    };
    for path in &batch.folders {
        folder_of(&mut s.mailbox, path)?;
    }
    let now = s.now();
    let mut n = 0usize;
    for (path, mut m) in batch.messages {
        let folder = folder_of(&mut s.mailbox, &path)?;
        m.id = 0;
        m.uid = 0;
        m.folder = folder;
        m.draft = false;
        if m.received == 0 {
            m.received = if m.date != 0 { m.date } else { now };
        }
        s.mailbox.add_message(m).map_err(|e| fail(e.to_string()))?;
        n = n.saturating_add(1);
    }
    Ok(n)
}

/// After the last batch: the file's categories added where this computer has no category of
/// that name, the report written beside the mailbox, the folder shown, and the report answered.
pub fn finish(s: &mut Session, place: &Place, outcome: Outcome) -> Result<Value, CmdError> {
    let Outcome { mut report, categories } = outcome;
    let dry = report.dry_run;
    let top = if dry {
        None
    } else {
        let account = s.mailbox.local_account();
        for (name, color) in categories {
            if s.mailbox.category_of(account, &name).is_none()
                && s.mailbox.categories_of(account).len() < MAX_CATEGORIES
                && s.mailbox.set_category(account, &name, color).is_ok()
            {
                report.categories_added = report.categories_added.saturating_add(1);
            }
        }
        match place {
            Place::New(name) => {
                let mut made = 0;
                child_within(&mut s.mailbox, account, None, name, true, &mut made, MAX_FOLDERS)?
            }
            Place::Under(id) => Some(*id),
        }
    };
    if let Some(id) = top {
        report.name = s.mailbox.folder(id).map(|f| f.name.clone()).unwrap_or_default();
        s.view.folder = id;
        s.view.module = crate::Module::Mail;
        s.view.search.clear();
    }
    if !dry {
        report.written = write_report(s, &report);
    }
    let mut answer = report.to_json();
    if let Some(o) = answer.as_object_mut() {
        o.insert("folder".into(), json!(top));
    }
    Ok(answer)
}

/// The report as a text file beside the mailbox file, when there is one: its path.
#[cfg(not(target_arch = "wasm32"))]
fn write_report(s: &Session, report: &Report) -> Option<String> {
    let dir = s.path.as_ref()?.parent()?.to_path_buf();
    let when = mailcraft_time::format_iso_utc(s.now());
    let stem = format!("{} import report {}", folder_name(&report.file), when.replace(':', "-"));
    let path = dir.join(crate::opening::export_name_or(&stem, "txt", "pst import report"));
    std::fs::write(&path, report.to_text(&when)).ok()?;
    Some(path.display().to_string())
}

#[cfg(target_arch = "wasm32")]
fn write_report(_: &Session, _: &Report) -> Option<String> {
    None
}

/// `file.importPst`: see the command's params.
pub fn run(s: &mut Session, v: &Value) -> Result<Value, CmdError> {
    if p::bool(v, "staged").unwrap_or(false) {
        // Read by the window, off its thread, and handed over a batch at a time.
        let name = p::str(v, "name").map(tidy_name).unwrap_or_else(|| "Imported .pst".to_string());
        let place = place_of(s, v, &name)?;
        return match s.staged_pst.take().ok_or_else(|| fail("nothing is waiting to be imported"))? {
            Staged::Batch(batch) => Ok(json!({"staged": apply(s, &place, batch)?})),
            Staged::Done(outcome) => finish(s, &place, *outcome),
        };
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (base64_decode, Limits::default);
        Err(fail("importing a .pst file isn't supported in the browser yet"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dry_run = p::bool(v, "dryRun").unwrap_or(false);
        let password = p::str(v, "password").map(str::to_string);
        let file = match (p::str(v, "path"), p::str(v, "name")) {
            (Some(path), _) => path.rsplit(['/', '\\']).next().unwrap_or(path).to_string(),
            (None, Some(name)) => name.to_string(),
            (None, None) => String::new(),
        };
        let name = folder_name(&file);
        let place = place_of(s, v, &name)?;
        let job = Job { password, dry_run, file: report::short(&file), name, known: known_keys(&s.mailbox) };
        let progress = Progress::default();
        let mut sink = |batch: Batch| -> Result<(), CmdError> {
            if !dry_run {
                apply(s, &place, batch)?;
            }
            Ok(())
        };
        let outcome = if let Some(data) = p::str(v, "data") {
            let bytes = base64_decode(data)?;
            read(std::io::Cursor::new(bytes), &job, Limits::default(), &progress, &mut sink)?
        } else if let Some(path) = p::str(v, "path") {
            let f = crate::io::regular_file(std::path::Path::new(path)).map_err(|e| failed!("the .pst file couldn't be opened: {e}", e = e.to_string()))?;
            read(std::io::BufReader::new(f), &job, Limits::default(), &progress, &mut sink)?
        } else {
            return Err(crate::cmd::bad("name the .pst file (`path`), or hand it over as `name` and base64 `data`"));
        };
        finish(s, &place, outcome)
    }
}
