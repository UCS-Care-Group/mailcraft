//! The mailbox on disk, kept so that saving it costs what changed rather than what it holds.
//!
//! One self-contained file ([`crate::io::write_mailbox`]) carries every attachment as base64 and is
//! written whole each time: right for an export, hopeless for years of mail that change with every
//! message read. The mailbox a program works on is kept in two parts instead:
//!
//! ```text
//! mailbox.mailcraft           the mailbox without its content (format 3, a log of records, see
//!                             store_index.rs): accounts, folders, every message's headers and
//!                             flags, contacts, events, tasks, and for each message the content it
//!                             keeps outside: [SHA-256, length]
//! mailbox.mailcraft.blobs/    the content, one file per distinct body or attachment
//!     3f/3f2a…                named by the SHA-256 of its bytes (64 lowercase hex digits), in a
//!                             folder named by the first two
//! ```
//!
//! - **What moves out:** a body or an attachment of more than [`INLINE_MAX`] bytes. A content file
//!   is written once and never changed. The same file attached to ten messages is kept once (and
//!   shared in memory).
//! - **Saving** ([`save`]) writes the content files that aren't there yet, then appends to the
//!   mailbox file a record for each message, contact, event or task that changed or went (and one
//!   for the rest of the mailbox when that changed), closed by a commit, in one write flushed to
//!   the disk: a save after a message was read appends a few hundred bytes, however large the
//!   mailbox. A crash in the middle leaves records no commit closes, which are not read. When
//!   what was appended outgrows the mailbox itself, the whole file is written again (compacted)
//!   to a temporary file beside it, flushed and renamed over. [`Saver`] does all this on a thread
//!   of its own for a window that must not wait.
//! - **Opening** ([`open`]) reads one self-contained file (format 1), the JSON mailbox file of the
//!   form before this one (format 2, with the same content folder), or this form. Either of the
//!   first two becomes this form the first time a changed mailbox is saved, written whole and
//!   renamed over (until then the old file is the mailbox), and the old file is kept beside it as
//!   `mailbox.mailcraft.format1` or `.format2`. Everything read back is hostile: a content file is
//!   taken only when it is a plain file of the length and SHA-256 the mailbox file names; names
//!   are 64 hex digits or nothing; sizes are capped; a record counts only whole, with its
//!   checksum, and closed by a commit. Bytes after the last commit (a save cut short, or damage)
//!   are reported, and the file is kept aside as `mailbox.mailcraft.damaged-<time>` before it is
//!   next written.
//! - **What comes into memory** ([`fill`]): every message's headers, and the bodies of the newest
//!   mail (by date) up to a budget, the smaller of [`MAX_CONTENT`] and a quarter of the machine's
//!   memory ([`body_budget`]). Attachments, and the bodies of older mail, stay on disk and are
//!   named on the message ([`mailcraft_model::Attachment::kept`], [`Message::kept`]); they are
//!   read when something needs them ([`fetch`], behind `Session::content`): a message shown,
//!   searched, answered, printed, exported or sent, a file opened or saved. What is fetched is
//!   kept for a while in a cache bounded by bytes ([`CACHE_MAX`]). Each part not loaded is looked
//!   for on disk when the mailbox opens (a stat, within a time cap), so that what is missing is
//!   told apart from what is merely not loaded: the first is reported, the second only noted.
//!   Content that is missing or damaged never stops the mailbox from opening: the message keeps
//!   everything else and says its text is unavailable, and what the mailbox file said about it
//!   is written back unchanged, so content that turns up again (a folder copied late) is found
//!   the next time.
//! - **Sweeping** ([`sweep`]) removes content files the mailbox file no longer names, never one
//!   written in the last few minutes: a save in flight has written its content before its mailbox
//!   file names it. Nothing is swept while a damaged mailbox file is kept aside: it may name
//!   content the mailbox no longer does.
//!
//! Not in the browser, which has no folder to keep content in: there the mailbox stays one file.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};

use mailcraft_model::{Contact, ContentRef, Event, Id, KeptBody, Mailbox, Message, Task};
use serde::{Deserialize, Serialize};

use crate::io::{BackgroundSave, ContentSource, MAX_FILE, SaveReport};

#[path = "store_index.rs"]
mod index;

use index::{Coll, Fp, Key, Tail};

/// Largest mailbox file read in this form (16 GB; the JSON forms stop at [`MAX_FILE`]).
pub const MAX_INDEX: u64 = 16 << 30;

/// Items of a mailbox file parsed at a time when it is opened.
const OPEN_SLICE: usize = 16_384;

/// What a mailbox file may carry beyond twice the mailbox it holds before it is compacted (less
/// in tests, so that small mailboxes get there).
const COMPACT_SLACK: u64 = if cfg!(test) { 256 << 10 } else { 16 << 20 };

/// A body or an attachment up to this many bytes stays in the mailbox file.
pub const INLINE_MAX: usize = 1024;

/// Largest content file read.
pub const MAX_BLOB: u64 = MAX_FILE;

/// Most content one mailbox brings into memory when it opens (8 GB), whatever the machine has;
/// the bodies beyond it stay on disk until they are wanted ([`body_budget`]).
pub const MAX_CONTENT: u64 = 8 << 30;

/// What the bodies get when the machine's memory can't be read (2 GB).
pub const FALLBACK_BUDGET: u64 = 2 << 30;

/// Most bytes of fetched content kept in memory for the next time they are wanted (512 MB).
pub const CACHE_MAX: u64 = 512 << 20;

/// A fetched piece larger than this is handed over but not kept.
const CACHE_ITEM_MAX: u64 = 64 << 20;

/// How long opening a mailbox spends looking for the content it doesn't load, so as to tell what
/// is missing from what is merely on disk; past it, the rest is taken to be on disk.
const STAT_TIME: Duration = Duration::from_secs(20);

/// A content file nothing names is left alone for this long after it was written.
pub const GRACE: Duration = Duration::from_secs(10 * 60);

/// The format number of the JSON mailbox file with its content beside it, the form before this
/// one (one self-contained file is format 1). Read, never written.
pub const SPLIT_VERSION: u32 = 2;

/// The format number of this form: a log of records ([`index`]).
pub const INDEX_VERSION: u32 = index::VERSION;

/// What a message says instead of text that couldn't be read back.
pub const UNAVAILABLE: &str =
    "[This message's text is unavailable: the file that keeps it is missing from the mailbox's content folder, or damaged.]";

#[cfg(test)]
#[test]
fn the_placeholder_is_the_one_the_session_writes() {
    assert_eq!(UNAVAILABLE, crate::store_placeholder());
}

/// Attachments of one message kept outside the mailbox file; any after these stay inside it.
const MAX_PARTS: usize = 4096;

/// Directory entries one sweep looks at.
const MAX_ENTRIES: usize = 4_000_000;

/// Threads reading or writing content files at once.
const MAX_THREADS: usize = 8;

/// How often a running program looks for content nothing names any more.
const SWEEP_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// How long [`Saver::flush`] waits for the disk before giving up.
const FLUSH_WAIT: Duration = Duration::from_secs(120);

/// Where the content of the mailbox file at `path` is kept: `<file>.blobs` beside it.
pub fn blob_dir(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".blobs");
    path.with_file_name(name)
}

/// The file a mailbox path stands for: itself, or where it leads when it is a link. A mailbox
/// kept somewhere else through a link stays there, with its content beside it, instead of the
/// link being replaced by a file the first time it is saved.
fn resolve(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

// ---- names ----

/// The SHA-256 of a piece of content: its name on disk.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Digest([u8; 32]);

impl Digest {
    pub fn of(bytes: &[u8]) -> Digest {
        let mut out = [0u8; 32];
        for (o, b) in out.iter_mut().zip(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref()) {
            *o = *b;
        }
        Digest(out)
    }

    /// 64 lowercase hex digits.
    pub fn hex(&self) -> String {
        let digit = |n: u8| char::from_digit(u32::from(n), 16).unwrap_or('0');
        self.0.iter().flat_map(|b| [digit(b >> 4), digit(b & 15)]).collect()
    }

    /// From exactly 64 lowercase hex digits. Nothing else names a content file, so nothing read
    /// from a mailbox file can become any other path.
    pub fn parse(text: &str) -> Option<Digest> {
        let digits = text.as_bytes();
        if digits.len() != 64 || !is_hex(text) {
            return None;
        }
        let mut out = [0u8; 32];
        for (o, pair) in out.iter_mut().zip(digits.chunks(2)) {
            let value = |i: usize| pair.get(i).and_then(|c| char::from(*c).to_digit(16));
            *o = (value(0)? * 16 + value(1)?) as u8;
        }
        Some(Digest(out))
    }
}

impl std::fmt::Debug for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// A content file as the mailbox file names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Ref {
    hash: Digest,
    len: u64,
}

impl Ref {
    fn of(bytes: &[u8]) -> Ref {
        Ref { hash: Digest::of(bytes), len: bytes.len() as u64 }
    }
}

impl Ref {
    fn from_model(r: &ContentRef) -> Ref {
        Ref { hash: Digest(r.sha256), len: r.len }
    }

    fn to_model(self) -> ContentRef {
        ContentRef { sha256: self.hash.0, len: self.len }
    }
}

/// One part of a message kept outside the mailbox file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Part {
    r: Ref,
    /// Its bytes are in memory. Not so for content left on disk when the mailbox was opened, or
    /// that couldn't be read: that keeps the name it had, and nothing can be written for it.
    held: bool,
}

/// The parts of one message kept outside the mailbox file; attachments by their index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Parts {
    text: Option<Part>,
    html: Option<Part>,
    attachments: Vec<Option<Part>>,
}

impl Parts {
    fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.attachments.iter().all(Option::is_none)
    }

    fn all(&self) -> impl Iterator<Item = &Part> {
        self.text.iter().chain(&self.html).chain(self.attachments.iter().flatten())
    }

    /// As the mailbox file has it under `blobs`: `{"text":[hash,length],"attachments":[…,null]}`.
    fn json(&self) -> String {
        let one = |p: &Option<Part>| p.map_or_else(|| "null".to_string(), |p| format!("[\"{}\",{}]", p.r.hash.hex(), p.r.len));
        let mut fields = Vec::new();
        if self.text.is_some() {
            fields.push(format!("\"text\":{}", one(&self.text)));
        }
        if self.html.is_some() {
            fields.push(format!("\"html\":{}", one(&self.html)));
        }
        if !self.attachments.is_empty() {
            fields.push(format!("\"attachments\":[{}]", self.attachments.iter().map(one).collect::<Vec<_>>().join(",")));
        }
        format!("{{{}}}", fields.join(","))
    }

    /// The bytes of the parts that are in memory, out of the message they were worked out from.
    fn content<'a>(&self, m: &'a Message) -> Vec<(Ref, &'a [u8])> {
        let held = |p: &Option<Part>| p.filter(|p| p.held).map(|p| p.r);
        let mut out = Vec::new();
        if let Some(r) = held(&self.text) {
            out.push((r, m.body.text.as_bytes()));
        }
        if let (Some(r), Some(html)) = (held(&self.html), &m.body.html) {
            out.push((r, html.as_bytes()));
        }
        for (p, a) in self.attachments.iter().zip(&m.attachments) {
            if let Some(r) = held(p) {
                out.push((r, a.data.as_slice()));
            }
        }
        out
    }
}

/// `blobs` as a mailbox file has it. Strictly typed: a file that says anything else there isn't
/// one of ours, and guessing would end in content being written back empty.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Head {
    version: u32,
    blobs: BTreeMap<Id, WireParts>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct WireParts {
    text: Option<(String, u64)>,
    html: Option<(String, u64)>,
    #[serde(deserialize_with = "capped")]
    attachments: Vec<Option<(String, u64)>>,
}

/// At most [`MAX_PARTS`] entries of a list; the rest is skipped without being kept.
fn capped<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Option<(String, u64)>>, D::Error> {
    struct Capped;
    impl<'de> serde::de::Visitor<'de> for Capped {
        type Value = Vec<Option<(String, u64)>>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("a list of content names")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            while out.len() < MAX_PARTS {
                match seq.next_element()? {
                    Some(item) => out.push(item),
                    None => return Ok(out),
                }
            }
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
            Ok(out)
        }
    }
    d.deserialize_seq(Capped)
}

fn parse_ref(wire: &(String, u64)) -> Option<Ref> {
    Digest::parse(&wire.0).map(|hash| Ref { hash, len: wire.1 })
}

impl WireParts {
    /// Every name given, parsed or not.
    fn hashes(&self) -> impl Iterator<Item = Digest> + '_ {
        self.text.iter().chain(&self.html).chain(self.attachments.iter().flatten()).filter_map(parse_ref).map(|r| r.hash)
    }

    /// The parts as named, all taken to be in memory (what couldn't be read is marked after).
    fn parts(&self) -> Parts {
        let one = |p: &Option<(String, u64)>| p.as_ref().and_then(parse_ref).map(|r| Part { r, held: true });
        Parts { text: one(&self.text), html: one(&self.html), attachments: self.attachments.iter().map(one).collect() }
    }
}

// ---- what is known about a mailbox file ----

/// A mailbox file as this program last read or wrote it, to notice anybody else writing it.
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl Stamp {
    /// The file as it is now, if it is still `len` bytes long (what was read or written of it).
    fn of(path: &Path, len: u64) -> Option<Stamp> {
        let meta = fs::metadata(path).ok()?;
        (meta.len() == len).then(|| Stamp { len, modified: meta.modified().ok() })
    }

    /// The file is still the one this was taken of.
    fn still(&self, path: &Path) -> bool {
        fs::metadata(path).is_ok_and(|m| m.len() == self.len && m.modified().ok() == self.modified)
    }
}

/// An item as the mailbox file has it, as last read or written here: so that an item nobody
/// touched since costs a lookup, not its JSON.
struct Entry<T> {
    /// The item in memory that is what the record says. While this is alive the allocation can't
    /// be reused and the item in it can't change in place (changing a shared item makes a new
    /// one, and so does changing one only this points at besides), so the same address is the
    /// same item. Dangling when that isn't known (a file the opening had to repair).
    of: Weak<T>,
    /// Its record's checksum: what the record says, in 16 bytes.
    fp: Fp,
    /// The bytes its record takes.
    size: u64,
}

impl<T> Entry<T> {
    fn is(&self, item: &Arc<T>) -> bool {
        std::ptr::eq(self.of.as_ptr(), Arc::as_ptr(item))
    }
}

/// A message as the mailbox file has it: as [`Entry`], and the content it names.
struct Msg {
    entry: Entry<Message>,
    parts: Parts,
}

/// What the mailbox file in this form says, as last read or written here.
struct Disk {
    messages: BTreeMap<Id, Msg>,
    contacts: BTreeMap<Id, Entry<Contact>>,
    events: BTreeMap<Id, Entry<Event>>,
    tasks: BTreeMap<Id, Entry<Task>>,
    rest: (Fp, u64),
    /// The bytes of the records that say what the mailbox is now (the rest and every item).
    live: u64,
    /// Where the last commit ends.
    len: u64,
    tail: Tail,
}

impl Disk {
    fn new(rest: (Fp, u64)) -> Disk {
        Disk {
            messages: BTreeMap::new(),
            contacts: BTreeMap::new(),
            events: BTreeMap::new(),
            tasks: BTreeMap::new(),
            rest,
            live: rest.1,
            len: index::HEADER_LEN,
            tail: Tail::Clean,
        }
    }

    /// A save can be appended: the file ends where its last save did.
    fn appendable(&self) -> bool {
        self.tail == Tail::Clean
    }

    /// Whether the file has outgrown what it says, so that the next save writes it whole.
    fn outgrown(&self) -> bool {
        self.len > self.live.saturating_mul(2).saturating_add(COMPACT_SLACK)
    }
}

/// Count `parts` among the content the file names.
fn hold(live: &mut HashMap<Digest, u32>, parts: &Parts) {
    for p in parts.all() {
        let n = live.entry(p.r.hash).or_default();
        *n = n.saturating_add(1);
    }
}

/// No longer count `parts` among the content the file names.
fn release(live: &mut HashMap<Digest, u32>, parts: &Parts) {
    for p in parts.all() {
        if let Some(n) = live.get_mut(&p.r.hash) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                live.remove(&p.r.hash);
            }
        }
    }
}

/// A mailbox as it was opened, to know it again without keeping it: its messages by where they
/// are in memory (a message that changes moves, see [`Entry`]), the rest by its SHA-256.
struct Loaded {
    messages: Vec<(Id, Weak<Message>)>,
    rest: Digest,
}

impl Loaded {
    /// Everything but the messages, as the model writes it.
    fn rest(mailbox: &Mailbox) -> Option<Digest> {
        let mut shell = mailbox.clone();
        shell.messages = BTreeMap::new();
        serde_json::to_vec(&shell).ok().map(|bytes| Digest::of(&bytes))
    }

    fn of(mailbox: &Mailbox) -> Option<Loaded> {
        let messages = mailbox.messages.iter().map(|(id, m)| (*id, Arc::downgrade(m))).collect();
        Some(Loaded { messages, rest: Loaded::rest(mailbox)? })
    }

    fn is(&self, mailbox: &Mailbox) -> bool {
        self.messages.len() == mailbox.messages.len()
            && self.messages.iter().zip(&mailbox.messages).all(|((id, was), (now, m))| id == now && std::ptr::eq(was.as_ptr(), Arc::as_ptr(m)))
            && Loaded::rest(mailbox) == Some(self.rest)
    }
}

#[derive(Default)]
struct State {
    /// The content the mailbox file on disk names, as last read or written here, and by how
    /// many parts. A sweep, by this program or another, leaves these alone; anything else is
    /// written (or its time renewed) before a mailbox file names it.
    live: HashMap<Digest, u32>,
    stamp: Option<Stamp>,
    /// The mailbox as it was opened, until something else is saved: a mailbox nobody changed is
    /// not rewritten, whatever form its file has.
    loaded: Option<Loaded>,
    /// The file is in this form, and this is what it says (with `stamp`).
    disk: Option<Disk>,
    /// The file was opened in an earlier form (1 or 2): it is kept beside the new one when that
    /// takes its place.
    legacy: Option<u32>,
    /// The SHA-256 of attachments already worked out, by where their bytes are.
    hashes: HashMap<usize, (Weak<Vec<u8>>, Digest)>,
    /// The newest state written (see [`next_seq`]): an older one arriving late is dropped.
    newest: u64,
    swept: Option<Instant>,
}

#[derive(Default)]
struct Shared {
    /// A host keeps this file up to date by itself ([`BackgroundSave::follow`]).
    followed: AtomicBool,
    state: Mutex<State>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What this program knows about the mailbox file at `path`. One per file, so that everybody
/// saving it (a command, the background saver) takes turns.
fn shared(path: &Path) -> Arc<Shared> {
    static ALL: OnceLock<Mutex<HashMap<PathBuf, Arc<Shared>>>> = OnceLock::new();
    let key = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    lock(ALL.get_or_init(Default::default)).entry(key).or_default().clone()
}

/// States of a mailbox are numbered as they are handed over to be written, so that of two saves
/// racing for the same file the later state wins whichever gets to the disk first.
fn next_seq() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    SEQ.fetch_add(1, Ordering::Relaxed) + 1
}

/// Whether a host keeps the mailbox file at `path` up to date by itself.
pub fn followed(path: &Path) -> bool {
    shared(path).followed.load(Ordering::Relaxed)
}

// ---- files ----

/// A name for a temporary file beside `path`.
fn temp_beside(path: &Path) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{}-{nanos:x}-{}.tmp", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    path.with_file_name(name)
}

/// Whether `name` is a temporary file [`temp_beside`] made for `stem`.
fn is_temp_of(name: &str, stem: &str) -> bool {
    name.strip_prefix(stem).and_then(|rest| rest.strip_prefix('.')).and_then(|rest| rest.strip_suffix(".tmp")).is_some_and(|middle| {
        let mut parts = middle.split('-');
        let ok = |p: Option<&str>, hex: bool| p.is_some_and(|p| if hex { is_hex(p) } else { !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) });
        ok(parts.next(), false) && ok(parts.next(), true) && ok(parts.next(), false) && parts.next().is_none()
    })
}

/// Rename `from` over `to`. On Windows a file somebody has open (a virus scanner, a backup) can't
/// be replaced that instant: try again for a moment before giving up.
fn rename_over(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match fs::rename(from, to) {
            Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied && tries < 20 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
            done => return done,
        }
    }
}

/// Write `bytes` to `path` so that a crash leaves what was there or all of the new file: into a
/// temporary file beside it, flushed to the disk, then renamed over.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = AtomicFile::create(path)?;
    file.write_all(bytes)?;
    file.commit()
}

/// A new file for its owner only, created where nothing is (never through something already
/// there by that name).
fn create_private(path: &Path) -> std::io::Result<fs::File> {
    let mut open = fs::OpenOptions::new();
    open.write(true).create_new(true);
    // Mail is private: nobody else on the machine reads it.
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut open, 0o600);
    open.open(path)
}

/// Flush the folder `path` is in, so that a rename in it reaches the disk (where folders can be
/// flushed).
fn sync_dir_of(path: &Path) {
    #[cfg(unix)]
    {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        if let Ok(dir) = fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// A file written to a temporary file beside it and renamed over it when it is whole
/// ([`AtomicFile::commit`]); dropped before that, the temporary file goes and nothing changed.
struct AtomicFile {
    tmp: PathBuf,
    path: PathBuf,
    out: Option<std::io::BufWriter<fs::File>>,
}

impl AtomicFile {
    fn create(path: &Path) -> std::io::Result<AtomicFile> {
        let mut tries = 0;
        loop {
            let tmp = temp_beside(path);
            match create_private(&tmp) {
                Ok(file) => {
                    let out = Some(std::io::BufWriter::with_capacity(64 << 10, file));
                    return Ok(AtomicFile { tmp, path: path.to_path_buf(), out });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 8 => tries += 1,
                Err(e) => return Err(e),
            }
        }
    }

    /// Flushed to the disk, then renamed over the file it stands for.
    fn commit(mut self) -> std::io::Result<()> {
        let out = self.out.take().ok_or_else(|| std::io::Error::other("written twice"))?;
        let file = out.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        rename_over(&self.tmp, &self.path)?;
        sync_dir_of(&self.path);
        Ok(())
    }
}

impl std::io::Write for AtomicFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.as_mut().ok_or_else(|| std::io::Error::other("already written"))?.write(buf)
    }

    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        self.out.as_mut().ok_or_else(|| std::io::Error::other("already written"))?.write_all(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.as_mut().map_or(Ok(()), |o| o.flush())
    }
}

impl Drop for AtomicFile {
    fn drop(&mut self) {
        // Not renamed over: what was written goes (whether or not it was renamed, it is no
        // longer under this name).
        drop(self.out.take());
        let _ = fs::remove_file(&self.tmp);
    }
}

/// Make the folder `dir` (its parent is there), for its owner only.
fn make_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Make sure `dir` is a folder of its own. Anything else by that name (a link to somewhere else, a
/// file) is removed first, without being followed: content goes into this folder or nowhere.
fn own_dir(dir: &Path) -> std::io::Result<()> {
    let is_dir = |dir: &Path| fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_dir());
    // Looked at once: between two looks another thread saving into the same folder can make it,
    // and taking that for "something else by that name" would remove the folder under its feet.
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_dir() => return Ok(()),
        Ok(_) => {
            let _ = fs::remove_file(dir).or_else(|_| fs::remove_dir(dir));
        }
        Err(_) => {}
    }
    match make_dir(dir) {
        // Another thread writing into the same folder got there first.
        Err(_) if is_dir(dir) => Ok(()),
        made => made,
    }
}

/// Where the content named `hash` lives under `dir`.
fn blob_path(dir: &Path, hash: &Digest) -> Option<(PathBuf, PathBuf)> {
    let name = hash.hex();
    let sub = dir.join(name.get(..2)?);
    let file = sub.join(&name);
    Some((sub, file))
}

/// Make sure the content `r` is on disk under `dir`, and safe from a sweep for a while. Returns
/// whether it had to be written. A file of the right length that is already there is this
/// content (nothing else is ever written under its name): only its time is renewed.
fn put(dir: &Path, r: &Ref, bytes: &[u8]) -> std::io::Result<bool> {
    let (sub, file) = blob_path(dir, &r.hash).ok_or_else(|| std::io::Error::other("no name"))?;
    let there = |file: &Path| fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_file() && m.len() == r.len);
    if there(&file) {
        let renewed = fs::OpenOptions::new().write(true).open(&file).and_then(|f| f.set_modified(SystemTime::now()));
        if renewed.is_ok() {
            return Ok(false);
        }
    }
    // Twice at most: a folder that went missing between making it and writing into it (a sweep, a
    // tidy-up tool) is made again.
    let mut last = None;
    for _ in 0..2 {
        own_dir(&sub)?;
        match write_atomic(&file, bytes) {
            Ok(()) => return Ok(true),
            // Somebody else saving the same content at the same moment is no failure.
            Err(_) if there(&file) => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("the content folder keeps going away")))
}

/// Which of the 256 folders of two digits are folders of their own (not links to somewhere
/// else), each looked at once however many files are read from it.
struct Folders([OnceLock<bool>; 256]);

impl Folders {
    fn new() -> Folders {
        Folders(std::array::from_fn(|_| OnceLock::new()))
    }

    fn real(&self, hash: &Digest, sub: &Path) -> bool {
        let first = hash.0.first().copied().unwrap_or(0);
        self.0.get(usize::from(first)).is_some_and(|seen| *seen.get_or_init(|| fs::symlink_metadata(sub).is_ok_and(|m| m.file_type().is_dir())))
    }
}

/// Open `file` for reading if it is a plain file; a link is never followed.
fn open_plain(file: &Path) -> Option<(fs::File, u64)> {
    #[cfg(windows)]
    let opened = {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_FLAG_OPEN_REPARSE_POINT: a link opens as itself, and then isn't a plain file.
        fs::OpenOptions::new().read(true).custom_flags(0x0020_0000).open(file).ok()?
    };
    #[cfg(not(windows))]
    let opened = fs::symlink_metadata(file).ok().filter(|m| m.file_type().is_file()).and_then(|_| fs::File::open(file).ok())?;
    let meta = opened.metadata().ok()?;
    meta.file_type().is_file().then_some((opened, meta.len()))
}

/// The content `r` from under `dir`, if it is there and is what its name says: a plain file (not
/// a link, and in a folder that isn't one either) of that length with that SHA-256.
fn get(dir: &Path, r: &Ref, folders: &Folders) -> Option<Vec<u8>> {
    let (sub, file) = blob_path(dir, &r.hash)?;
    if r.len > MAX_BLOB || !folders.real(&r.hash, &sub) {
        return None;
    }
    let (file, on_disk) = open_plain(&file)?;
    if on_disk != r.len {
        return None;
    }
    let len = usize::try_from(r.len).ok()?;
    let mut bytes = Vec::new();
    // Memory that isn't there is content that is unavailable, not a crash.
    bytes.try_reserve_exact(len.checked_add(1)?).ok()?;
    // Never more than was promised, however the file grows while it is read.
    file.take(r.len.checked_add(1)?).read_to_end(&mut bytes).ok()?;
    (bytes.len() == len && Digest::of(&bytes) == r.hash).then_some(bytes)
}

/// Do `work` for each of `0..n`, on a few threads when there is a lot of it: content files are
/// many and mostly small, and one after another the disk spends its time waiting. Results come
/// back by index; `None` where a thread gave up. A thread that can't start costs only speed.
fn fan_out<T: Send>(n: usize, work: impl Fn(usize) -> T + Sync) -> Vec<Option<T>> {
    let threads = std::thread::available_parallelism().map_or(1, |p| p.get()).min(MAX_THREADS).min(n / 8).max(1);
    let next = AtomicUsize::new(0);
    let run = || {
        let mut done = Vec::new();
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= n {
                return done;
            }
            done.push((i, work(i)));
        }
    };
    let mut out: Vec<Option<T>> = (0..n).map(|_| None).collect();
    std::thread::scope(|scope| {
        let others: Vec<_> =
            (1..threads).filter_map(|_| std::thread::Builder::new().name("mailbox content".into()).spawn_scoped(scope, run).ok()).collect();
        let mine = run();
        for (i, value) in others.into_iter().filter_map(|t| t.join().ok()).flatten().chain(mine) {
            if let Some(slot) = out.get_mut(i) {
                *slot = Some(value);
            }
        }
    });
    out
}

// ---- opening ----

/// A mailbox read from its file.
pub struct Opened {
    pub mailbox: Mailbox,
    /// The file keeps its content in a folder beside it (`false`: one self-contained file).
    pub split: bool,
    /// Bodies and attachments whose content file was found to be missing or damaged.
    pub unavailable: usize,
    /// Bodies and attachments brought into memory.
    pub loaded: usize,
    /// Bodies and attachments left on disk, to be loaded when something needs them
    /// (`Session::content`): every attachment, and the bodies of mail beyond the budget.
    pub deferred: usize,
    /// Bytes at the end of the file that aren't a finished save (a save a crash cut short, or
    /// damage). They are not read; the file is kept aside before it is next written.
    pub unfinished: u64,
}

impl std::fmt::Debug for Opened {
    /// Without the mail itself.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Opened {{ messages: {}, split: {}, unavailable: {}, loaded: {}, deferred: {}, unfinished: {} }}",
            self.mailbox.messages.len(),
            self.split,
            self.unavailable,
            self.loaded,
            self.deferred,
            self.unfinished
        )
    }
}

/// Say what opening the file at `show` left out, where the owner reads warnings only for what is
/// wrong: content left on disk is the normal way, noted for anybody debugging.
fn report(show: &std::path::Display<'_>, dir: &Path, filled: &Filled) {
    if filled.missing > 0 {
        log::warn!("{show}: {} bodies or attachments are missing from {} or damaged", filled.missing, dir.display());
    }
    if filled.deferred > 0 {
        log::debug!("{show}: {} attachments and bodies are kept on disk and load when opened ({} loaded)", filled.deferred, filled.loaded);
    }
}

/// Whether the file at `path` begins as a mailbox file in this form does.
fn is_index(file: &Path) -> Result<bool, String> {
    let mut head = [0u8; 16];
    let mut f = fs::File::open(file).map_err(|e| e.to_string())?;
    let got = index::fill(&mut f, &mut head).map_err(|e| e.to_string())?;
    Ok(got == head.len() && &head == crate::io::INDEX_MAGIC)
}

/// Open the mailbox file at `path`, in any of its forms. Whatever the file and its content
/// folder hold, the result is consistent (`Mailbox::repair`).
pub fn open(path: &Path) -> Result<Opened, String> {
    let shared = shared(path);
    let mut st = lock(&shared.state);
    let show = path.display();
    let file = resolve(path);
    let len = fs::metadata(&file).map_err(|e| format!("{show}: {e}"))?.len();
    // Whatever was known about this file was about another state of it.
    *st = State { newest: st.newest, swept: st.swept, ..State::default() };
    if is_index(&file).map_err(|e| format!("{show}: {e}"))? {
        return open_index(&mut st, &file, len).map_err(|e| format!("{show}: {e}"));
    }
    if len > MAX_FILE {
        return Err(format!("{show} is larger than {} MB", MAX_FILE >> 20));
    }
    let bytes = fs::read(&file).map_err(|e| format!("{show}: {e}"))?;
    let mut mailbox: Mailbox = serde_json::from_slice(&bytes).map_err(|e| format!("{show}: this isn't a MailCraft mailbox ({e})"))?;
    let version = mailbox.version;
    let (split, filled) = match version {
        0 | 1 => (false, Filled::default()),
        SPLIT_VERSION => {
            let head: Head = serde_json::from_slice(&bytes).map_err(|e| format!("{show}: this mailbox file is damaged ({e})"))?;
            (true, fill(&mut st, &blob_dir(&file), &mut mailbox, head, None))
        }
        newer => return Err(format!("{show}: this mailbox was written by a newer MailCraft (format {newer})")),
    };
    // In memory a mailbox is whole, whichever form it came from.
    mailbox.version = 1;
    for line in mailbox.repair() {
        log::warn!("mailbox repaired: {line}");
    }
    report(&show, &blob_dir(&file), &filled);
    st.stamp = Stamp::of(&file, bytes.len() as u64);
    st.loaded = Loaded::of(&mailbox);
    st.legacy = Some(version.max(1));
    Ok(Opened { mailbox, split, unavailable: filled.missing, loaded: filled.loaded, deferred: filled.deferred, unfinished: 0 })
}

/// One item of a mailbox file in this form, read.
enum Parsed {
    Message(Arc<Message>, Option<WireParts>),
    Contact(Arc<Contact>),
    Event(Arc<Event>),
    Task(Arc<Task>),
}

fn parse_item(key: &Key, item: &index::Item) -> Result<(Parsed, Fp), String> {
    let damaged = |e: serde_json::Error| format!("this mailbox file is damaged (item {} of {:?}: {e})", key.1, key.0);
    let fp = item.fp;
    let parsed = match key.0 {
        Coll::Message => {
            let m: Message = serde_json::from_slice(item.json()).map_err(damaged)?;
            let refs = item.refs();
            let wire = if refs.is_empty() { None } else { Some(serde_json::from_slice::<WireParts>(refs).map_err(damaged)?) };
            Parsed::Message(Arc::new(m), wire)
        }
        Coll::Contact => Parsed::Contact(Arc::new(serde_json::from_slice(item.json()).map_err(damaged)?)),
        Coll::Event => Parsed::Event(Arc::new(serde_json::from_slice(item.json()).map_err(damaged)?)),
        Coll::Task => Parsed::Task(Arc::new(serde_json::from_slice(item.json()).map_err(damaged)?)),
    };
    Ok((parsed, fp))
}

/// Open a mailbox file in this form, `len` bytes long.
fn open_index(st: &mut State, file: &Path, len: u64) -> Result<Opened, String> {
    if len > MAX_INDEX {
        return Err(format!("the mailbox file is larger than {} GB", MAX_INDEX >> 30));
    }
    let f = fs::File::open(file).map_err(|e| e.to_string())?;
    let replayed = index::replay(std::io::BufReader::with_capacity(1 << 20, f), len)?;
    let (rest, rest_sum) = replayed.rest.ok_or("this mailbox file is damaged (it has no mailbox record)")?;
    let mut mailbox: Mailbox = serde_json::from_slice(&rest).map_err(|e| format!("this mailbox file is damaged ({e})"))?;
    let rest_fp = (rest_sum, index::FRAME + rest.len() as u64);
    drop(rest);
    // Items are their own records: whatever the rest says of them is no part of the mailbox.
    mailbox.messages = BTreeMap::new();
    mailbox.contacts = BTreeMap::new();
    mailbox.events = BTreeMap::new();
    mailbox.tasks = BTreeMap::new();

    // Each item parsed on one of a few threads (JSON is most of what opening costs), a slice at a
    // time from the lowest key, each slice's bytes let go once it is read: the file's bytes and
    // the mailbox they become are never both whole in memory.
    let mut items: Vec<(Key, index::Item)> = replayed.items.into_iter().collect();
    items.sort_unstable_by_key(|(key, _)| std::cmp::Reverse(*key));
    let mut records: Vec<(Key, Fp, u64)> = Vec::with_capacity(items.len());
    let mut head = Head { version: INDEX_VERSION, blobs: BTreeMap::new() };
    let (mut messages, mut contacts, mut events, mut tasks) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    while !items.is_empty() {
        let mut slice = items.split_off(items.len().saturating_sub(OPEN_SLICE));
        slice.reverse();
        let parsed = fan_out(slice.len(), |i| slice.get(i).map(|(key, item)| (*key, item.size(), parse_item(key, item))));
        drop(slice);
        for one in parsed {
            let Some(Some((key, size, result))) = one else { return Err("the mailbox file couldn't be read".into()) };
            let (item, fp) = result?;
            records.push((key, fp, size));
            match item {
                Parsed::Message(m, wire) => {
                    if let Some(wire) = wire {
                        head.blobs.insert(key.1, wire);
                    }
                    messages.push((key.1, m));
                }
                Parsed::Contact(c) => contacts.push((key.1, c)),
                Parsed::Event(e) => events.push((key.1, e)),
                Parsed::Task(t) => tasks.push((key.1, t)),
            }
        }
    }
    drop(items);
    // Sorted by key: the maps are built in one go.
    mailbox.messages = messages.into_iter().collect();
    mailbox.contacts = contacts.into_iter().collect();
    mailbox.events = events.into_iter().collect();
    mailbox.tasks = tasks.into_iter().collect();

    // What each message keeps outside, to know it again at the next save without working it out
    // (`fill` marks which of it is in memory).
    let mut parts: HashMap<Id, Parts> = head.blobs.iter().map(|(id, wire)| (*id, wire.parts())).collect();
    let dir = blob_dir(file);
    let filled = fill(st, &dir, &mut mailbox, head, Some(&mut parts));
    mailbox.version = 1;
    let fixed = mailbox.repair();
    for line in &fixed {
        log::warn!("mailbox repaired: {line}");
    }
    let show = file.display();
    report(&show, &dir, &filled);
    let unfinished = match replayed.tail {
        Tail::Bytes(n) => {
            log::warn!(
                "{show}: the last {n} bytes aren't a finished save (one cut short, or damage); the file is kept aside before it is next written"
            );
            n
        }
        Tail::Clean | Tail::Zeros => 0,
    };

    // What the file says, item by item. Each is the item in memory, unless the repair changed
    // something: then which is isn't known, each is worked out again at the next save, and what
    // was fixed is written.
    let mut disk = Disk::new(rest_fp);
    (disk.len, disk.tail) = (replayed.good_len, replayed.tail);
    let trusted = fixed.is_empty();
    fn of<T>(map: &BTreeMap<Id, Arc<T>>, id: Id, trusted: bool) -> Weak<T> {
        map.get(&id).filter(|_| trusted).map_or_else(Weak::new, Arc::downgrade)
    }
    for (key, fp, size) in records {
        disk.live = disk.live.saturating_add(size);
        let id = key.1;
        match key.0 {
            Coll::Message => {
                let parts = parts.remove(&id).unwrap_or_default();
                disk.messages.insert(id, Msg { entry: Entry { of: of(&mailbox.messages, id, trusted), fp, size }, parts });
            }
            Coll::Contact => {
                disk.contacts.insert(id, Entry { of: of(&mailbox.contacts, id, trusted), fp, size });
            }
            Coll::Event => {
                disk.events.insert(id, Entry { of: of(&mailbox.events, id, trusted), fp, size });
            }
            Coll::Task => {
                disk.tasks.insert(id, Entry { of: of(&mailbox.tasks, id, trusted), fp, size });
            }
        }
    }
    // The content it names, counted by its parts.
    st.live.clear();
    for m in disk.messages.values() {
        hold(&mut st.live, &m.parts);
    }
    st.stamp = Stamp::of(file, len);
    st.disk = Some(disk);
    st.loaded = Loaded::of(&mailbox);
    Ok(Opened { mailbox, split: true, unavailable: filled.missing, loaded: filled.loaded, deferred: filled.deferred, unfinished })
}

/// How the machine's memory stands, in bytes, where it can be read (Linux: `/proc/meminfo`).
fn physical_memory() -> Option<u64> {
    #[cfg(target_os = "linux")]
    let total = {
        let text = fs::read_to_string("/proc/meminfo").ok()?;
        let line = text.lines().find(|l| l.starts_with("MemTotal:"))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        kb.checked_mul(1024)
    };
    #[cfg(not(target_os = "linux"))]
    let total = None;
    total
}

#[cfg(test)]
thread_local! {
    /// A budget a test sets for the mailboxes it opens on this thread (0: the machine's).
    static TEST_BUDGET: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Open mailboxes on this thread with `budget` bytes for their bodies while `f` runs.
#[cfg(test)]
pub(crate) fn with_budget<T>(budget: u64, f: impl FnOnce() -> T) -> T {
    let before = TEST_BUDGET.replace(budget);
    let out = f();
    TEST_BUDGET.set(before);
    out
}

/// How many bytes of bodies a mailbox brings into memory when it opens: a quarter of the
/// machine's memory, [`FALLBACK_BUDGET`] when that can't be read, and never more than
/// [`MAX_CONTENT`]. The newest mail gets it; the rest is read when it is wanted.
pub fn body_budget() -> u64 {
    #[cfg(test)]
    {
        let set = TEST_BUDGET.get();
        if set > 0 {
            return set;
        }
    }
    physical_memory().map_or(FALLBACK_BUDGET, |m| m / 4).min(MAX_CONTENT)
}

/// What [`fill`] did with the content the mailbox file names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Filled {
    /// Parts brought into memory.
    loaded: usize,
    /// Parts left on disk, to be fetched when wanted.
    deferred: usize,
    /// Parts whose file is missing or damaged.
    missing: usize,
}

/// Whether the content `r` is on disk under `dir`, by a look at the file alone (its kind and
/// length, not its bytes).
fn present(dir: &Path, r: &Ref, folders: &Folders) -> bool {
    blob_path(dir, &r.hash).is_some_and(|(sub, file)| {
        r.len <= MAX_BLOB && folders.real(&r.hash, &sub) && fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_file() && m.len() == r.len)
    })
}

/// What a mailbox file wants for one part of a message.
struct Want {
    /// `None`: what the file gives as a name isn't one.
    r: Option<Ref>,
    /// Where its bytes will be (an index into what is read); `None` when it stays on disk.
    slot: Option<usize>,
}

impl Want {
    /// The part as the message names it while it stays outside memory.
    fn kept(&self) -> Option<ContentRef> {
        self.r.map(Ref::to_model)
    }
}

/// Put the content the mailbox file names back into `mailbox`'s messages: the bodies of the
/// newest mail up to [`body_budget`], the rest named on the messages to be fetched when wanted.
/// `parts`, when given, learns which parts are in memory. Returns what was done.
fn fill(st: &mut State, dir: &Path, mailbox: &mut Mailbox, head: Head, mut parts: Option<&mut HashMap<Id, Parts>>) -> Filled {
    // Every distinct piece of content read is read once, and only as much as fits the budget: a
    // file can name the same large content for a million bodies.
    let mut slots: Vec<(Ref, u32)> = Vec::new();
    let mut index: HashMap<Ref, usize> = HashMap::new();
    let mut budget = body_budget();
    let mut want = |wire: &Option<(String, u64)>, load: bool, live: &mut HashMap<Digest, u32>| -> Option<Want> {
        let wire = wire.as_ref()?;
        let Some(r) = parse_ref(wire) else { return Some(Want { r: None, slot: None }) };
        let n = live.entry(r.hash).or_default();
        *n = n.saturating_add(1);
        // A body is a copy of its own, whatever else names the same bytes.
        if !load || r.len > MAX_BLOB || r.len > budget {
            return Some(Want { r: Some(r), slot: None });
        }
        budget -= r.len;
        let slot = *index.entry(r).or_insert_with(|| {
            slots.push((r, 0));
            slots.len().saturating_sub(1)
        });
        if let Some((_, uses)) = slots.get_mut(slot) {
            *uses = uses.saturating_add(1);
        }
        Some(Want { r: Some(r), slot: Some(slot) })
    };
    // The newest mail first (by when it was sent, then by its number), so that what the budget
    // holds is what the owner reads; content of a message that isn't there is still named by the
    // file on disk, and counted, but nothing of it is read.
    let mut order: Vec<(i64, Id)> = head.blobs.keys().filter_map(|id| mailbox.messages.get(id).map(|m| (m.date, *id))).collect();
    order.sort_unstable_by(|a, b| b.cmp(a));
    let mut plan = Vec::with_capacity(order.len());
    for (_, id) in order {
        let Some(wire) = head.blobs.get(&id) else { continue };
        let text = want(&wire.text, true, &mut st.live);
        let html = want(&wire.html, true, &mut st.live);
        let attachments: Vec<Option<Want>> = wire.attachments.iter().map(|a| want(a, false, &mut st.live)).collect();
        plan.push((id, text, html, attachments));
    }
    for (id, wire) in &head.blobs {
        if !mailbox.messages.contains_key(id) {
            for part in wire.text.iter().chain(&wire.html).chain(wire.attachments.iter().flatten()) {
                want(&Some(part.clone()), false, &mut st.live);
            }
        }
    }
    drop(want);
    let folders = Folders::new();
    let read = fan_out(slots.len(), |i| slots.get(i).and_then(|(r, _)| get(dir, r, &folders)));
    let mut data: Vec<Option<Arc<Vec<u8>>>> = read.into_iter().map(|b| b.flatten().map(Arc::new)).collect();
    let mut uses: Vec<u32> = slots.iter().map(|(_, uses)| *uses).collect();
    drop(slots);
    // What stays on disk is looked for (its file, not its bytes), within a time cap, so that what
    // is missing is told from what is merely not loaded. Past the cap the rest is taken to be
    // there: a part that isn't says so when it is wanted.
    let mut on_disk: Vec<Ref> = Vec::new();
    let mut seen: HashSet<Ref> = HashSet::new();
    for (_, text, html, attachments) in &plan {
        for want in text.iter().chain(html).chain(attachments.iter().flatten()) {
            if let (Some(r), None) = (want.r, want.slot)
                && seen.insert(r)
            {
                on_disk.push(r);
            }
        }
    }
    drop(seen);
    let deadline = Instant::now() + STAT_TIME;
    let looked = fan_out(on_disk.len(), |i| on_disk.get(i).map(|r| (Instant::now() < deadline).then(|| present(dir, r, &folders))));
    let missing: HashSet<Ref> = on_disk.iter().zip(&looked).filter(|(_, l)| matches!(l, Some(Some(Some(false))))).map(|(r, _)| *r).collect();
    drop(on_disk);
    let mut filled = Filled::default();
    // Three ways a part can stand after this: in memory (`Some(true)`), on disk (`Some(false)`),
    // missing (`None`).
    let readable: Vec<bool> = data.iter().map(Option::is_some).collect();
    let stand = |want: &Want, filled: &mut Filled| -> Option<bool> {
        match (want.slot, want.r) {
            (Some(slot), _) if readable.get(slot).copied().unwrap_or(false) => {
                filled.loaded += 1;
                Some(true)
            }
            (None, Some(r)) if !missing.contains(&r) => {
                filled.deferred += 1;
                Some(false)
            }
            _ => {
                filled.missing += 1;
                None
            }
        }
    };
    // The bytes for one more use of a slot, as text; the last use takes them along instead of
    // a copy, so that what was read and the bodies it becomes are never both whole in memory.
    let mut take = |slot: usize| -> Option<String> {
        let left = uses.get_mut(slot)?;
        *left = left.saturating_sub(1);
        let bytes = if *left == 0 { data.get_mut(slot)?.take()? } else { data.get(slot)?.clone()? };
        let bytes = Arc::try_unwrap(bytes).unwrap_or_else(|used_elsewhere| used_elsewhere.to_vec());
        Some(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
    };
    for (id, text, html, attachments) in plan {
        let Some(m) = mailbox.messages.get_mut(&id).map(Arc::make_mut) else { continue };
        let mut held = parts.as_deref_mut().and_then(|p| p.remove(&id));
        let mut kept = KeptBody::default();
        if let Some(want) = &text {
            match stand(want, &mut filled) {
                Some(true) => {
                    if let Some(text) = want.slot.and_then(&mut take) {
                        m.body.text = text;
                    }
                }
                Some(false) => {
                    m.body.text = String::new();
                    kept.text = want.kept();
                }
                None => {
                    m.body.text = UNAVAILABLE.to_string();
                    kept.text = want.kept();
                    kept.failed = true;
                }
            }
            if let Some(p) = held.as_mut().and_then(|p| p.text.as_mut()) {
                p.held = !m.body.text.is_empty() && m.body.text != UNAVAILABLE;
            }
        }
        if let Some(want) = &html {
            // Without its HTML a message shows its plain text.
            m.body.html = None;
            match stand(want, &mut filled) {
                Some(true) => {
                    m.body.html = want.slot.and_then(&mut take);
                }
                Some(false) => kept.html = want.kept(),
                None => {
                    kept.html = want.kept();
                    kept.failed = true;
                }
            }
            if let Some(p) = held.as_mut().and_then(|p| p.html.as_mut()) {
                p.held = m.body.html.is_some();
            }
        }
        // (A missing HTML is noted on the text, which is otherwise there to read.)
        kept.text_hash = mailcraft_model::text_hash(&m.body.text);
        m.kept = (!kept.is_empty()).then(|| Box::new(kept));
        for (i, want) in attachments.iter().enumerate() {
            let (Some(want), Some(a)) = (want, m.attachments.get_mut(i)) else { continue };
            // The attachment keeps its name and type, with nothing in it until it is wanted.
            a.data = Arc::default();
            a.kept = want.kept();
            stand(want, &mut filled);
            if let Some(p) = held.as_mut().and_then(|p| p.attachments.get_mut(i)).and_then(Option::as_mut) {
                p.held = false;
            }
        }
        if let (Some(parts), Some(held)) = (parts.as_deref_mut(), held) {
            parts.insert(id, held);
        }
    }
    filled
}

// ---- fetching ----

/// Fetched content kept for the next time it is wanted, newest last, bounded by bytes. Keyed by
/// the content folder it came from as well as the name: one mailbox's cache answers nothing
/// about another's folder.
#[derive(Default)]
struct Cache {
    items: HashMap<(PathBuf, Ref), Arc<Vec<u8>>>,
    order: std::collections::VecDeque<(PathBuf, Ref)>,
    bytes: u64,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

impl Cache {
    fn get(&mut self, dir: &Path, r: &Ref) -> Option<Arc<Vec<u8>>> {
        let key = (dir.to_path_buf(), *r);
        let bytes = self.items.get(&key)?.clone();
        // Wanted again: it goes to the back of the line for eviction.
        if let Some(at) = self.order.iter().position(|x| *x == key) {
            self.order.remove(at);
            self.order.push_back(key);
        }
        Some(bytes)
    }

    fn put(&mut self, dir: &Path, r: Ref, bytes: Arc<Vec<u8>>) {
        let key = (dir.to_path_buf(), r);
        if r.len > CACHE_ITEM_MAX || self.items.contains_key(&key) {
            return;
        }
        while self.bytes.saturating_add(r.len) > CACHE_MAX {
            let Some(old) = self.order.pop_front() else { break };
            if self.items.remove(&old).is_some() {
                self.bytes = self.bytes.saturating_sub(old.1.len);
            }
        }
        self.bytes = self.bytes.saturating_add(r.len);
        self.order.push_back(key.clone());
        self.items.insert(key, bytes);
    }
}

/// How many bytes the cache of fetched content holds.
pub fn cached_bytes() -> u64 {
    lock(cache()).bytes
}

/// Let go of everything cached (a mailbox was closed; tests).
pub fn clear_cache() {
    *lock(cache()) = Cache::default();
}

/// The content `r` of the mailbox file at `path`, from the cache or from the content folder
/// beside the file, checked against the length and SHA-256 `r` names.
pub fn fetch(path: &Path, r: &ContentRef) -> Result<Arc<Vec<u8>>, String> {
    let r = Ref::from_model(r);
    let dir = blob_dir(&resolve(path));
    if let Some(bytes) = lock(cache()).get(&dir, &r) {
        return Ok(bytes);
    }
    let bytes =
        get(&dir, &r, &Folders::new()).map(Arc::new).ok_or_else(|| format!("the file that keeps it is missing from {} or damaged", dir.display()))?;
    lock(cache()).put(&dir, r, bytes.clone());
    Ok(bytes)
}

/// The content folder beside the mailbox file, as the engine's source of content that isn't in
/// memory (`crate::io::ContentSource`): what a desktop host gives its session.
#[derive(Clone, Copy, Debug, Default)]
pub struct Content;

impl ContentSource for Content {
    fn fetch(&self, mailbox: &Path, r: &ContentRef) -> Result<Arc<Vec<u8>>, String> {
        fetch(mailbox, r)
    }
}

/// Bring every kept part of every message of `mailbox` into memory from the content folder
/// beside the file at `path`, the same bytes shared where messages share a file. Returns how many
/// parts were brought in; a part that can't be read is an error naming its message.
pub fn load_all(path: &Path, mailbox: &mut Mailbox) -> Result<usize, String> {
    let mut got: HashMap<ContentRef, Arc<Vec<u8>>> = HashMap::new();
    let mut n = 0;
    let ids: Vec<Id> = mailbox.messages.keys().copied().collect();
    for id in ids {
        let Some(m) = mailbox.messages.get(&id) else { continue };
        if m.is_loaded() {
            continue;
        }
        let mut m = Message::clone(m);
        let mut take = |r: &ContentRef| -> Result<Arc<Vec<u8>>, String> {
            if let Some(bytes) = got.get(r) {
                return Ok(bytes.clone());
            }
            let bytes = fetch(path, r).map_err(|e| format!("message {id}: {e}"))?;
            got.insert(*r, bytes.clone());
            Ok(bytes)
        };
        if m.body_is_kept()
            && let Some(kept) = m.kept.take()
        {
            if let Some(r) = kept.text.filter(|_| m.body.text.is_empty()) {
                m.body.text = text_of_bytes(&take(&r)?);
                n += 1;
            }
            if let Some(r) = kept.html.filter(|_| m.body.html.is_none()) {
                m.body.html = Some(text_of_bytes(&take(&r)?));
                n += 1;
            }
        }
        for a in &mut m.attachments {
            if let Some(r) = a.kept.filter(|_| a.data.is_empty()) {
                a.data = take(&r)?;
                n += 1;
            }
            a.kept = None;
        }
        mailbox.messages.insert(id, Arc::new(m));
    }
    Ok(n)
}

/// Bytes as text, the way a body is read back: what isn't UTF-8 is read as well as it can be.
pub fn text_of_bytes(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

// ---- saving ----

/// What a save wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    /// Bytes of the mailbox file written (0 when it already was as it should be).
    pub bytes: u64,
    /// Content files written, and their bytes.
    pub blobs: usize,
    pub blob_bytes: u64,
    /// Nothing was written: the file already holds this state, or a newer one.
    pub unchanged: bool,
    /// The mailbox file was written whole, not appended to.
    pub compacted: bool,
}

/// Save `mailbox` to the file at `path` and its content folder. An error leaves the file as it
/// was (and readable).
pub fn save(path: &Path, mailbox: &Mailbox) -> Result<Saved, String> {
    save_as_of(path, mailbox, next_seq())
}

fn save_as_of(path: &Path, mailbox: &Mailbox, seq: u64) -> Result<Saved, String> {
    let shared = shared(path);
    let mut st = lock(&shared.state);
    if seq < st.newest {
        return Ok(Saved { unchanged: true, ..Saved::default() });
    }
    let saved = write_index(&mut st, &resolve(path), mailbox).map_err(|e| format!("{}: {e}", path.display()))?;
    st.newest = seq;
    Ok(saved)
}

/// Save `mailbox` to `path` as one self-contained file (an export, a copy to carry somewhere),
/// written whole and atomically. Returns the bytes written.
pub fn save_single(path: &Path, mailbox: &Mailbox) -> Result<u64, String> {
    let bytes = crate::io::write_mailbox(mailbox)?;
    let shared = shared(path);
    let mut st = lock(&shared.state);
    write_atomic(&resolve(path), &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    // The file is another one now: nothing known about the old one holds, and a state of it
    // still waiting to be written has had its turn.
    *st = State { newest: next_seq(), swept: st.swept, ..State::default() };
    Ok(bytes.len() as u64)
}

/// What a message comes to in the mailbox file: itself without the content that goes outside,
/// and the names of that content.
fn prepare(id: Id, m: &Arc<Message>, hashes: &mut HashMap<usize, (Weak<Vec<u8>>, Digest)>) -> Result<Prepared, String> {
    let outside = |bytes: &[u8]| (bytes.len() > INLINE_MAX).then(|| Part { r: Ref::of(bytes), held: true });
    let on_disk = |r: &ContentRef| Part { r: Ref::from_model(r), held: false };
    // Content that stayed on disk when the mailbox was opened (or couldn't be read then) keeps
    // the name the message carries for it, as long as the message still shows what was put in
    // the content's place: nothing (or the notice) for its text, no HTML beside that, an empty
    // file. A text written anew is new content, and the HTML that went with the old one goes.
    let kept = m.kept.as_deref();
    let placeholder = m.body.text.is_empty() || m.body.text == UNAVAILABLE;
    let same_text = kept.is_some_and(|k| k.same_text(&m.body.text));
    let mut parts = Parts {
        text: kept.filter(|_| placeholder).and_then(|k| k.text.as_ref()).map(on_disk).or_else(|| outside(m.body.text.as_bytes())),
        html: match &m.body.html {
            None => kept.filter(|_| same_text).and_then(|k| k.html.as_ref()).map(on_disk),
            Some(html) => outside(html.as_bytes()),
        },
        attachments: Vec::new(),
    };
    for a in m.attachments.iter().take(MAX_PARTS) {
        let was = a.kept.as_ref().filter(|_| a.data.is_empty()).map(on_disk);
        let part = was.or_else(|| {
            (a.data.len() > INLINE_MAX).then(|| {
                let key = Arc::as_ptr(&a.data) as usize;
                let known = hashes.get(&key).filter(|(of, _)| std::ptr::eq(of.as_ptr(), Arc::as_ptr(&a.data))).map(|(_, hash)| *hash);
                let hash = known.unwrap_or_else(|| {
                    let hash = Digest::of(&a.data);
                    hashes.insert(key, (Arc::downgrade(&a.data), hash));
                    hash
                });
                Part { r: Ref { hash, len: a.data.len() as u64 }, held: true }
            })
        });
        parts.attachments.push(part);
    }
    if parts.attachments.iter().all(Option::is_none) {
        parts.attachments.clear();
    }
    let mut lean = Message::clone(m);
    if parts.text.is_some() {
        lean.body.text = String::new();
    }
    if parts.html.is_some() {
        lean.body.html = None;
    }
    let nothing: Arc<Vec<u8>> = Arc::default();
    for (a, part) in lean.attachments.iter_mut().zip(&parts.attachments) {
        if part.is_some() {
            a.data = nothing.clone();
        }
    }
    let json = serde_json::to_vec(&lean).map_err(|e| e.to_string())?;
    let refs = if parts.is_empty() { Vec::new() } else { parts.json().into_bytes() };
    let fp = index::put_fp((Coll::Message, id), &json, &refs).map_err(io_error)?;
    Ok(Prepared { of: Arc::downgrade(m), fp, parts, json, refs })
}

/// A mailbox file never names content that isn't beside it: a part whose bytes aren't in memory
/// (it stayed on disk when the mailbox was opened) must be in this file's folder, which it is
/// when the file already names it (`live`), else when its file is there. A message carried over
/// from a mailbox kept elsewhere, with its content still there, is refused with why.
fn on_disk_here(id: Id, parts: &Parts, live: &HashMap<Digest, u32>, dir: &Path, folders: &Folders) -> Result<(), String> {
    for p in parts.all().filter(|p| !p.held && !live.contains_key(&p.r.hash)) {
        if !present(dir, &p.r, folders) {
            return Err(format!(
                "message {id}: its content isn't in this mailbox's folder (it was opened from another mailbox); save the mailbox where it was opened, or save it as one file first"
            ));
        }
    }
    Ok(())
}

/// A message worked out ([`prepare`]): its record's JSON and content names, and what to remember
/// of it.
struct Prepared {
    of: Weak<Message>,
    fp: Fp,
    parts: Parts,
    json: Vec<u8>,
    refs: Vec<u8>,
}

/// The mailbox without the items kept in records of their own, as its rest record has it.
fn rest_json(mailbox: &Mailbox) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    mailbox.serialize(Shell(&mut serde_json::Serializer::new(&mut out))).map_err(|e| e.to_string())?;
    Ok(out)
}

/// The fields of a mailbox kept in records of their own.
const ITEMS: [&str; 4] = ["messages", "contacts", "events", "tasks"];

/// Writes a mailbox as the model does, with each field of [`ITEMS`] an empty map and this form's
/// format number: what a copy without its items would come to, without copying the items (half a
/// million of them, at every save).
struct Shell<S>(S);

/// The mailbox fields as [`Shell`] writes them.
struct ShellStruct<S>(S);

/// An empty map.
struct Empty;

impl Serialize for Empty {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde::ser::SerializeMap::end(s.serialize_map(Some(0))?)
    }
}

fn not_a_mailbox<E: serde::ser::Error>() -> E {
    E::custom("only a mailbox is written this way")
}

/// The methods of [`Shell`] for what isn't a struct: a mailbox is one.
macro_rules! not_a_mailbox {
    ($($name:ident($($arg:ty),*) -> $out:ty;)*) => {
        $(fn $name(self, $(_: $arg),*) -> Result<$out, S::Error> { Err(not_a_mailbox()) })*
    };
}

impl<S: serde::Serializer> serde::Serializer for Shell<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = serde::ser::Impossible<S::Ok, S::Error>;
    type SerializeTuple = serde::ser::Impossible<S::Ok, S::Error>;
    type SerializeTupleStruct = serde::ser::Impossible<S::Ok, S::Error>;
    type SerializeTupleVariant = serde::ser::Impossible<S::Ok, S::Error>;
    type SerializeMap = serde::ser::Impossible<S::Ok, S::Error>;
    type SerializeStruct = ShellStruct<S::SerializeStruct>;
    type SerializeStructVariant = serde::ser::Impossible<S::Ok, S::Error>;

    fn serialize_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeStruct, S::Error> {
        Ok(ShellStruct(self.0.serialize_struct(name, len)?))
    }

    not_a_mailbox! {
        serialize_bool(bool) -> S::Ok;
        serialize_i8(i8) -> S::Ok;
        serialize_i16(i16) -> S::Ok;
        serialize_i32(i32) -> S::Ok;
        serialize_i64(i64) -> S::Ok;
        serialize_u8(u8) -> S::Ok;
        serialize_u16(u16) -> S::Ok;
        serialize_u32(u32) -> S::Ok;
        serialize_u64(u64) -> S::Ok;
        serialize_f32(f32) -> S::Ok;
        serialize_f64(f64) -> S::Ok;
        serialize_char(char) -> S::Ok;
        serialize_str(&str) -> S::Ok;
        serialize_bytes(&[u8]) -> S::Ok;
        serialize_none() -> S::Ok;
        serialize_unit() -> S::Ok;
        serialize_unit_struct(&'static str) -> S::Ok;
        serialize_unit_variant(&'static str, u32, &'static str) -> S::Ok;
        serialize_seq(Option<usize>) -> Self::SerializeSeq;
        serialize_tuple(usize) -> Self::SerializeTuple;
        serialize_tuple_struct(&'static str, usize) -> Self::SerializeTupleStruct;
        serialize_tuple_variant(&'static str, u32, &'static str, usize) -> Self::SerializeTupleVariant;
        serialize_map(Option<usize>) -> Self::SerializeMap;
        serialize_struct_variant(&'static str, u32, &'static str, usize) -> Self::SerializeStructVariant;
    }

    fn serialize_some<T: ?Sized + Serialize>(self, _: &T) -> Result<S::Ok, S::Error> {
        Err(not_a_mailbox())
    }

    fn serialize_newtype_struct<T: ?Sized + Serialize>(self, _: &'static str, _: &T) -> Result<S::Ok, S::Error> {
        Err(not_a_mailbox())
    }

    fn serialize_newtype_variant<T: ?Sized + Serialize>(self, _: &'static str, _: u32, _: &'static str, _: &T) -> Result<S::Ok, S::Error> {
        Err(not_a_mailbox())
    }
}

impl<S: serde::ser::SerializeStruct> serde::ser::SerializeStruct for ShellStruct<S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, key: &'static str, value: &T) -> Result<(), S::Error> {
        match key {
            "version" => self.0.serialize_field(key, &INDEX_VERSION),
            _ if ITEMS.contains(&key) => self.0.serialize_field(key, &Empty),
            _ => self.0.serialize_field(key, value),
        }
    }

    fn skip_field(&mut self, key: &'static str) -> Result<(), S::Error> {
        self.0.skip_field(key)
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

/// How the contacts, events or tasks differ from what the file says of them.
struct Changes<T> {
    /// Records to write: each item's entry once they are.
    put: Vec<(Id, Entry<T>)>,
    /// Items as the file says them, in another copy in memory.
    relink: Vec<(Id, Weak<T>)>,
    gone: Vec<Id>,
}

/// What a save appends, worked out against what the file says ([`delta`]).
struct Delta<'m> {
    /// The records and their commit, to append in one write.
    group: Vec<u8>,
    records: u32,
    /// Content to write first.
    wanted: HashMap<Ref, &'m [u8]>,
    messages: Vec<(Id, Msg)>,
    relink: Vec<(Id, Weak<Message>)>,
    gone: Vec<Id>,
    contacts: Changes<Contact>,
    events: Changes<Event>,
    tasks: Changes<Task>,
    rest: Option<(Fp, u64)>,
}

fn io_error(e: std::io::Error) -> String {
    e.to_string()
}

/// Walk `now` and `was` side by side, both in the order of their ids: `each` is given every item
/// of `now` with what `was` has under its id. Returns the ids only `was` has, in order.
fn walk<'a, 'b, A, B>(
    now: &'a BTreeMap<Id, A>,
    was: &'b BTreeMap<Id, B>,
    mut each: impl FnMut(&'a Id, &'a A, Option<&'b B>) -> Result<(), String>,
) -> Result<Vec<Id>, String> {
    let mut old = was.iter().peekable();
    let mut gone = Vec::new();
    for (id, item) in now {
        while let Some((before, _)) = old.next_if(|(other, _)| *other < id) {
            gone.push(*before);
        }
        let same = old.next_if(|(other, _)| *other == id).map(|(_, b)| b);
        each(id, item, same)?;
    }
    gone.extend(old.map(|(id, _)| *id));
    Ok(gone)
}

/// Work out what `mailbox` changed since the file said `d`: an item that is the one the file
/// says costs a step of a walk; any other is worked out and compared with what the file says.
fn delta<'m>(st: &mut State, path: &Path, mailbox: &'m Mailbox, d: &Disk) -> Result<Delta<'m>, String> {
    let (dir, folders) = (blob_dir(path), Folders::new());
    let mut group = Vec::new();
    let mut records = 0u32;
    let (mut wanted, mut messages, mut relink) = (HashMap::new(), Vec::new(), Vec::new());
    let gone = walk(&mailbox.messages, &d.messages, |id, m, was| {
        if was.is_some_and(|w| w.entry.is(m)) {
            return Ok(());
        }
        let p = prepare(*id, m, &mut st.hashes)?;
        if was.is_some_and(|w| w.entry.fp == p.fp) {
            relink.push((*id, p.of));
            return Ok(());
        }
        for (r, bytes) in p.parts.content(m) {
            if !st.live.contains_key(&r.hash) {
                wanted.entry(r).or_insert(bytes);
            }
        }
        on_disk_here(*id, &p.parts, &st.live, &dir, &folders)?;
        let (size, _) = index::put(&mut group, (Coll::Message, *id), &p.json, &p.refs).map_err(io_error)?;
        records = records.saturating_add(1);
        messages.push((*id, Msg { entry: Entry { of: p.of, fp: p.fp, size }, parts: p.parts }));
        Ok(())
    })?;
    for id in &gone {
        index::delete(&mut group, (Coll::Message, *id)).map_err(io_error)?;
        records = records.saturating_add(1);
    }
    let contacts = changes(&mailbox.contacts, Coll::Contact, &d.contacts, &mut group, &mut records)?;
    let events = changes(&mailbox.events, Coll::Event, &d.events, &mut group, &mut records)?;
    let tasks = changes(&mailbox.tasks, Coll::Task, &d.tasks, &mut group, &mut records)?;
    let rest_json = rest_json(mailbox)?;
    let fp = index::rest_fp(&rest_json).map_err(io_error)?;
    let rest = if fp == d.rest.0 {
        None
    } else {
        records = records.saturating_add(1);
        Some((fp, index::rest(&mut group, &rest_json).map_err(io_error)?.0))
    };
    if records > 0 {
        index::commit(&mut group, records).map_err(io_error)?;
    }
    Ok(Delta { group, records, wanted, messages, relink, gone, contacts, events, tasks, rest })
}

/// [`delta`] for the contacts, events or tasks.
fn changes<T: Serialize>(
    map: &BTreeMap<Id, Arc<T>>,
    coll: Coll,
    was: &BTreeMap<Id, Entry<T>>,
    group: &mut Vec<u8>,
    records: &mut u32,
) -> Result<Changes<T>, String> {
    let (mut put, mut relink) = (Vec::new(), Vec::new());
    let gone = walk(map, was, |id, item, old| {
        if old.is_some_and(|e| e.is(item)) {
            return Ok(());
        }
        let json = serde_json::to_vec(&**item).map_err(|e| e.to_string())?;
        let fp = index::put_fp((coll, *id), &json, b"").map_err(io_error)?;
        if old.is_some_and(|e| e.fp == fp) {
            relink.push((*id, Arc::downgrade(item)));
            return Ok(());
        }
        let (size, _) = index::put(group, (coll, *id), &json, b"").map_err(io_error)?;
        *records = records.saturating_add(1);
        put.push((*id, Entry { of: Arc::downgrade(item), fp, size }));
        Ok(())
    })?;
    for id in &gone {
        index::delete(group, (coll, *id)).map_err(io_error)?;
        *records = records.saturating_add(1);
    }
    Ok(Changes { put, relink, gone })
}

/// Make `d` say what the file says once `delta`'s records are written.
fn apply(d: &mut Disk, live: &mut HashMap<Digest, u32>, delta: Delta<'_>) {
    for (id, of) in delta.relink {
        if let Some(m) = d.messages.get_mut(&id) {
            m.entry.of = of;
        }
    }
    for (id, m) in delta.messages {
        hold(live, &m.parts);
        d.live = d.live.saturating_add(m.entry.size);
        if let Some(old) = d.messages.insert(id, m) {
            release(live, &old.parts);
            d.live = d.live.saturating_sub(old.entry.size);
        }
    }
    for id in delta.gone {
        if let Some(old) = d.messages.remove(&id) {
            release(live, &old.parts);
            d.live = d.live.saturating_sub(old.entry.size);
        }
    }
    apply_changes(&mut d.contacts, &mut d.live, delta.contacts);
    apply_changes(&mut d.events, &mut d.live, delta.events);
    apply_changes(&mut d.tasks, &mut d.live, delta.tasks);
    if let Some(rest) = delta.rest {
        d.live = d.live.saturating_sub(d.rest.1).saturating_add(rest.1);
        d.rest = rest;
    }
}

fn apply_changes<T>(entries: &mut BTreeMap<Id, Entry<T>>, live: &mut u64, changes: Changes<T>) {
    for (id, of) in changes.relink {
        if let Some(e) = entries.get_mut(&id) {
            e.of = of;
        }
    }
    for (id, e) in changes.put {
        *live = live.saturating_add(e.size);
        if let Some(old) = entries.insert(id, e) {
            *live = live.saturating_sub(old.size);
        }
    }
    for id in changes.gone {
        if let Some(old) = entries.remove(&id) {
            *live = live.saturating_sub(old.size);
        }
    }
}

/// Write the content files `wanted` that aren't there yet into the folder beside `path`.
fn write_content(path: &Path, wanted: &HashMap<Ref, &[u8]>, saved: &mut Saved) -> Result<(), String> {
    if wanted.is_empty() {
        return Ok(());
    }
    let wanted: Vec<(Ref, &[u8])> = wanted.iter().map(|(r, bytes)| (*r, *bytes)).collect();
    // The folder beside the mailbox file, and no more: a mailbox isn't saved to a place that
    // isn't there.
    let dir = blob_dir(path);
    if let Err(e) = make_dir(&dir)
        && !dir.is_dir()
    {
        return Err(format!("{}: {e}", dir.display()));
    }
    let done = fan_out(wanted.len(), |i| wanted.get(i).map(|(r, bytes)| put(&dir, r, bytes)));
    for (result, (r, _)) in done.into_iter().zip(&wanted) {
        match result.flatten() {
            Some(Ok(true)) => {
                saved.blobs += 1;
                saved.blob_bytes += r.len;
            }
            Some(Ok(false)) => {}
            Some(Err(e)) => return Err(format!("content file {}: {e}", r.hash.hex())),
            None => return Err("the content couldn't be written".into()),
        }
    }
    Ok(())
}

/// Append `group` to the mailbox file at `path`, which must be `at` bytes long, flushed to the
/// disk. A write that fails is taken back as far as it can be (what is left of it is closed by
/// no commit, and isn't read).
fn append(path: &Path, at: u64, group: &[u8]) -> std::io::Result<()> {
    let mut f = fs::OpenOptions::new().append(true).open(path)?;
    if f.metadata()?.len() != at {
        return Err(std::io::Error::other("the mailbox file changed while it was being saved"));
    }
    let written = f.write_all(group).and_then(|()| f.sync_data());
    if written.is_err() {
        let _ = f.set_len(at).and_then(|()| f.sync_data());
    }
    written
}

/// Keep the file at `path` as it is under `<file>.<what>` (or `.<what>-2` and so on when that is
/// taken) before it is written over: a second name for the same file where the system has them,
/// a copy otherwise. Returns where it went.
fn keep_aside(path: &Path, what: &str) -> std::io::Result<PathBuf> {
    let name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    for n in 1..=100 {
        let mut aside = name.clone();
        aside.push(if n == 1 { format!(".{what}") } else { format!(".{what}-{n}") });
        let aside = path.with_file_name(aside);
        if fs::symlink_metadata(&aside).is_ok() {
            continue;
        }
        if fs::hard_link(path, &aside).is_err() {
            let mut from = fs::File::open(path)?;
            let mut to = create_private(&aside)?;
            std::io::copy(&mut from, &mut to).and_then(|_| to.sync_all()).inspect_err(|_| {
                let _ = fs::remove_file(&aside);
            })?;
        }
        sync_dir_of(&aside);
        return Ok(aside);
    }
    Err(std::io::Error::other("too many files kept aside already"))
}

fn write_index(st: &mut State, path: &Path, mailbox: &Mailbox) -> Result<Saved, String> {
    // Somebody else wrote the file since: what it names now is theirs to say, and what it named
    // before may be swept any time.
    let ours = st.stamp.as_ref().is_some_and(|s| s.still(path));
    if !ours {
        st.live.clear();
        st.stamp = None;
        st.disk = None;
        st.legacy = None;
    }
    if ours && st.loaded.as_ref().is_some_and(|loaded| loaded.is(mailbox)) {
        return Ok(Saved { unchanged: true, ..Saved::default() });
    }
    st.loaded = None;
    let prior = st.disk.take();
    let damaged = matches!(prior, Some(Disk { tail: Tail::Bytes(_), .. }));
    let Some(mut d) = prior.filter(Disk::appendable) else { return compact(st, path, mailbox, damaged) };
    let mut delta = match delta(st, path, mailbox, &d) {
        Ok(delta) => delta,
        Err(e) => {
            st.disk = Some(d);
            return Err(e);
        }
    };
    // The content first: a mailbox file never names what isn't on disk yet.
    let mut saved = Saved::default();
    if let Err(e) = write_content(path, &delta.wanted, &mut saved) {
        st.disk = Some(d);
        return Err(e);
    }
    let group = std::mem::take(&mut delta.group);
    let records = delta.records;
    apply(&mut d, &mut st.live, delta);
    if records == 0 {
        st.disk = Some(d);
        saved.unchanged = saved.blobs == 0;
        return Ok(saved);
    }
    // When what was appended outgrows what the file says, it is written whole instead.
    d.len = d.len.saturating_add(group.len() as u64);
    if d.outgrown() {
        return compact(st, path, mailbox, false).map(|whole| Saved {
            blobs: saved.blobs + whole.blobs,
            blob_bytes: saved.blob_bytes + whole.blob_bytes,
            ..whole
        });
    }
    if let Err(e) = append(path, d.len.saturating_sub(group.len() as u64), &group) {
        // What the file holds now is for the next save to find out: it writes it whole.
        st.stamp = None;
        return Err(e.to_string());
    }
    st.stamp = Stamp::of(path, d.len);
    st.disk = Some(d);
    saved.bytes = group.len() as u64;
    Ok(saved)
}

/// Write the mailbox file at `path` whole: the rest of the mailbox and every item, closed by one
/// commit, to a temporary file renamed over once the content it names is on disk. A file that
/// ends in what isn't a finished save (`damaged`) is kept aside first, unread bytes and all, or
/// nothing is written; so is a file of an earlier form, the first time this form takes its place.
fn compact(st: &mut State, path: &Path, mailbox: &Mailbox, damaged: bool) -> Result<Saved, String> {
    let keep = if damaged {
        let secs = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Some((format!("damaged-{secs}"), true))
    } else {
        st.legacy.map(|version| (format!("format{version}"), false))
    };
    let rest = rest_json(mailbox)?;
    let mut out = AtomicFile::create(path).map_err(io_error)?;
    out.write_all(&index::header()).map_err(io_error)?;
    let (rest_size, rest_fp) = index::rest(&mut out, &rest).map_err(io_error)?;
    let mut d = Disk::new((rest_fp, rest_size));
    drop(rest);
    let mut live = HashMap::new();
    let mut wanted = HashMap::new();
    let mut records: u32 = 1;
    let too_many = || "the mailbox holds too many items to be saved".to_string();
    let (dir, folders) = (blob_dir(path), Folders::new());
    for (id, m) in &mailbox.messages {
        let p = prepare(*id, m, &mut st.hashes)?;
        for (r, bytes) in p.parts.content(m) {
            if !st.live.contains_key(&r.hash) {
                wanted.entry(r).or_insert(bytes);
            }
        }
        on_disk_here(*id, &p.parts, &st.live, &dir, &folders)?;
        let (size, _) = index::put(&mut out, (Coll::Message, *id), &p.json, &p.refs).map_err(io_error)?;
        hold(&mut live, &p.parts);
        d.live = d.live.saturating_add(size);
        records = records.checked_add(1).ok_or_else(too_many)?;
        d.messages.insert(*id, Msg { entry: Entry { of: p.of, fp: p.fp, size }, parts: p.parts });
    }
    compact_items(&mailbox.contacts, Coll::Contact, &mut out, &mut d.contacts, &mut d.live, &mut records)?;
    compact_items(&mailbox.events, Coll::Event, &mut out, &mut d.events, &mut d.live, &mut records)?;
    compact_items(&mailbox.tasks, Coll::Task, &mut out, &mut d.tasks, &mut d.live, &mut records)?;
    let commit = index::commit(&mut out, records).map_err(io_error)?;
    d.len = index::HEADER_LEN.saturating_add(d.live).saturating_add(commit);
    // The content first: the file isn't the mailbox until it is renamed over.
    let mut saved = Saved::default();
    write_content(path, &wanted, &mut saved)?;
    drop(wanted);
    if let Some((what, needed)) = keep {
        match keep_aside(path, &what) {
            Ok(aside) => log::info!("the mailbox file as it was is kept as {}", aside.display()),
            Err(e) if needed => return Err(format!("the mailbox file couldn't be kept aside before it is written again ({e})")),
            Err(e) => log::warn!("the mailbox file as it was couldn't be kept aside ({e})"),
        }
    }
    out.commit().map_err(io_error)?;
    st.live = live;
    st.stamp = Stamp::of(path, d.len);
    st.legacy = None;
    saved.bytes = d.len;
    saved.compacted = true;
    st.disk = Some(d);
    Ok(saved)
}

/// Write each item of a collection's record for [`compact`].
fn compact_items<T: Serialize>(
    map: &BTreeMap<Id, Arc<T>>,
    coll: Coll,
    out: &mut impl std::io::Write,
    entries: &mut BTreeMap<Id, Entry<T>>,
    live: &mut u64,
    records: &mut u32,
) -> Result<(), String> {
    for (id, item) in map {
        let json = serde_json::to_vec(&**item).map_err(|e| e.to_string())?;
        let (size, fp) = index::put(out, (coll, *id), &json, b"").map_err(io_error)?;
        *live = live.saturating_add(size);
        *records = records.checked_add(1).ok_or("the mailbox holds too many items to be saved")?;
        entries.insert(*id, Entry { of: Arc::downgrade(item), fp, size });
    }
    Ok(())
}

// ---- sweeping ----

/// What a sweep did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// Files removed, and their bytes.
    pub removed: usize,
    pub bytes: u64,
    /// Content files left in place.
    pub kept: usize,
}

/// The content the mailbox file at `path` names, read from the file itself. `None` when the file
/// isn't in a form that keeps content outside, or when it ends in what isn't a finished save
/// (which may name content the rest doesn't).
fn named_by(path: &Path) -> Result<Option<HashSet<Digest>>, String> {
    let len = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if is_index(path)? {
        if len > MAX_INDEX {
            return Err("too large".into());
        }
        let replayed = index::replay(std::io::BufReader::with_capacity(1 << 20, fs::File::open(path).map_err(|e| e.to_string())?), len)?;
        if matches!(replayed.tail, Tail::Bytes(_)) {
            return Ok(None);
        }
        // Only a file that opens is believed about what it names.
        serde_json::from_slice::<Mailbox>(&replayed.rest.as_ref().ok_or("no mailbox record")?.0).map_err(|e| e.to_string())?;
        let items: Vec<(&Key, &index::Item)> = replayed.items.iter().collect();
        let mut named = HashSet::new();
        for parsed in fan_out(items.len(), |i| items.get(i).map(|(key, item)| parse_item(key, item))) {
            if let (Parsed::Message(_, Some(wire)), _) = parsed.flatten().ok_or("the mailbox file couldn't be read")?? {
                named.extend(wire.hashes());
            }
        }
        return Ok(Some(named));
    }
    if len > MAX_FILE {
        return Err("too large".into());
    }
    let head: Head = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if head.version != SPLIT_VERSION {
        return Ok(None);
    }
    Ok(Some(head.blobs.values().flat_map(WireParts::hashes).collect()))
}

/// Whether a mailbox file is kept aside beside `file` because it was damaged.
fn damaged_beside(file: &Path) -> bool {
    let (Some(parent), Some(stem)) = (file.parent(), file.file_name().and_then(|n| n.to_str())) else { return false };
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    let prefix = format!("{stem}.damaged-");
    fs::read_dir(parent).into_iter().flatten().flatten().take(MAX_ENTRIES).any(|e| e.file_name().to_str().is_some_and(|n| n.starts_with(&prefix)))
}

/// Remove the content files next to the mailbox file at `path` that it no longer names, and
/// temporary files a crash left behind, when they are older than `grace` ([`GRACE`]): a save in
/// flight has written its content before its mailbox file names it. Without a readable mailbox
/// file in this form nothing is removed, and nothing that isn't recognisably ours ever is.
pub fn sweep(path: &Path, grace: Duration) -> Result<Swept, String> {
    let shared = shared(path);
    let mut st = lock(&shared.state);
    st.swept = Some(Instant::now());
    let file = resolve(path);
    let dir = blob_dir(&file);
    // What a damaged file kept aside names may be wanted back.
    if damaged_beside(&file) {
        log::info!("{}: not sweeping its content folder while a damaged mailbox file is kept beside it", path.display());
        return Ok(Swept::default());
    }
    // The file as this program last read or wrote it names what it knows; otherwise it is read.
    let known = st.disk.as_ref().is_some_and(Disk::appendable) && st.stamp.as_ref().is_some_and(|s| s.still(&file));
    let named = if known { Some(st.live.keys().copied().collect()) } else { named_by(&file).map_err(|e| format!("{}: {e}", path.display()))? };
    let Some(named) = named else { return Ok(Swept::default()) };
    let now = SystemTime::now();
    // A time in the future is no age at all.
    let old = |meta: &fs::Metadata| meta.modified().ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age >= grace);
    let mut swept = Swept::default();
    let remove = |stale: &Path, meta: &fs::Metadata, swept: &mut Swept| {
        // Looked at again right before it goes: a save may just have written or renewed it.
        if fs::symlink_metadata(stale).is_ok_and(|m| !m.is_dir() && old(&m)) && fs::remove_file(stale).is_ok() {
            swept.removed += 1;
            swept.bytes += meta.len();
        }
    };
    let mut seen = 0usize;
    'folders: for sub in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let folder = sub.file_name();
        let Some(folder) = folder.to_str().filter(|n| n.len() == 2 && is_hex(n)) else { continue };
        // Only real folders are walked: a link is never followed.
        if !sub.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        for entry in fs::read_dir(sub.path()).into_iter().flatten().flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                break 'folders;
            }
            let name = entry.file_name();
            let (Some(name), Ok(meta)) = (name.to_str(), entry.metadata()) else { continue };
            if meta.is_dir() {
                continue;
            }
            let stale = match Digest::parse(name) {
                Some(hash) => !name.starts_with(folder) || !named.contains(&hash),
                None => name.get(..64).is_some_and(|stem| Digest::parse(stem).is_some() && is_temp_of(name, stem)),
            };
            if stale && old(&meta) {
                remove(&entry.path(), &meta, &mut swept);
            } else if !stale {
                swept.kept += 1;
            }
        }
    }
    // Temporary files of the mailbox file itself.
    if let (Some(parent), Some(stem)) = (file.parent(), file.file_name().and_then(|n| n.to_str())) {
        let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
        for entry in fs::read_dir(parent).into_iter().flatten().flatten().take(MAX_ENTRIES) {
            let name = entry.file_name();
            let (Some(name), Ok(meta)) = (name.to_str(), entry.metadata()) else { continue };
            if !meta.is_dir() && is_temp_of(name, stem) && old(&meta) {
                remove(&entry.path(), &meta, &mut swept);
            }
        }
    }
    Ok(swept)
}

// ---- saving in the background ----

/// Writes mailbox files on a thread of its own, so whoever hands it a state never waits for the
/// disk. A state handed in while another is being written waits its turn; a newer one handed in
/// meanwhile takes its place, so a save in flight is followed by at most one more, with the
/// newest state. Now and then it sweeps what a mailbox no longer names.
pub struct Saver {
    queue: Arc<Queue>,
}

struct Queue {
    jobs: Mutex<Jobs>,
    /// Something to do (for the thread).
    work: Condvar,
    /// Nothing left to write (for [`Saver::flush`]).
    idle: Condvar,
    /// Wakes whoever wants to hear that a state was written (the window).
    notify: Box<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Jobs {
    /// States waiting to be written: the newest one of each mailbox file.
    waiting: Vec<Job>,
    /// The file being written and the state going into it.
    writing: Option<(PathBuf, u64)>,
    sweeps: Vec<PathBuf>,
    reports: Vec<SaveReport>,
    closed: bool,
}

struct Job {
    path: PathBuf,
    mailbox: Mailbox,
    rev: u64,
    seq: u64,
}

enum Work {
    Save(Box<Job>),
    Sweep(PathBuf),
}

impl Saver {
    /// Start the thread. `notify` is called (on that thread) each time a state has been written
    /// or has failed to be: wake whatever reads [`BackgroundSave::reports`].
    pub fn spawn(notify: impl Fn() + Send + Sync + 'static) -> std::io::Result<Saver> {
        let queue = Arc::new(Queue { jobs: Mutex::default(), work: Condvar::new(), idle: Condvar::new(), notify: Box::new(notify) });
        let theirs = queue.clone();
        std::thread::Builder::new().name("mailbox saver".into()).spawn(move || theirs.run())?;
        Ok(Saver { queue })
    }

    /// Sweep the content folder of the mailbox file at `path` when there is nothing to write
    /// (after opening it, say).
    pub fn sweep_later(&self, path: &Path) {
        let mut jobs = lock(&self.queue.jobs);
        if !jobs.closed && !jobs.sweeps.iter().any(|p| p == path) {
            jobs.sweeps.push(path.to_path_buf());
            self.queue.work.notify_one();
        }
    }
}

impl BackgroundSave for Saver {
    fn follow(&self, path: &Path) {
        shared(path).followed.store(true, Ordering::Relaxed);
    }

    fn submit(&self, path: &Path, mailbox: Mailbox, rev: u64) {
        self.follow(path);
        let mut jobs = lock(&self.queue.jobs);
        // The same state again (the window asks until it hears back) is already on its way.
        if jobs.closed || jobs.writing.as_ref().is_some_and(|(p, r)| p == path && *r == rev) {
            return;
        }
        match jobs.waiting.iter_mut().find(|j| j.path == path) {
            Some(job) if job.rev == rev => return,
            Some(job) => (job.mailbox, job.rev, job.seq) = (mailbox, rev, next_seq()),
            None => jobs.waiting.push(Job { path: path.to_path_buf(), mailbox, rev, seq: next_seq() }),
        }
        self.queue.work.notify_one();
    }

    fn reports(&self) -> Vec<SaveReport> {
        std::mem::take(&mut lock(&self.queue.jobs).reports)
    }

    fn flush(&self) {
        let deadline = Instant::now() + FLUSH_WAIT;
        let mut jobs = lock(&self.queue.jobs);
        while !jobs.waiting.is_empty() || jobs.writing.is_some() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                log::warn!("gave up waiting for the mailbox to be written");
                return;
            }
            jobs = self.queue.idle.wait_timeout(jobs, left).map_or_else(|e| e.into_inner().0, |(jobs, _)| jobs);
        }
    }
}

impl Drop for Saver {
    /// The thread writes what is still waiting, then ends.
    fn drop(&mut self) {
        lock(&self.queue.jobs).closed = true;
        self.queue.work.notify_all();
    }
}

impl Queue {
    fn run(&self) {
        loop {
            let task = {
                let mut jobs = lock(&self.jobs);
                loop {
                    if !jobs.waiting.is_empty() {
                        let job = jobs.waiting.remove(0);
                        jobs.writing = Some((job.path.clone(), job.rev));
                        break Work::Save(Box::new(job));
                    }
                    self.idle.notify_all();
                    if jobs.closed {
                        return;
                    }
                    if let Some(path) = jobs.sweeps.pop() {
                        break Work::Sweep(path);
                    }
                    jobs = self.work.wait(jobs).unwrap_or_else(|e| e.into_inner());
                }
            };
            match task {
                Work::Save(job) => {
                    let Job { path, mailbox, rev, seq } = *job;
                    // Never-crash: a panic in here is a save that failed, and the thread lives on.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| save_as_of(&path, &mailbox, seq)))
                        .unwrap_or_else(|_| Err("saving the mailbox stopped unexpectedly".to_string()));
                    drop(mailbox);
                    let due = result.is_ok() && lock(&shared(&path).state).swept.is_none_or(|at| at.elapsed() >= SWEEP_EVERY);
                    let mut jobs = lock(&self.jobs);
                    jobs.writing = None;
                    if due && !jobs.sweeps.contains(&path) {
                        jobs.sweeps.push(path.clone());
                    }
                    jobs.reports.push(SaveReport { path, rev, result: result.map(|_| ()) });
                    drop(jobs);
                    (self.notify)();
                }
                Work::Sweep(path) => match std::panic::catch_unwind(|| sweep(&path, GRACE)) {
                    Ok(Ok(swept)) if swept.removed > 0 => {
                        log::info!("{}: removed {} content file(s) nothing names any more ({} bytes)", path.display(), swept.removed, swept.bytes)
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => log::warn!("sweeping the mailbox's content folder failed: {e}"),
                    Err(_) => log::error!("sweeping the mailbox's content folder stopped unexpectedly"),
                },
            }
        }
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
