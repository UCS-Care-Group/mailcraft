// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Importing a .pst file (`file.importPst`, #815): the mail of every folder the owner sees in it, Deleted Items and Junk E-mail too,
//! as ordinary folders on this computer, and a report that accounts for every item found.
//!
//! The file is read by `mailcraft-pst` from [MS-PST], over `Read + Seek`, never whole into
//! memory; each item is read into a message from [MS-OXCMSG], [MS-OXPROPS] and [MS-OXOMSG]
//! ([`item`]). Mail lands in local folders alone, never in an account on a server, and nothing
//! of it leaves the computer. Contacts, calendar items, tasks, notes and journal entries come
//! onto this computer too ([`contact`], [`calendar`]).
//!
//! The work ([`read`]) takes no `Session`, so that a host runs it on a thread: it hands what it
//! has read over in batches, which go into the mailbox as they come ([`apply`]); cancelling
//! keeps what is in, and another run of the same file brings the rest, as every item brought in
//! remembers where in the file it was ([`mailcraft_mime::PST_ITEM_HEADER`]: the file's identity
//! and the item's node id) and is counted "already here".
//!
//! The import fails unless every item found is accounted for: brought in, already here, left
//! out (with what it is, its folder and why) or not yet brought in.

pub mod calendar;
pub mod contact;
pub mod item;
pub mod report;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use mailcraft_model::{AccountKind, Calendar, Contact, Event as CalendarEvent, Id, MAX_FOLDER_DEPTH, Mailbox, Message, Rgb, Task};
use mailcraft_pst::{Error as PstError, FolderKind, Limits, Nid, PS_PUBLIC_STRINGS, PropName, PropValue, Pst, node_types};
use serde_json::{Value, json};

use crate::cmd::{base64_decode, fail};
use crate::import_tree::tidy_name;
use crate::{CmdError, Session, p};
use item::{Named, Text, ends_import, item_ref};
use report::{Report, Row};

pub use mailcraft_mime::PST_ITEM_HEADER;

/// Most messages handed over in one batch.
pub const BATCH_MESSAGES: usize = if cfg!(test) { 3 } else { 500 };
/// Most bytes (text and attachments) of one batch.
pub const BATCH_BYTES: u64 = 64 << 20;
/// Most bytes an import may add to memory at once, over all its batches (#815). With a mailbox
/// file beside it, every attachment over 1 KB goes to the content folder as its batch is read
/// (by the reader's thread) and only what stays in memory counts: attachments the disk refused
/// and the small ones. Without a file, everything an import adds counts (the text, HTML and
/// hidden words of each message and every attachment, a file carried by many messages once), and
/// an import that would pass this stops with an error saying the file is too large to import
/// without a mailbox file, rather than run the computer out of memory.
pub const MEMORY_BUDGET: u64 = if cfg!(test) { 1 << 20 } else { 256 << 20 };
/// Attachments this small stay in their message (the size the mailbox file keeps inline).
#[cfg(not(target_arch = "wasm32"))]
const SPILL_MIN: usize = 1024;
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
        // (An item a folder's list names that the node tree doesn't is one more than was counted.)
        let done = self.done.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        self.total.fetch_max(done, Ordering::Relaxed);
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
    /// How many levels of the file's folders fit below the place ([`room`]): a folder nested
    /// deeper has its mail put in its ancestor at the last level, and its row says so.
    pub room: usize,
    /// The contacts already brought in from .pst files ([`known_keys`] holds them too), to tell
    /// them from mail.
    pub known_contacts: HashSet<String>,
    /// The events and to-dos already brought in from .pst files.
    pub known_plans: HashSet<String>,
    /// How much of its notes and journal entries this import may keep ([`kept_room`]).
    pub kept_room: u64,
    /// The mailbox file the mail is going to, when it has one: the reader writes each batch's
    /// attachments to the content folder beside it before it hands the batch over (#815).
    pub keep_at: Option<std::path::PathBuf>,
    /// The calendar on this computer the file's calendar items go into; `None` for a new one
    /// named after the file.
    pub calendar: Option<Id>,
    /// The clock of the calendar.
    pub here: calendar::Here,
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
    /// Contacts and distribution lists (as contacts), read the same way.
    pub contacts: Vec<Contact>,
    /// Calendar items, each with the calendar asked for (0: a new one, [`Batch::calendar_name`]).
    pub events: Vec<CalendarEvent>,
    pub tasks: Vec<Task>,
    pub notes: Vec<mailcraft_model::Note>,
    pub journal: Vec<mailcraft_model::JournalEntry>,
    /// The name of the calendar made for the file, when one is.
    pub calendar_name: String,
    /// The file's identity: a batch of another file starts an import of its own.
    pub identity: String,
    bytes: u64,
}

impl Batch {
    fn is_empty(&self) -> bool {
        self.folders.is_empty()
            && self.messages.is_empty()
            && self.contacts.is_empty()
            && self.events.is_empty()
            && self.tasks.is_empty()
            && self.notes.is_empty()
            && self.journal.is_empty()
    }

    /// Items in it, for the size of a batch.
    fn items(&self) -> usize {
        self.messages
            .len()
            .saturating_add(self.contacts.len())
            .saturating_add(self.events.len())
            .saturating_add(self.tasks.len())
            .saturating_add(self.notes.len())
            .saturating_add(self.journal.len())
    }
}

/// What a whole read came to: the report, and the file's list of categories.
pub struct Outcome {
    pub report: Report,
    pub categories: Vec<(String, Rgb)>,
    pub identity: String,
}

/// What the window has waiting for `file.importPst` with `staged`.
// (One at a time, moved once a batch: its size costs nothing.)
#[allow(clippy::large_enum_variant)]
pub enum Staged {
    Batch(Batch),
    Done(Box<Outcome>),
}

/// What the thread of an import hands the window, in turn.
pub enum Event {
    Batch(Batch),
    Done(Result<Box<Outcome>, CmdError>),
}

/// Read the .pst file at `path` (on the import's own thread) and hand each batch, then the
/// outcome, to `send`; a `send` that answers `false` (the window has gone) ends the read.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_path(path: &str, job: &Job, progress: &Progress, send: &mut dyn FnMut(Event) -> bool) {
    // A panic on the way is an import that couldn't go on, said to whoever reads the events,
    // not a crash of whatever thread this is (#815).
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(test)]
        if path.ends_with("panic-in-reader.pst") {
            std::panic::resume_unwind(Box::new("a reader that fell over"));
        }
        let mut sink = |batch: Batch| if send(Event::Batch(batch)) { Ok(()) } else { Err(fail("the import was stopped")) };
        open_path(path).and_then(|f| read(f, job, Limits::default(), progress, &mut sink))
    }))
    .unwrap_or_else(|_| Err(fail("MailCraft stopped reading the .pst file before it was done: what came in is kept")));
    let _ = send(Event::Done(done.map(Box::new)));
}

#[cfg(not(target_arch = "wasm32"))]
fn open_path(path: &str) -> Result<std::io::BufReader<std::fs::File>, CmdError> {
    crate::io::regular_file(std::path::Path::new(path))
        .map(std::io::BufReader::new)
        .map_err(|e| failed!("the .pst file couldn't be opened: {e}", e = e.to_string()))
}

/// The keys of every message and contact brought in from a .pst file ([`PST_ITEM_HEADER`],
/// [`Contact::imported`]).
pub fn known_keys(mb: &Mailbox) -> HashSet<String> {
    let messages =
        mb.messages.values().filter_map(|m| m.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(PST_ITEM_HEADER)).map(|(_, v)| v.clone()));
    messages.chain(known_plans_and_people(mb)).collect()
}

/// The keys of what came from .pst files and isn't mail: contacts, events and to-dos.
fn known_plans_and_people(mb: &Mailbox) -> impl Iterator<Item = String> + '_ {
    let contacts = mb.contacts.values().flat_map(|c| c.imported.split_whitespace().map(str::to_string));
    let events = mb.events.values().filter(|e| !e.imported.is_empty()).map(|e| e.imported.clone());
    let tasks = mb.tasks.values().filter(|t| !t.imported.is_empty()).map(|t| t.imported.clone());
    let notes = mb.notes.values().filter(|n| !n.imported.is_empty()).map(|n| n.imported.clone());
    let journal = mb.journal.values().filter(|j| !j.imported.is_empty()).map(|j| j.imported.clone());
    contacts.chain(events).chain(tasks).chain(notes).chain(journal)
}

/// The identity of a file: its provider's id (PidTagRecordKey), which every copy of it keeps.
/// So does a copy that has gone its own way since (kept on another computer, added to): its
/// items of the same node ids count as already here, and only its new ones come in. A file
/// without a record key is known by its size, its name inside and the start of the file (its
/// header, `head`), which changes when the file does: a changed copy of such a file is another.
fn identity<R: Read + Seek>(pst: &Pst<R>, store: &mailcraft_pst::Store, head: &[u8]) -> String {
    match &store.record_key {
        Some(k) if !k.is_empty() => k.iter().take(32).map(|b| format!("{b:02x}")).collect(),
        _ => {
            let name = store.display_name.clone().unwrap_or_default();
            format!("size{}-{:08x}-{:08x}", pst.sizes().1, mailcraft_pst::crc32(name.as_bytes()), mailcraft_pst::crc32(head))
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
    /// Nested deeper than the room there is: its mail goes into its ancestor at `path`.
    merged: bool,
    /// In Deleted Items or below it: its contacts are left out.
    deleted: bool,
    /// Hidden, or one Outlook fills by itself (a contacts folder of a class of its own): its
    /// contacts come in, and the report says where they came from.
    automatic: bool,
}

/// Most bytes of the start of a file its identity is worked out from when it has no record key.
const HEAD_BYTES: u64 = 4096;

/// Writes each batch's attachments to the content folder beside the mailbox file before the
/// batch is handed over (#815), on the reader's thread: the window's thread only takes the batch
/// in. Each file is looked at and written (or its time renewed) once per carrier, so content
/// that went missing between batches is written again. A disk that refuses is left alone for the
/// rest of the batch (what is left stays in memory, against [`MEMORY_BUDGET`]) and said once.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Spiller {
    path: Option<std::path::PathBuf>,
    said: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Spiller {
    pub(crate) fn new(path: Option<std::path::PathBuf>) -> Spiller {
        Spiller { path, said: false }
    }

    pub(crate) fn spill(&mut self, batch: &mut Batch) {
        let Some(path) = self.path.as_deref() else { return };
        let mut refused = false;
        for (_, m) in &mut batch.messages {
            for a in &mut m.attachments {
                if refused {
                    return;
                }
                if a.kept.is_some() || a.data.len() <= SPILL_MIN {
                    continue;
                }
                let digest = crate::store::Digest::of(&a.data);
                match crate::store::stash(path, &digest, &a.data) {
                    Ok(r) => {
                        a.kept = Some(r);
                        a.data = std::sync::Arc::default();
                    }
                    Err(e) => {
                        refused = true;
                        if !self.said {
                            self.said = true;
                            log::warn!("a .pst import couldn't write attachments ahead: {e}; they are held in memory");
                        }
                    }
                }
            }
        }
    }
}

/// Read a .pst file over `reader` and hand over its mail in batches through `sink` (nothing,
/// for a dry run): see the module. `limits` are the reader's caps ([`Limits::default`] for an
/// import).
pub fn read<R: Read + Seek>(
    mut reader: R,
    job: &Job,
    limits: Limits,
    progress: &Progress,
    sink: &mut dyn FnMut(Batch) -> Result<(), CmdError>,
) -> Result<Outcome, CmdError> {
    // (With a mailbox file, the attachments of each batch are written beside it on the way.)
    #[cfg(not(target_arch = "wasm32"))]
    let mut spiller = Spiller::new(job.keep_at.clone());
    let mut written_ahead = |batch: Batch| {
        #[cfg(not(target_arch = "wasm32"))]
        let batch = {
            let mut batch = batch;
            spiller.spill(&mut batch);
            batch
        };
        sink(batch)
    };
    let sink = &mut written_ahead;
    // The start of the file, for its identity when it has no record key.
    let mut head = Vec::new();
    let read_head = (&mut reader).take(HEAD_BYTES).read_to_end(&mut head).and_then(|_| reader.seek(std::io::SeekFrom::Start(0)));
    read_head.map_err(|e| failed!("the .pst file couldn't be read: {why}", why = e.to_string()))?;
    let mut pst = Pst::open_with(reader, limits.clone()).map_err(|e| pst_error(&e))?;
    if pst.password_required() {
        let Some(password) = job.password.as_deref() else { return Err(pst_error(&PstError::PasswordRequired)) };
        pst.unlock(password).map_err(|e| pst_error(&e))?;
    }
    let store = pst.store().map_err(|e| pst_error(&e))?;
    let identity = identity(&pst, &store, &head);
    let mut report = Report {
        file: job.file.clone(),
        name: job.name.clone(),
        dry_run: job.dry_run,
        format: format_name(pst.format()).into(),
        ..Default::default()
    };

    let tree = pst.walk_folders().map_err(|e| pst_error(&e))?;
    let store_charset = store.properties.int(0x3FFD).and_then(item::code_page);
    let named = named(&mut pst, store_charset, job.kept_room, &mut report)?;

    // Every item of the node tree, by its folder: what the folders' lists are checked against.
    // The one cut of the number of items: what is past it here is past it everywhere.
    let mut nodes: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut within: HashSet<u32> = HashSet::new();
    let mut past = 0usize;
    let walked = pst.for_each_node(|n| {
        if n.id.kind() == node_types::NORMAL_MESSAGE {
            if within.len() < limits.max_items {
                within.insert(n.id.0);
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
    let mut internal: HashSet<u32> = HashSet::new();
    // Deleted Items (by the store's own word) and the folders below it.
    let mut deleted: HashSet<Nid> = HashSet::new();
    let mut used: HashSet<(Vec<String>, String)> = HashSet::new();
    // The next number to try for a name already used side by side, so that a file of thousands
    // of folders of one name is not tried from (2) up each time.
    let mut next: HashMap<(Vec<String>, String), u32> = HashMap::new();
    // Folders nested deeper than the room there is: what they are below their place, as shown.
    let mut deep: HashMap<Nid, String> = HashMap::new();
    for entry in &tree.folders {
        let f = &entry.folder;
        if !entry.in_ipm_subtree {
            let name = if f.name.trim().is_empty() { "(the root)".to_string() } else { f.name.clone() };
            report.not_brought(&name, "the file's own folder (its root, searches or views)");
            internal.insert(f.id.0);
            continue;
        }
        if f.kind == FolderKind::Search {
            report.not_brought(&f.name, "a search folder: its items are in other folders");
            internal.insert(f.id.0);
            continue;
        }
        let (path, shown, merged) = if Some(f.id) == ipm {
            (Vec::new(), String::new(), false)
        } else {
            let Some(path) = f.parent.and_then(|p| paths.get(&p)).cloned() else {
                internal.insert(f.id.0);
                continue;
            };
            let parent_deep = f.parent.and_then(|p| deep.get(&p)).cloned();
            if path.len() >= job.room {
                // No room for it here: its mail goes into its ancestor at the deepest level there is.
                let base = parent_deep.unwrap_or_else(|| path.join("/"));
                let shown = report::short_path(&format!("{base}/{}", tidy_name(&f.name)));
                deep.insert(f.id, shown.clone());
                (path, shown, true)
            } else {
                // Two folders of one name side by side stay two.
                let base = tidy_name(&f.name);
                let lower = base.to_lowercase();
                let mut path = path;
                let name = if used.insert((path.clone(), lower.clone())) {
                    base
                } else {
                    let n = next.entry((path.clone(), lower)).or_insert(1);
                    loop {
                        *n = n.saturating_add(1);
                        let name = format!("{base} ({n})");
                        if used.insert((path.clone(), name.to_lowercase())) || *n == u32::MAX {
                            break name;
                        }
                    }
                };
                path.push(name);
                let shown = path.join("/");
                (path, shown, false)
            }
        };
        paths.insert(f.id, path.clone());
        let mail = matches!(f.kind, FolderKind::Mail | FolderKind::Other(_));
        // Contacts in Deleted Items, and below it, are left out; every other folder's come in.
        let in_deleted = Some(f.id) == store.deleted_items || f.parent.is_some_and(|p| deleted.contains(&p));
        if in_deleted {
            deleted.insert(f.id);
        }
        let own_contacts =
            f.kind == FolderKind::Contacts && f.container_class.as_deref().is_some_and(|c| c.trim().eq_ignore_ascii_case("IPF.Contact"));
        let automatic = f.properties.bool(0x10F4).unwrap_or(false) || (f.kind == FolderKind::Contacts && !own_contacts);
        brought.push(Brought { id: f.id, shown, path, mail, merged, deleted: in_deleted, automatic });
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

    let mut batch = Batch { identity: identity.clone(), ..Default::default() };
    // The file's empty mail folders, made at the end unless the file holds contacts and no mail (a file of
    // contacts alone, with an empty Inbox and Deleted Items, makes none).
    let empty_folders: Vec<Vec<String>> = brought.iter().filter(|b| b.mail && !b.merged && !b.path.is_empty()).map(|b| b.path.clone()).collect();
    let categories = if job.dry_run { Vec::new() } else { category_list(&mut pst, &tree, &mut report)? };
    let mut claimed: HashSet<u32> = HashSet::new();
    let mut cancelled = false;

    'folders: for b in &brought {
        let into = b.merged.then(|| b.path.join("/"));
        let mut row = Row { path: b.shown.clone(), into, ..Default::default() };
        let (mut in_deleted, mut told) = (0usize, false);
        let listed = match pst.items(b.id) {
            Ok(l) => l,
            Err(e) if ends_import(&e) => return Err(pst_error(&e)),
            Err(e) => {
                report.damaged(format!("the list of items of {}: {e}", report::short(&b.shown)));
                mailcraft_pst::ItemList::default()
            }
        };
        // Its items: those its list names, and those the node tree puts in it that the list
        // doesn't (a damaged list, or one longer than the reader lists). An item past the one
        // cut of the node tree is counted there, not here.
        let mut queue: Vec<(u32, Option<String>, String)> = Vec::new();
        for i in listed.items {
            if (past == 0 || within.contains(&i.id.0)) && claimed.insert(i.id.0) {
                let subject = i.properties.get(0x0037).map(|v| summary_text(v, &named)).unwrap_or_default();
                queue.push((i.id.0, i.class, subject));
            }
        }
        for &nid in nodes.get(&b.id.0).into_iter().flatten() {
            if claimed.insert(nid) {
                queue.push((nid, None, String::new()));
            }
        }
        row.found = queue.len();
        progress.set_folder(&b.shown, queue.len());

        for (nid, class, subject) in queue {
            if progress.is_cancelled() {
                cancelled = true;
                report.push_row(row);
                break 'folders;
            }
            progress.step();
            let what = |kind: &str| if subject.is_empty() { kind.to_string() } else { format!("{kind} \"{subject}\"") };
            if job.known.contains(&key(&identity, nid)) {
                row.already = row.already.saturating_add(1);
                if job.known_contacts.contains(&key(&identity, nid)) {
                    report.contacts_seen = report.contacts_seen.saturating_add(1);
                } else if job.known_plans.contains(&key(&identity, nid)) {
                    report.plans_seen = report.plans_seen.saturating_add(1);
                } else {
                    report.mail_seen = report.mail_seen.saturating_add(1);
                }
                continue;
            }
            // (A dry run reads each item too, so that what it says would be brought in would be.)
            let m = match pst.message(&item_ref(nid)) {
                Ok(m) => m,
                Err(e) if ends_import(&e) => return Err(pst_error(&e)),
                Err(e) => {
                    row.left_out = row.left_out.saturating_add(1);
                    report.leave_out(&what("item"), &b.shown, &e.to_string());
                    if class.as_deref().and_then(calendar::kind).is_some() {
                        report.plans_seen = report.plans_seen.saturating_add(1);
                    } else if class.as_deref().and_then(contact::kind).is_none() {
                        report.mail_seen = report.mail_seen.saturating_add(1);
                    }
                    continue;
                }
            };
            // An appointment or a task. (Neither is brought back from Deleted Items.)
            if let Some(what_kind) = m.class.as_deref().and_then(calendar::kind) {
                report.plans_seen = report.plans_seen.saturating_add(1);
                if b.deleted {
                    row.left_out = row.left_out.saturating_add(1);
                    in_deleted = in_deleted.saturating_add(1);
                    continue;
                }
                let k = key(&identity, nid);
                match what_kind {
                    calendar::Kind::Appointment => {
                        let built = match calendar::build(&mut pst, &m, &named, job.here).map_err(|e| pst_error(&e))? {
                            Ok(built) => built,
                            Err(why) => {
                                row.left_out = row.left_out.saturating_add(1);
                                report.leave_out(&what("appointment"), &b.shown, why);
                                continue;
                            }
                        };
                        row.added = row.added.saturating_add(1);
                        report.events_added = report.events_added.saturating_add(1);
                        let mut e = built.event;
                        let shown = if e.title.is_empty() { "appointment".to_string() } else { format!("appointment \"{}\"", e.title) };
                        for n in &built.notes {
                            report.note(&shown, &b.shown, n);
                        }
                        if job.dry_run {
                            continue;
                        }
                        e.imported.clone_from(&k);
                        e.calendar = job.calendar.unwrap_or(0);
                        batch.calendar_name.clone_from(&job.name);
                        batch.bytes = batch.bytes.saturating_add(calendar::held(&e));
                        for (mark, mut one) in built.changed {
                            // Known by itself: it may outlive its series, which then comes in again.
                            one.imported = format!("{k}/{mark}");
                            if job.known_plans.contains(&one.imported) {
                                continue;
                            }
                            one.calendar = e.calendar;
                            batch.bytes = batch.bytes.saturating_add(calendar::held(&one));
                            batch.events.push(one);
                        }
                        batch.events.push(e);
                    }
                    calendar::Kind::Task => {
                        let (mut t, notes) = match calendar::build_task(&mut pst, &m, &named).map_err(|e| pst_error(&e))? {
                            Ok(built) => built,
                            Err(why) => {
                                row.left_out = row.left_out.saturating_add(1);
                                report.leave_out(&what("task"), &b.shown, why);
                                continue;
                            }
                        };
                        row.added = row.added.saturating_add(1);
                        report.tasks_added = report.tasks_added.saturating_add(1);
                        let shown = if t.title.is_empty() { "task".to_string() } else { format!("task \"{}\"", t.title) };
                        for n in &notes {
                            report.note(&shown, &b.shown, n);
                        }
                        if job.dry_run {
                            continue;
                        }
                        t.imported = k;
                        batch.bytes = batch.bytes.saturating_add(calendar::held_task(&t));
                        batch.tasks.push(t);
                    }
                    calendar::Kind::Note => {
                        let (mut n, notes) = match calendar::build_note(&mut pst, &m, &named).map_err(|e| pst_error(&e))? {
                            Ok(built) => built,
                            Err(why) => {
                                row.left_out = row.left_out.saturating_add(1);
                                report.leave_out(&what("note"), &b.shown, why);
                                continue;
                            }
                        };
                        row.added = row.added.saturating_add(1);
                        report.notes_added = report.notes_added.saturating_add(1);
                        for why in &notes {
                            report.note(&what("note"), &b.shown, why);
                        }
                        if job.dry_run {
                            continue;
                        }
                        n.imported = k;
                        batch.bytes = batch.bytes.saturating_add(n.text.len() as u64);
                        batch.notes.push(n);
                    }
                    calendar::Kind::Journal => {
                        let (mut j, notes) = match calendar::build_journal(&mut pst, &m, &named).map_err(|e| pst_error(&e))? {
                            Ok(built) => built,
                            Err(why) => {
                                row.left_out = row.left_out.saturating_add(1);
                                report.leave_out(&what("journal entry"), &b.shown, why);
                                continue;
                            }
                        };
                        row.added = row.added.saturating_add(1);
                        report.journal_added = report.journal_added.saturating_add(1);
                        for why in &notes {
                            report.note(&what("journal entry"), &b.shown, why);
                        }
                        if job.dry_run {
                            continue;
                        }
                        j.imported = k;
                        batch.bytes = batch.bytes.saturating_add(j.text.len() as u64);
                        batch.journal.push(j);
                    }
                }
                if batch.items() >= BATCH_MESSAGES || batch.bytes >= BATCH_BYTES {
                    let identity = batch.identity.clone();
                    sink(std::mem::replace(&mut batch, Batch { identity, ..Default::default() }))?;
                }
                continue;
            }
            // A contact, or a distribution list kept as a contact.
            if let Some(list) = m.class.as_deref().and_then(contact::kind) {
                report.contacts_seen = report.contacts_seen.saturating_add(1);
                if b.deleted {
                    // (One line for the folder, not one for each.)
                    row.left_out = row.left_out.saturating_add(1);
                    in_deleted = in_deleted.saturating_add(1);
                    continue;
                }
                if b.automatic && !told {
                    told = true;
                    report.note(
                        &b.shown,
                        &b.shown,
                        "a hidden or automatic contacts folder (recipient cache, suggested contacts): its contacts are brought in",
                    );
                }
                // (A dry run reads it too, so that what it says would come in does.)
                let built = contact::build(&mut pst, &m, &named, list).map_err(|e| pst_error(&e))?;
                row.added = row.added.saturating_add(1);
                if list {
                    report.lists_added = report.lists_added.saturating_add(1);
                } else {
                    report.contacts_added = report.contacts_added.saturating_add(1);
                }
                let mut c = built.contact;
                c.imported = key(&identity, nid);
                c.imported_from = job.file.chars().take(mailcraft_model::MAX_NAME).collect();
                let shown = if c.name().is_empty() { "contact".to_string() } else { format!("contact \"{}\"", c.name()) };
                if let Some(n) = built.members {
                    report.note(&shown, &b.shown, &format!("a distribution list, kept as a contact listing its members ({n})"));
                }
                for n in &built.notes {
                    report.note(&shown, &b.shown, n);
                }
                if job.dry_run {
                    continue;
                }
                batch.bytes = batch.bytes.saturating_add(item::held_contact(&c));
                batch.contacts.push(c);
                if batch.items() >= BATCH_MESSAGES || batch.bytes >= BATCH_BYTES {
                    let identity = batch.identity.clone();
                    sink(std::mem::replace(&mut batch, Batch { identity, ..Default::default() }))?;
                }
                continue;
            }
            report.mail_seen = report.mail_seen.saturating_add(1);
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
            batch.bytes = batch.bytes.saturating_add(item::held(&message));
            batch.messages.push((b.path.clone(), message));
            if batch.items() >= BATCH_MESSAGES || batch.bytes >= BATCH_BYTES {
                let identity = batch.identity.clone();
                sink(std::mem::replace(&mut batch, Batch { identity, ..Default::default() }))?;
            }
        }
        if in_deleted > 0 {
            report.leave_out(
                &format!("contacts, distribution lists, calendar items and tasks ({in_deleted})"),
                &b.shown,
                "in Deleted Items, left out",
            );
        }
        report.push_row(row);
    }
    if (cancelled || !(report.contacts_seen.saturating_add(report.plans_seen) > 0 && report.mail_seen == 0)) && !job.dry_run {
        batch.folders = empty_folders;
    }
    if !batch.is_empty() && !job.dry_run {
        sink(std::mem::take(&mut batch))?;
    }

    if !cancelled {
        // Items of the node tree in no folder that was brought, one row for each reason: in the
        // file's own folders, in folders that couldn't be read, or in none.
        let unreadable: HashMap<u32, String> =
            tree.left_out.iter().filter(|l| l.property.is_none()).map(|l| (l.id.0, l.reason.to_string())).collect();
        let mut rest: Vec<(&u32, &Vec<u32>)> = nodes.iter().collect();
        rest.sort_unstable_by_key(|(parent, _)| **parent);
        let mut orphans = [
            Row { path: "(the file's own folders)".into(), ..Default::default() },
            Row { path: "(folders that couldn't be read)".into(), ..Default::default() },
            Row { path: "(no folder)".into(), ..Default::default() },
        ];
        for (parent, items) in rest {
            let left: Vec<u32> = items.iter().copied().filter(|n| !claimed.contains(n)).collect();
            if left.is_empty() {
                continue;
            }
            let (at, why) = if internal.contains(parent) {
                (0, "in the file's own folder, which isn't brought".to_string())
            } else if let Some(reason) = unreadable.get(parent) {
                (1, format!("its folder couldn't be read: {reason}"))
            } else {
                (2, "its folder isn't in the file".to_string())
            };
            let Some(row) = orphans.get_mut(at) else { continue };
            for nid in &left {
                report.leave_out(&format!("item {nid:#x}"), &row.path, &why);
            }
            row.found = row.found.saturating_add(left.len());
            row.left_out = row.left_out.saturating_add(left.len());
        }
        for row in orphans.into_iter().filter(|r| r.found > 0) {
            report.push_row(row);
        }
        if past > 0 {
            report.leave_out(&format!("{past} items"), "", "past the number of items one import takes");
            report.push_row(Row { path: "(past the limit)".into(), found: past, left_out: past, ..Default::default() });
        }
    }
    for d in pst.damage() {
        let pages = if d.pages > 1 { format!(" ({} pages)", d.pages) } else { String::new() };
        report.damaged(format!("{} at {:#x}{pages}: {}", d.what, d.offset, d.reason));
    }
    report.cancelled = cancelled;
    // Every item found is accounted for, or the import fails.
    if !cancelled && let Some(path) = report.first_short() {
        return Err(failed!("the import couldn't account for every item of {path}", path = path));
    }
    Ok(Outcome { report, categories, identity })
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
fn named<R: Read + Seek>(pst: &mut Pst<R>, charset: Option<&'static str>, kept_room: u64, report: &mut Report) -> Result<Named, CmdError> {
    match pst.named_properties() {
        Ok(map) => Ok(Named {
            keywords: map.find(&PS_PUBLIC_STRINGS, &PropName::Name("Keywords".into())),
            due: map.find(&item::PSETID_TASK, &PropName::Id(0x8105)),
            charset,
            contact: contact::ContactIds::find(map),
            rtf_room: std::cell::Cell::new(item::RTF_TEXT_BUDGET),
            plan_room: std::cell::Cell::new(item::PLAN_TEXT_BUDGET),
            kept_room: std::cell::Cell::new(kept_room),
            calendar: calendar::CalendarIds::find(map),
        }),
        Err(e) if ends_import(&e) => Err(pst_error(&e)),
        Err(e) => {
            report.damaged(format!("the file's named properties (categories, due dates): {e}"));
            Ok(Named {
                charset,
                rtf_room: std::cell::Cell::new(item::RTF_TEXT_BUDGET),
                plan_room: std::cell::Cell::new(item::PLAN_TEXT_BUDGET),
                ..Named::default()
            })
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

/// The calendar asked for (`places.calendar`): one on this computer, or `None` for a new one.
fn calendar_place(s: &Session, v: &Value) -> Result<Option<Id>, CmdError> {
    match v.get("places").and_then(|p| p.get("calendar")) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(t)) if t == "new" => Ok(None),
        Some(Value::Number(n)) => {
            let id = n.as_u64().ok_or_else(|| crate::cmd::bad("`places.calendar` is a calendar id or \"new\""))?;
            let calendar = s.mailbox.calendar(id).ok_or_else(|| failed!("no calendar with id {id}", id = id))?;
            if !calendar.origin.is_local() {
                return Err(fail("a .pst file is imported into a calendar on this computer only, never an account on a server"));
            }
            Ok(Some(id))
        }
        Some(_) => Err(crate::cmd::bad("`places.calendar` is a calendar id or \"new\"")),
    }
}

/// How many levels of the file's folders fit below `place`: folders here nest at most
/// [`MAX_FOLDER_DEPTH`] deep, and a new folder for the file is one of them.
pub fn room(s: &Session, place: &Place) -> usize {
    match place {
        Place::New(_) => MAX_FOLDER_DEPTH.saturating_sub(1),
        Place::Under(id) => MAX_FOLDER_DEPTH.saturating_sub(s.mailbox.depth(*id)),
    }
}

/// What an import may keep of notes and journal entries: they are saved in one record with the
/// rest of the mailbox, so what earlier imports kept counts, as it is written.
fn kept_room(mb: &Mailbox) -> u64 {
    let written = |j: serde_json::Result<String>| j.map_or(0, |j| j.len() as u64);
    let notes = mb.notes.values().map(|n| written(serde_json::to_string(n)));
    let journal = mb.journal.values().map(|e| written(serde_json::to_string(e)));
    let held = notes.chain(journal).fold(0, u64::saturating_add);
    item::KEPT_TOTAL_BUDGET.saturating_sub(held).min(item::KEPT_TEXT_BUDGET)
}

/// The job of importing `file` (its name or its path) with the place `v`'s `places` asks for.
pub fn job(s: &Session, file: &str, password: Option<String>, dry_run: bool, v: &Value) -> Result<(Job, Place), CmdError> {
    let file = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let name = folder_name(file);
    let place = place_of(s, v, &name)?;
    let room = room(s, &place);
    let known_contacts = s.mailbox.contacts.values().flat_map(|c| c.imported.split_whitespace().map(str::to_string)).collect();
    let known_plans = known_plans_and_people(&s.mailbox).collect::<HashSet<String>>();
    let calendar = calendar_place(s, v)?;
    Ok((
        Job {
            password,
            dry_run,
            file: report::short(file),
            name,
            known: known_keys(&s.mailbox),
            room,
            known_contacts,
            known_plans,
            kept_room: kept_room(&s.mailbox),
            keep_at: if cfg!(target_arch = "wasm32") { None } else { s.path.clone() },
            calendar,
            here: calendar::Here { offset: s.clock.offset, zone: s.zone() },
        },
        place,
    ))
}

/// An import under way: where it puts what it reads, and what it has put in, so that the whole
/// import is one step to undo, a batch that fails is taken out again, and the names of the
/// folders it makes are looked up in an index of its own rather than by reading every folder.
pub struct Running {
    identity: String,
    account: Id,
    place: Place,
    /// The folder the mail goes into: found or made when mail first lands (a file of contacts
    /// alone makes none).
    top: Option<Id>,
    paths: HashMap<Vec<String>, Id>,
    /// The names under each folder of the account (lowercase), with the folder when mail may go
    /// into it (a folder of its own kind, an Outbox or a server's isn't the one).
    names: HashMap<Option<Id>, HashMap<String, Option<Id>>>,
    /// The next number to try for a name taken by a folder mail may not go into.
    next: HashMap<(Id, String), u32>,
    made: Vec<Id>,
    added: Vec<Id>,
    /// The contacts it put in.
    contacts: Vec<Id>,
    /// The events and to-dos it put in, and the calendar it made.
    plans: crate::session::ImportedPlans,
    /// The calendar a new event goes into, once found or made.
    calendar: Option<Id>,
    /// Bytes this import has added to memory ([`MEMORY_BUDGET`]).
    resident: u64,
    /// The attachments held in memory, by content, so that a file carried by many messages is one
    /// copy (#815).
    shared: HashMap<[u8; 32], std::sync::Arc<Vec<u8>>>,
}

impl Running {
    fn start(s: &mut Session, place: &Place, identity: &str) -> Result<Running, CmdError> {
        let account = match place {
            Place::Under(id) => s.mailbox.folder(*id).map(|f| f.account).ok_or_else(|| failed!("no folder with id {id}", id = *id))?,
            Place::New(_) => s.mailbox.local_account(),
        };
        let mb = &s.mailbox;
        let mut names: HashMap<Option<Id>, HashMap<String, Option<Id>>> = HashMap::new();
        for f in mb.folders.iter().filter(|f| f.account == account) {
            let usable = f.kind == mailcraft_model::FolderKind::Custom && !mb.is_send_queue(f.id) && f.remote.is_empty();
            names.entry(f.parent).or_default().insert(f.name.to_lowercase(), usable.then_some(f.id));
        }
        let mut run = Running {
            identity: identity.to_string(),
            account,
            place: place.clone(),
            top: None,
            paths: HashMap::new(),
            names,
            next: HashMap::new(),
            made: Vec::new(),
            added: Vec::new(),
            contacts: Vec::new(),
            plans: Default::default(),
            calendar: None,
            resident: 0,
            shared: HashMap::new(),
        };
        if let Place::Under(id) = place {
            run.top = Some(*id);
        }
        Ok(run)
    }

    /// The folder the mail goes into, found or made.
    fn top(&mut self, s: &mut Session) -> Result<Id, CmdError> {
        if let Some(top) = self.top {
            return Ok(top);
        }
        let Place::New(name) = self.place.clone() else { return Err(fail("the import has no folder to put mail in")) };
        let top = self.own_folder(s, &name)?;
        s.pst_tops.insert(self.identity.clone(), top);
        self.top = Some(top);
        Ok(top)
    }

    /// The folder on this computer named after the file: one of that name (any case) only when it
    /// is this file's (it holds mail of this file, or an earlier run of this import made it and it
    /// is still empty); else a new one, `name (2)` and on when the name is taken.
    fn own_folder(&mut self, s: &mut Session, name: &str) -> Result<Id, CmdError> {
        let mb = &s.mailbox;
        let prefix = format!("{}:", self.identity);
        let holding: HashSet<Id> = mb
            .messages
            .values()
            .filter(|m| m.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case(PST_ITEM_HEADER) && v.starts_with(&prefix)))
            .map(|m| m.folder)
            .collect();
        let made_before = s.pst_tops.get(&self.identity).copied();
        for n in 1..100u32 {
            let candidate = if n == 1 { name.to_string() } else { format!("{name} ({n})") };
            match self.names.get(&None).and_then(|names| names.get(&candidate.to_lowercase())).copied() {
                None => return self.make(s, None, &candidate),
                Some(Some(id)) => {
                    let tree = mb.subtree(id);
                    let ours = tree.iter().any(|f| holding.contains(f))
                        || (made_before == Some(id) && !mb.messages.values().any(|m| tree.contains(&m.folder)));
                    if ours {
                        return Ok(id);
                    }
                }
                Some(None) => {}
            }
        }
        Err(failed!("no free name for the folder {name}", name = name.to_string()))
    }

    fn make(&mut self, s: &mut Session, parent: Option<Id>, name: &str) -> Result<Id, CmdError> {
        if self.made.len() >= MAX_FOLDERS {
            return Err(failed!("more than {max} folders", max = MAX_FOLDERS));
        }
        let id = s.mailbox.add_folder_free(self.account, parent, name).map_err(|e| fail(e.to_string()))?;
        let lower = s.mailbox.folders.last().map_or_else(|| name.to_lowercase(), |f| f.name.to_lowercase());
        self.names.entry(parent).or_default().insert(lower, Some(id));
        self.made.push(id);
        Ok(id)
    }

    /// The folder `name` under `parent` that mail may go into, made when there is none.
    fn child(&mut self, s: &mut Session, parent: Id, name: &str) -> Result<Id, CmdError> {
        let lower = name.to_lowercase();
        match self.names.get(&Some(parent)).and_then(|names| names.get(&lower)).copied() {
            Some(Some(id)) => return Ok(id),
            None => return self.make(s, Some(parent), name),
            Some(None) => {}
        }
        let start = self.next.get(&(parent, lower.clone())).copied().unwrap_or(1);
        for n in start.saturating_add(1)..start.saturating_add(100) {
            self.next.insert((parent, lower.clone()), n);
            let candidate = format!("{name} ({n})");
            match self.names.get(&Some(parent)).and_then(|names| names.get(&candidate.to_lowercase())).copied() {
                Some(Some(id)) => return Ok(id),
                None => return self.make(s, Some(parent), &candidate),
                Some(None) => {}
            }
        }
        Err(failed!("no free name for the folder {name}", name = name.to_string()))
    }

    fn folder(&mut self, s: &mut Session, path: &[String]) -> Result<Id, CmdError> {
        if let Some(id) = self.paths.get(path) {
            return Ok(*id);
        }
        let mut at = self.top(s)?;
        for i in 0..path.len() {
            let prefix = path.get(..=i).unwrap_or_default();
            at = match self.paths.get(prefix) {
                Some(id) => *id,
                None => {
                    let name = prefix.last().map(String::as_str).unwrap_or_default();
                    let id = self.child(s, at, name)?;
                    self.paths.insert(prefix.to_vec(), id);
                    id
                }
            };
        }
        Ok(at)
    }

    /// Count what `m` adds to memory against [`MEMORY_BUDGET`] (#815). Its attachments are in the
    /// content folder already when there is a mailbox file (the reader wrote them: `kept`); what
    /// is held is counted, a file carried by many messages once, and shared among them. Without
    /// a mailbox file the text, HTML and hidden words count too.
    fn account(&mut self, with_file: bool, m: &mut Message) -> Result<(), CmdError> {
        let mut cost: u64 = 0;
        if !with_file {
            let body = m.body.text.len().saturating_add(m.body.html.as_ref().map_or(0, String::len)).saturating_add(m.hidden_text.len());
            cost = cost.saturating_add(body as u64);
        }
        for a in &mut m.attachments {
            if a.kept.is_some() {
                continue;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if a.data.len() > SPILL_MIN {
                let digest = crate::store::Digest::of(&a.data).bytes();
                if let Some(have) = self.shared.get(&digest) {
                    a.data = have.clone();
                    continue;
                }
                self.shared.insert(digest, a.data.clone());
            }
            cost = cost.saturating_add(a.data.len() as u64);
        }
        if self.resident.saturating_add(cost) > MEMORY_BUDGET {
            return Err(failed!(
                "this .pst file is too large to import without a mailbox file with room beside it: save the mailbox to a file first, then import again"
            ));
        }
        self.resident = self.resident.saturating_add(cost);
        Ok(())
    }

    /// Let go of what only a batch taken out again held.
    fn forget_unused(&mut self, resident: u64) {
        self.shared.retain(|_, bytes| std::sync::Arc::strong_count(bytes) > 1);
        self.resident = resident;
    }

    /// Take out what was put in since `made` folders and `added` messages.
    /// The calendar for the file's events: the one an earlier import of it made, else a new one
    /// named after it (`name (2)` when a calendar has that name).
    fn calendar(&mut self, s: &mut Session, name: &str) -> Result<Id, CmdError> {
        if let Some(id) = self.calendar {
            return Ok(id);
        }
        let prefix = format!("{}:", self.identity);
        let earlier = s.mailbox.events.values().find(|e| e.imported.starts_with(&prefix)).map(|e| e.calendar);
        if let Some(id) = earlier.filter(|id| s.mailbox.calendar(*id).is_some_and(|c| c.origin.is_local())) {
            self.calendar = Some(id);
            return Ok(id);
        }
        if s.mailbox.calendars.len() >= 200 {
            return Err(fail("there are too many calendars to make one for this file"));
        }
        let taken = |n: &str| s.mailbox.calendars.iter().any(|c| c.name.eq_ignore_ascii_case(n));
        let mut chosen = name.to_string();
        let mut n = 2u32;
        while taken(&chosen) {
            chosen = format!("{name} ({n})");
            n = n.saturating_add(1);
        }
        let color = crate::cmd::calendar::next_color(&s.mailbox);
        let id = s.mailbox.alloc_id();
        s.mailbox.calendars.push(Calendar { id, name: chosen, color, visible: true, ..Default::default() });
        self.plans.calendar = Some(id);
        self.calendar = Some(id);
        Ok(id)
    }

    fn roll_back(&mut self, s: &mut Session, made: usize, added: usize, contacts: usize, plans: [usize; 4]) {
        for id in self.plans.events.drain(plans[0].min(self.plans.events.len())..) {
            s.mailbox.events.remove(&id);
        }
        for id in self.plans.tasks.drain(plans[1].min(self.plans.tasks.len())..) {
            s.mailbox.tasks.remove(&id);
        }
        for id in self.plans.notes.drain(plans[2].min(self.plans.notes.len())..) {
            s.mailbox.notes.remove(&id);
        }
        for id in self.plans.journal.drain(plans[3].min(self.plans.journal.len())..) {
            s.mailbox.journal.remove(&id);
        }
        if self.plans.events.is_empty()
            && let Some(id) = self.plans.calendar.take()
        {
            s.mailbox.calendars.retain(|c| c.id != id);
            self.calendar = None;
        }
        for id in self.added.drain(added.min(self.added.len())..) {
            s.mailbox.messages.remove(&id);
        }
        for id in self.contacts.drain(contacts.min(self.contacts.len())..) {
            s.mailbox.contacts.remove(&id);
        }
        let gone: Vec<Id> = self.made.drain(made.min(self.made.len())..).collect();
        s.mailbox.remove_folders(&gone);
        let gone: HashSet<Id> = gone.into_iter().collect();
        // (The folder named after the file, made by this batch, is gone with it.)
        s.pst_tops.retain(|_, top| !gone.contains(top));
        if self.top.is_some_and(|t| gone.contains(&t)) {
            self.top = None;
        }
        self.paths.retain(|_, id| !gone.contains(id));
        for names in self.names.values_mut() {
            names.retain(|_, id| !id.is_some_and(|id| gone.contains(&id)));
        }
        self.names.retain(|parent, _| !parent.is_some_and(|p| gone.contains(&p)));
    }
}

/// Whether the folder an import puts mail into is still there, and not in Deleted Items.
fn check_top(s: &Session, top: Id) -> Result<(), CmdError> {
    let mut cur = Some(top);
    let mut steps = 0usize;
    let mut found = false;
    while let Some(f) = cur.and_then(|c| s.mailbox.folder(c)) {
        found = true;
        if f.kind == mailcraft_model::FolderKind::Deleted {
            return Err(fail("the folder the .pst file was being imported into was moved to Deleted Items: the import stopped"));
        }
        cur = f.parent;
        steps = steps.saturating_add(1);
        if steps > MAX_FOLDER_DEPTH {
            break;
        }
    }
    if found { Ok(()) } else { Err(fail("the folder the .pst file was being imported into is gone: the import stopped")) }
}

/// Put a batch of read messages into the mailbox, under the place asked for. Quick: no file is
/// read or written here (the reader's thread has written the attachments to the content folder,
/// and they come in named by their content). A batch that can't all go in is taken out again.
pub fn apply(s: &mut Session, place: &Place, batch: Batch) -> Result<usize, CmdError> {
    let mut run = match s.pst_running.take() {
        Some(r) if r.identity == batch.identity => r,
        Some(r) => {
            s.pst_running = Some(r);
            return Err(fail(PST_BUSY));
        }
        None => Running::start(s, place, &batch.identity)?,
    };
    let (made, added, contacts, resident) = (run.made.len(), run.added.len(), run.contacts.len(), run.resident);
    let plans = [run.plans.events.len(), run.plans.tasks.len(), run.plans.notes.len(), run.plans.journal.len()];
    let r = put(s, &mut run, batch);
    if r.is_err() {
        run.roll_back(s, made, added, contacts, plans);
        run.forget_unused(resident);
    }
    s.pst_running = Some(run);
    r
}

fn put(s: &mut Session, run: &mut Running, batch: Batch) -> Result<usize, CmdError> {
    if let Some(top) = run.top {
        check_top(s, top)?;
    }
    if !batch.folders.is_empty() || !batch.messages.is_empty() {
        let top = run.top(s)?;
        check_top(s, top)?;
    }
    for path in &batch.folders {
        run.folder(s, path)?;
    }
    let now = s.now();
    let mut n = 0usize;
    let with_file = s.path.is_some() && cfg!(not(target_arch = "wasm32"));
    for (path, mut m) in batch.messages {
        let folder = run.folder(s, &path)?;
        run.account(with_file, &mut m)?;
        m.id = 0;
        m.uid = 0;
        m.folder = folder;
        m.draft = false;
        if m.received == 0 {
            m.received = if m.date != 0 { m.date } else { now };
        }
        // The content the message names is asked for again as the message comes in, so that a
        // save taken before this moment doesn't end its protection (`store::protect`).
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(path) = s.path.as_deref() {
            for r in m.attachments.iter().filter_map(|a| a.kept.as_ref()) {
                crate::store::protect_ref(path, r);
            }
        }
        let id = s.mailbox.add_message(m).map_err(|e| fail(e.to_string()))?;
        run.added.push(id);
        n = n.saturating_add(1);
    }
    for mut c in batch.contacts {
        c.id = 0;
        c.origin = Default::default();
        let id = s.mailbox.add_contact(c);
        run.contacts.push(id);
        n = n.saturating_add(1);
    }
    for mut e in batch.events {
        e.id = 0;
        e.origin = Default::default();
        if e.calendar == 0 {
            e.calendar = run.calendar(s, &batch.calendar_name)?;
        }
        // (Gone while the file was read: an event never falls to another, perhaps a server's.)
        if !s.mailbox.calendar(e.calendar).is_some_and(|c| c.origin.is_local()) {
            return Err(fail("the calendar the .pst file was being imported into was removed: the import stopped"));
        }
        let id = s.mailbox.add_event(e).map_err(|e| fail(e.to_string()))?;
        run.plans.events.push(id);
        n = n.saturating_add(1);
    }
    for mut t in batch.tasks {
        t.id = 0;
        t.origin = Default::default();
        let id = s.mailbox.add_task(t);
        run.plans.tasks.push(id);
        n = n.saturating_add(1);
    }
    for mut note in batch.notes {
        note.id = 0;
        let id = s.mailbox.add_note(note);
        run.plans.notes.push(id);
        n = n.saturating_add(1);
    }
    for mut entry in batch.journal {
        entry.id = 0;
        let id = s.mailbox.add_journal(entry);
        run.plans.journal.push(id);
        n = n.saturating_add(1);
    }
    Ok(n)
}

/// After the last batch: the file's categories added where this computer has no category of
/// that name, the report written beside the mailbox, the folder shown, and the report answered.
/// An import that came in a batch at a time (`staged`) is kept here as one step to undo; one
/// run as a single command is one already.
pub fn finish(s: &mut Session, place: &Place, outcome: Outcome, staged: bool) -> Result<Value, CmdError> {
    let Outcome { mut report, categories, identity } = outcome;
    let dry = report.dry_run;
    let mut run = s.pst_running.take().filter(|r| r.identity == identity);
    let categories_before = s.mailbox.categories.clone();
    let top = if dry {
        None
    } else {
        let mut r = match run.take() {
            Some(r) => r,
            None => Running::start(s, place, &identity)?,
        };
        // (Nothing came in: the folder is still found, or made, for the owner to see; a file of
        // contacts alone has no mail, so no folder.)
        let contacts_only = report.contacts_seen.saturating_add(report.plans_seen) > 0 && report.mail_seen == 0 && !report.cancelled;
        let top = if contacts_only && r.top.is_none() { None } else { Some(r.top(s)?) };
        run = Some(r);
        let account = s.mailbox.local_account();
        for (name, color) in categories {
            if s.mailbox.category_of(account, &name).is_none()
                && s.mailbox.categories_of(account).len() < MAX_CATEGORIES
                && s.mailbox.set_category(account, &name, color).is_ok()
            {
                report.categories_added = report.categories_added.saturating_add(1);
            }
        }
        top
    };
    // (A file of contacts alone is shown where its contacts are.)
    let contacts_only = report.contacts_seen.saturating_add(report.plans_seen) > 0 && report.mail_seen == 0 && !report.cancelled;
    if contacts_only {
        report.name = String::new();
    } else if let Some(id) = top {
        report.name = s.mailbox.folder(id).map(|f| f.name.clone()).unwrap_or_default();
        s.view.folder = id;
        s.view.module = crate::Module::Mail;
        s.view.search.clear();
    }
    if !dry && contacts_only {
        s.view.module = if report.contacts_seen > 0 { crate::Module::People } else { crate::Module::Calendar };
    }
    if staged && let Some(r) = run {
        s.keep_import_step_with(IMPORT_LABEL, &r.added, &r.contacts, &r.plans, &r.made, categories_before);
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

/// Why another import waits.
const PST_BUSY: &str = "A .pst file is being imported: try again when it is done";

/// The label of an import's step to undo (the command's own).
pub const IMPORT_LABEL: &str = "Import a .pst File";

/// An import that came in a batch at a time and ended without its last word (its reader
/// stopped, or the window went): what came in is kept as one step to undo, and Undo is free
/// again.
pub fn end_staged(s: &mut Session) {
    if let Some(r) = s.pst_running.take() {
        let categories = s.mailbox.categories.clone();
        s.keep_import_step_with(IMPORT_LABEL, &r.added, &r.contacts, &r.plans, &r.made, categories);
    }
}

/// The report as a text file beside the mailbox file, when there is one: its path. The name is
/// the file's (cut short), "import report" and the time; one that is there already, or a link,
/// is never written over: the next free `(2)`, `(3)` is taken.
#[cfg(not(target_arch = "wasm32"))]
fn write_report(s: &Session, report: &Report) -> Option<String> {
    use std::io::Write;
    let dir = s.path.as_ref()?.parent()?.to_path_buf();
    let when = mailcraft_time::format_iso_utc(s.now());
    let stem: String = folder_name(&report.file).chars().take(REPORT_NAME_CHARS).collect();
    let text = report.to_text(&when);
    for n in 1..=100u32 {
        let more = if n == 1 { String::new() } else { format!(" ({n})") };
        let name = crate::opening::export_name_or(&format!("{stem} import report {}{more}", when.replace(':', "-")), "txt", "pst import report");
        let path = dir.join(name);
        // (`create_new` makes the file or fails: it never follows a link that is there.)
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                f.write_all(text.as_bytes()).ok()?;
                return Some(path.display().to_string());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
    }
    None
}

/// Most characters of the file's name in its report's name, so that "import report" and the
/// time always fit.
#[cfg(not(target_arch = "wasm32"))]
const REPORT_NAME_CHARS: usize = 36;

#[cfg(target_arch = "wasm32")]
fn write_report(_: &Session, _: &Report) -> Option<String> {
    None
}

/// `file.importPst`: see the command's params.
pub fn run(s: &mut Session, v: &Value) -> Result<Value, CmdError> {
    if p::bool(v, "staged").unwrap_or(false) {
        // Read by the window, off its thread, and handed over a batch at a time.
        let name = p::str(v, "name").map(tidy_name).unwrap_or_else(|| "Imported .pst".to_string());
        let staged = s.staged_pst.take().ok_or_else(|| fail("nothing is waiting to be imported"))?;
        let place = place_of(s, v, &name);
        return match staged {
            Staged::Batch(batch) => Ok(json!({"staged": apply(s, &place?, batch)?})),
            Staged::Done(outcome) => {
                // The last word: whatever becomes of it, what came in is one step to undo.
                let done = place.and_then(|place| finish(s, &place, *outcome, true));
                if done.is_err() {
                    end_staged(s);
                }
                done
            }
        };
    }
    // Not while the window's import runs: that one is still putting its mail in.
    if s.pst_running.is_some() {
        return Err(fail(PST_BUSY));
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (base64_decode, Limits::default, job);
        Err(fail("importing a .pst file isn't supported in the browser yet"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // One command, one step to undo: the session's own.
        s.pst_running = None;
        let r = run_whole(s, v);
        s.pst_running = None;
        r
    }
}

/// Import the .pst file at `path` the way the window does (#815): the file is read on a thread of
/// its own and its mail comes in a batch at a time, through `file.importPst` with `staged`, so
/// that what is read is put into the mailbox (and its attachments out to the content folder)
/// before more is read, and the read waits for the mailbox rather than holding the file. `v`
/// carries the `places` and the `name` as the command's params do; the answer is the report. An
/// import that fails part-way keeps what came in as one step to undo, as the window's does.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_staged(s: &mut Session, path: &str, password: Option<String>, dry_run: bool, v: &Value) -> Result<Value, CmdError> {
    if s.pst_running.is_some() {
        return Err(fail(PST_BUSY));
    }
    let (job, _) = job(s, path, password, dry_run, v)?;
    let mut params = json!({"staged": true, "name": folder_name(path)});
    if let (Some(p), Some(places)) = (params.as_object_mut(), v.get("places")) {
        p.insert("places".to_string(), places.clone());
    }
    let progress = Progress::default();
    // A small channel: the reader waits for the mailbox.
    let (tx, rx) = std::sync::mpsc::sync_channel(2);
    std::thread::scope(|scope| {
        let (job, progress) = (&job, &progress);
        let reader = std::thread::Builder::new()
            .name("pst import".into())
            .spawn_scoped(scope, move || read_path(path, job, progress, &mut |event| tx.send(event).is_ok()));
        if let Err(e) = reader {
            return Err(failed!("the .pst file couldn't be read: {why}", why = e.to_string()));
        }
        // (Dropping the receiver ends the read: its next `send` fails.)
        let rx = rx;
        loop {
            let answer = match rx.recv() {
                Ok(Event::Batch(batch)) => {
                    s.staged_pst = Some(Staged::Batch(batch));
                    let r = s.run("file.importPst", &params);
                    s.staged_pst = None;
                    if r.is_err() {
                        progress.cancel();
                        end_staged(s);
                    }
                    r.map(|_| None)
                }
                Ok(Event::Done(Ok(outcome))) => {
                    s.staged_pst = Some(Staged::Done(outcome));
                    let r = s.run("file.importPst", &params);
                    s.staged_pst = None;
                    r.map(Some)
                }
                Ok(Event::Done(Err(e))) => {
                    end_staged(s);
                    Err(e)
                }
                Err(_) => {
                    end_staged(s);
                    Err(fail("the .pst file's reader stopped before it was done"))
                }
            };
            match answer {
                Ok(None) => {}
                Ok(Some(report)) => return Ok(report),
                Err(e) => return Err(e),
            }
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn run_whole(s: &mut Session, v: &Value) -> Result<Value, CmdError> {
    let dry_run = p::bool(v, "dryRun").unwrap_or(false);
    let password = p::str(v, "password").map(str::to_string);
    let file = p::str(v, "path").or_else(|| p::str(v, "name")).unwrap_or_default().to_string();
    let (job, place) = job(s, &file, password, dry_run, v)?;
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
        read(open_path(path)?, &job, Limits::default(), &progress, &mut sink)?
    } else {
        return Err(crate::cmd::bad("name the .pst file (`path`), or hand it over as `name` and base64 `data`"));
    };
    finish(s, &place, outcome, false)
}

/// How many bytes the import under way has added to memory (for the tests).
#[cfg(test)]
pub(crate) fn resident_of(s: &Session) -> Option<u64> {
    s.pst_running.as_ref().map(|r| r.resident)
}
