// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Opening a one-file mailbox (format 1) without holding it in memory (#649).
//!
//! The one-file form is JSON with every attachment as base64 inside it, and years of mail make
//! it several gigabytes. Reading it whole and parsing it wants a few times its size in memory,
//! which many computers don't have. This reads it as a stream instead: the file goes through a
//! small buffer, one item at a time (a message, a contact, an event, a task), and as each
//! message is read the content over [`INLINE_MAX`] (its text, HTML, hidden words and
//! attachments) goes straight to the content folder beside the file and is let go of, exactly
//! as a mailbox in the form with a log of records keeps it. What is left in memory is that
//! mailbox: every message's headers and the names of its content, which is fetched when it is
//! wanted (`Session::content`).
//!
//! The file itself is only read. The first save after this writes the form with a log of
//! records, keeps the file as it was beside it (`<file>.format1`) before anything is written
//! over it, and an error part-way leaves it untouched; the content already written is only
//! more files in the content folder, which a sweep never takes while the file is not in the
//! form that names content.
//!
//! Hostile files: an item is read through a gate that stops at [`MAX_ITEM`] bytes (a longer
//! string never grows past it), nesting is bounded by the parser, the number of items is
//! capped, and nothing read is believed beyond what the mailbox model accepts.

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use serde::de::{DeserializeSeed, Error as _, MapAccess, Visitor};
use serde_json::Value;

use super::*;

/// The most bytes one item of the file may come to (a message with all its attachments as base64,
/// say), and one value of the rest of the mailbox: past it the file is refused, not read on.
pub(super) const MAX_ITEM: u64 = if cfg!(test) { 4 << 20 } else { 768 << 20 };

/// The most messages, contacts, events and tasks together a file may hold.
pub(super) const MAX_ITEMS: usize = if cfg!(test) { 60_000 } else { MAX_ENTRIES };

/// Bytes read from the file at a time.
const PIECE: usize = 1 << 16;

/// Why a read stopped for an item too long.
const TOO_LONG: &str = "an item in it is too large";

/// Where the reader is, shared with the code that reads items from it.
struct Gate {
    /// Bytes handed to the parser so far.
    taken: Cell<u64>,
    /// The parser gets no byte past this one.
    until: Cell<u64>,
}

impl Gate {
    /// A new item (or key) starts here: it may be at most [`MAX_ITEM`] bytes.
    fn open(&self) {
        self.until.set(self.taken.get().saturating_add(MAX_ITEM));
    }
}

/// The file, read in pieces, summed, counted in the open's progress, and handed to the parser
/// through a [`Gate`].
struct Feed<'a> {
    inner: Summed<Counted<'a, fs::File>>,
    buf: Box<[u8]>,
    pos: usize,
    end: usize,
    gate: Rc<Gate>,
}

impl std::io::Read for Feed<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.end {
            self.pos = 0;
            self.end = 0;
            self.end = self.inner.read(&mut self.buf)?;
            if self.end == 0 {
                return Ok(0);
            }
        }
        let n = out.len().min(self.end.saturating_sub(self.pos));
        let taken = self.gate.taken.get().saturating_add(n as u64);
        if taken > self.gate.until.get() {
            return Err(std::io::Error::other(TOO_LONG));
        }
        let (Some(from), Some(to)) = (self.buf.get(self.pos..self.pos.saturating_add(n)), out.get_mut(..n)) else {
            return Err(std::io::Error::other("the buffer is inconsistent"));
        };
        to.copy_from_slice(from);
        self.pos = self.pos.saturating_add(n);
        self.gate.taken.set(taken);
        Ok(n)
    }
}

/// What reading the file builds.
struct Build {
    dir: PathBuf,
    gate: Rc<Gate>,
    /// The file's own `version`, when it says one.
    version: u32,
    /// Everything but the four collections, as JSON, for the model to read.
    rest: serde_json::Map<String, Value>,
    messages: BTreeMap<Id, Arc<Message>>,
    contacts: BTreeMap<Id, Arc<Contact>>,
    events: BTreeMap<Id, Arc<Event>>,
    tasks: BTreeMap<Id, Arc<Task>>,
    /// Parts of content moved to the content folder.
    moved: usize,
    /// The content folder is there (made when the first content goes into it).
    made: bool,
    /// Content waiting to be written, a batch at a time.
    pending: Pending,
    /// Top-level keys seen, to refuse one that is there twice.
    seen: HashSet<String>,
    /// Bytes of the file (as JSON) that stay in memory, so far, and the most that may.
    kept: u64,
    cap: u64,
    /// Why reading stopped, when it was not the JSON.
    failure: Option<String>,
}

/// Content waiting to be written to the content folder: written (and flushed to the disk) a batch
/// at a time, on several threads, instead of one file after another.
#[derive(Default)]
struct Pending {
    items: Vec<(Ref, Vec<u8>)>,
    seen: HashSet<Ref>,
    bytes: usize,
}

/// A batch is written once it holds this many bytes, or this many files (small under test).
#[cfg(not(test))]
fn batch_limits() -> (usize, usize) {
    (64 << 20, 4096)
}

#[cfg(test)]
thread_local! {
    pub(super) static BATCH: Cell<(usize, usize)> = const { Cell::new((1 << 20, 64)) };
}

#[cfg(test)]
fn batch_limits() -> (usize, usize) {
    BATCH.get()
}

/// The most bytes (as JSON) of a file of `len` bytes that stay in memory. What stays can't be
/// more than the file holds (the headers of its mail, which the next start holds again, being
/// read from the form with a log of records), apart from what parsed values cost over their text:
/// so where memory can't be measured it is the file's length with a quarter more, and at least
/// 1 GB; where it can, a quarter of the memory not in use (at least 512 MB, at most 4 GB), and
/// never less than the file's length (up to 4 GB), so that a mailbox the log form could hold is not
/// refused by moving to it alone. 24 MB for both floors under test.
fn kept_cap(len: u64) -> u64 {
    let (floor, measured_floor) = if cfg!(test) { (24 << 20, 24 << 20) } else { (1u64 << 30, 512u64 << 20) };
    let own = len.saturating_add(len / 4);
    match available_memory() {
        None => floor.max(own),
        Some(free) => (free / 4).clamp(measured_floor, 4 << 30).max(len.min(4 << 30)),
    }
}

/// The content `r` as a file under `dir`, in place only once it is whole and flushed to the disk:
/// written to a temporary file beside it, flushed, then renamed. A file of the right length that
/// is already there is this content (nothing else is ever written under its name): only its
/// time is renewed.
fn write_blob(dir: &Path, r: &Ref, bytes: &[u8]) -> std::io::Result<()> {
    let (_, file) = blob_path(dir, &r.hash).ok_or_else(|| std::io::Error::other("no name"))?;
    let there = |file: &Path| fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_file() && m.len() == r.len);
    if there(&file) && fs::OpenOptions::new().write(true).open(&file).and_then(|f| f.set_modified(SystemTime::now())).is_ok() {
        return Ok(());
    }
    let mut tries = 0;
    let (tmp, mut out) = loop {
        let tmp = temp_beside(&file);
        match create_private(&tmp) {
            Ok(out) => break (tmp, out),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 8 => tries += 1,
            Err(e) => return Err(e),
        }
    };
    let written = out.write_all(bytes).and_then(|()| out.sync_all()).and_then(|()| {
        drop(out);
        rename_over(&tmp, &file)
    });
    match written {
        Ok(()) => Ok(()),
        // Somebody else saving the same content at the same moment is no failure.
        Err(_) if there(&file) => {
            let _ = fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

impl Build {
    fn fail<E: serde::de::Error>(&mut self, why: String) -> E {
        let error = E::custom(&why);
        self.failure.get_or_insert(why);
        error
    }

    fn charge<E: serde::de::Error>(&mut self) -> Result<(), E> {
        let held = self.messages.len() + self.contacts.len() + self.events.len() + self.tasks.len();
        if held >= MAX_ITEMS {
            return Err(self.fail(format!("it holds more than {MAX_ITEMS} items")));
        }
        Ok(())
    }

    /// `n` more bytes of the file stay in memory: refused past the total allowed.
    fn keep<E: serde::de::Error>(&mut self, n: u64) -> Result<(), E> {
        self.kept = self.kept.saturating_add(n);
        if self.kept > self.cap {
            let (kept, cap) = (self.kept >> 20, self.cap >> 20);
            return Err(self.fail(format!("it needs {kept} MB of memory to open, more than the {cap} MB this computer allows for it")));
        }
        Ok(())
    }

    /// Bytes of the file read since `since`.
    fn taken_since(&self, since: u64) -> u64 {
        self.gate.taken.get().saturating_sub(since)
    }

    /// Content to be written to the content folder; its name.
    fn queue(&mut self, bytes: Vec<u8>) -> Result<ContentRef, String> {
        let r = Ref::of(&bytes);
        self.moved = self.moved.saturating_add(1);
        if self.pending.seen.insert(r) {
            self.pending.bytes = self.pending.bytes.saturating_add(bytes.len());
            self.pending.items.push((r, bytes));
        }
        let (bytes, items) = batch_limits();
        if self.pending.bytes >= bytes || self.pending.items.len() >= items {
            self.flush()?;
        }
        Ok(r.to_model())
    }

    /// Write what is waiting: the files on several threads, each flushed to the disk before it is
    /// renamed into place, then each folder they went into flushed once.
    fn flush(&mut self) -> Result<(), String> {
        let items = std::mem::take(&mut self.pending.items);
        self.pending.seen.clear();
        self.pending.bytes = 0;
        if items.is_empty() {
            return Ok(());
        }
        if !self.made {
            own_dir(&self.dir).map_err(io_error)?;
            sync_dir_of(&self.dir);
            self.made = true;
        }
        let mut subs: BTreeSet<PathBuf> = BTreeSet::new();
        for (r, _) in &items {
            if let Some((sub, _)) = blob_path(&self.dir, &r.hash) {
                subs.insert(sub);
            }
        }
        for sub in &subs {
            own_dir(sub).map_err(io_error)?;
        }
        let dir = &self.dir;
        for done in fan_out(items.len(), |i| items.get(i).map(|(r, bytes)| write_blob(dir, r, bytes))) {
            match done.flatten() {
                Some(Ok(())) => {}
                Some(Err(e)) => return Err(io_error(e)),
                None => return Err("the content couldn't be written".into()),
            }
        }
        for sub in &subs {
            sync_dir_of(&sub.join("x"));
        }
        Ok(())
    }

    /// Move the content of `m` over [`INLINE_MAX`] to the content folder, and leave in its place
    /// what a message opened from the form with a log of records has ([`fill`]). With how many
    /// bytes of the message's JSON that no longer stay in memory.
    fn move_out(&mut self, mut m: Message) -> Result<(Message, u64), String> {
        let mut gone = 0u64;
        let mut kept = KeptBody::default();
        if m.body.text.len() > INLINE_MAX {
            let text = std::mem::take(&mut m.body.text);
            gone = gone.saturating_add(text.len() as u64);
            kept.text = Some(self.queue(text.into_bytes())?);
        }
        if m.body.html.as_ref().is_some_and(|h| h.len() > INLINE_MAX)
            && let Some(html) = m.body.html.take()
        {
            gone = gone.saturating_add(html.len() as u64);
            kept.html = Some(self.queue(html.into_bytes())?);
        }
        // (More than a hidden text can be stays where it is, as in a mailbox opened the old way.)
        if m.hidden_text.len() > INLINE_MAX && m.hidden_text.len() <= mailcraft_model::MAX_HIDDEN_TEXT {
            let hidden = std::mem::take(&mut m.hidden_text);
            gone = gone.saturating_add(hidden.len() as u64);
            kept.hidden = Some(self.queue(hidden.into_bytes())?);
        }
        // (A log of records names no more attachments of a message than this.)
        for a in m.attachments.iter_mut().take(MAX_PARTS) {
            if a.data.len() > INLINE_MAX {
                let data = std::mem::take(&mut a.data);
                gone = gone.saturating_add((data.len() as u64).div_ceil(3).saturating_mul(4));
                let bytes = Arc::try_unwrap(data).unwrap_or_else(|shared| shared.to_vec());
                a.kept = Some(self.queue(bytes)?);
            }
        }
        kept.text_hash = mailcraft_model::text_hash(&m.body.text);
        m.kept = (!kept.is_empty()).then(|| Box::new(kept));
        Ok((m, gone))
    }
}

/// Which collection a map of the file is.
#[derive(Clone, Copy)]
enum Kind {
    Messages,
    Contacts,
    Events,
    Tasks,
}

/// The map of one collection, its items taken one at a time.
struct Collection<'a> {
    build: &'a mut Build,
    kind: Kind,
}

impl<'de> DeserializeSeed<'de> for Collection<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Collection<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a map of items by number")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let Collection { build, kind } = self;
        loop {
            build.gate.open();
            let Some(id) = map.next_key::<Id>()? else { return Ok(()) };
            build.charge::<A::Error>()?;
            build.gate.open();
            let start = build.gate.taken.get();
            match kind {
                Kind::Messages => {
                    let m: Message = map.next_value()?;
                    match build.move_out(m) {
                        Ok((m, gone)) => {
                            build.keep::<A::Error>(build.taken_since(start).saturating_sub(gone))?;
                            build.messages.insert(id, Arc::new(m));
                        }
                        Err(why) => return Err(build.fail(why)),
                    };
                }
                Kind::Contacts => {
                    build.contacts.insert(id, Arc::new(map.next_value()?));
                    build.keep::<A::Error>(build.taken_since(start))?;
                }
                Kind::Events => {
                    build.events.insert(id, Arc::new(map.next_value()?));
                    build.keep::<A::Error>(build.taken_since(start))?;
                }
                Kind::Tasks => {
                    build.tasks.insert(id, Arc::new(map.next_value()?));
                    build.keep::<A::Error>(build.taken_since(start))?;
                }
            }
        }
    }
}

/// The mailbox as a map: its four collections taken item by item, everything else kept as JSON.
struct Top<'a>(&'a mut Build);

impl<'de> DeserializeSeed<'de> for Top<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for Top<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a mailbox")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let build = self.0;
        loop {
            build.gate.open();
            let Some(key) = map.next_key::<String>()? else { return Ok(()) };
            build.gate.open();
            build.keep::<A::Error>(key.len() as u64 * 2)?;
            if !build.seen.insert(key.clone()) {
                return Err(build.fail(format!("{key:?} is in it twice")));
            }
            let kind = match key.as_str() {
                "messages" => Kind::Messages,
                "contacts" => Kind::Contacts,
                "events" => Kind::Events,
                "tasks" => Kind::Tasks,
                "version" => {
                    let version: u32 = map.next_value()?;
                    build.version = version;
                    // Another form: the caller reads it the way it does.
                    if version >= SPLIT_VERSION {
                        return Err(A::Error::custom("not the one-file form"));
                    }
                    continue;
                }
                _ => {
                    let start = build.gate.taken.get();
                    let value: Value = map.next_value()?;
                    // (A value held as JSON values comes to more than its text.)
                    build.keep::<A::Error>(build.taken_since(start).saturating_mul(2))?;
                    build.rest.insert(key, value);
                    continue;
                }
            };
            map.next_value_seed(Collection { build, kind })?;
        }
    }
}

/// Open the one-file mailbox `file` (`len` bytes) as a stream, its content moved into the folder
/// beside it as it is read. `Ok(None)` when it isn't format 1 (a file with a content folder of
/// its own, format 2, which is read the other way).
pub(super) fn open(st: &mut State, file: &Path, len: u64, progress: &OpenProgress) -> Result<Option<Opened>, String> {
    let show = file.display();
    let gate = Rc::new(Gate { taken: Cell::new(0), until: Cell::new(MAX_ITEM) });
    let dir = blob_dir(file);
    let f = crate::io::regular_file(file).map_err(|e| format!("{show}: {e}"))?;
    let mut feed =
        Feed { inner: Summed::new(Counted { inner: f, progress }), buf: vec![0u8; PIECE].into_boxed_slice(), pos: 0, end: 0, gate: gate.clone() };
    let mut build = Build {
        dir: dir.clone(),
        gate,
        version: 0,
        rest: serde_json::Map::new(),
        messages: BTreeMap::new(),
        contacts: BTreeMap::new(),
        events: BTreeMap::new(),
        tasks: BTreeMap::new(),
        moved: 0,
        made: false,
        pending: Pending::default(),
        seen: HashSet::new(),
        kept: 0,
        cap: kept_cap(len),
        failure: None,
    };
    // (A byte order mark is skipped by hand: the parser takes none.)
    let mut head = [0u8; 3];
    let mut got = 0;
    while got < head.len() {
        let n = std::io::Read::read(&mut feed, head.get_mut(got..).unwrap_or(&mut [])).map_err(|e| format!("{show}: {e}"))?;
        if n == 0 {
            break;
        }
        got += n;
    }
    let skip = if head == [0xEF, 0xBB, 0xBF] { 3 } else { 0 };
    // Whatever of the first bytes was not a mark goes back in front of the rest.
    let first: Vec<u8> = head.get(skip..got).unwrap_or(&[]).to_vec();
    let parsed = {
        let mut reader = std::io::Read::chain(std::io::Cursor::new(first), &mut feed);
        let mut de = serde_json::Deserializer::from_reader(&mut reader);
        Top(&mut build).deserialize(&mut de).and_then(|()| de.end())
    };
    if progress.cancelled() {
        return Err(CANCELLED.into());
    }
    if let Err(e) = parsed {
        if build.version >= SPLIT_VERSION {
            return if build.version == SPLIT_VERSION {
                Ok(None)
            } else {
                Err(format!("{show}: this mailbox was written by a newer MailCraft (format {})", build.version))
            };
        }
        if let Some(why) = build.failure.take() {
            return Err(format!("{show}: {why}"));
        }
        if e.is_io() && e.to_string().contains(TOO_LONG) {
            return Err(format!("{show}: this isn't a mailbox MailCraft opens ({TOO_LONG}: over {} MB)", MAX_ITEM >> 20));
        }
        return Err(format!("{show}: this isn't a MailCraft mailbox ({e})"));
    }
    // The last of the content, on the disk before anything names it.
    build.flush().map_err(|e| format!("{show}: {e}"))?;
    let Build { rest, messages, contacts, events, tasks, moved, version, .. } = build;
    let rest = serde_json::to_vec(&Value::Object(rest)).map_err(|e| format!("{show}: this isn't a MailCraft mailbox ({e})"))?;
    let mut mailbox: Mailbox =
        mailcraft_model::from_json(&rest, max_json_items()).map_err(|e| format!("{show}: this isn't a MailCraft mailbox ({e})"))?;
    drop(rest);
    (mailbox.messages, mailbox.contacts, mailbox.events, mailbox.tasks) = (messages, contacts, events, tasks);
    mailbox.version = 1;
    // The newest Inbox mail has its text read once the window shows, as for a mailbox opened for
    // a window: with no body in memory, a list would show no previews.
    st.later = Some(newest_bodies(&mailbox));
    let fixed = mailbox.repair();
    for line in &fixed {
        log::warn!("mailbox repaired: {line}");
    }
    let filled = Filled { deferred: moved, ..Filled::default() };
    report(&show, &dir, &filled);
    log::info!("{show}: read as a stream, {moved} bodies and attachments moved to its content folder");
    let Summed { sum, len: read, .. } = feed.inner;
    st.stamp = Stamp::of(file, read, Some(sum));
    // Not "unchanged" however little is done to it: the first save writes the form with a log of
    // records, so that this file isn't read through again at every start.
    st.loaded = None;
    st.legacy = Some(version.max(1));
    // (The file is kept as it was before it is written over, or nothing is written.)
    st.streamed = true;
    Ok(Some(Opened {
        mailbox,
        split: true,
        unavailable: 0,
        loaded: 0,
        deferred: filled.deferred,
        unfinished: 0,
        lost_saves: 0,
        lost_records: 0,
        bytes: len,
        repaired: fixed.len(),
        // (The window then asks for the bodies left for after it shows, `bodies_later`.)
        unchecked: true,
    }))
}

/// The bodies of the Inbox's newest [`OPENING_ROWS`] messages that were left on disk, within the
/// budget for bodies, for [`bodies_later`] to read.
fn newest_bodies(mailbox: &Mailbox) -> Vec<crate::session::BodyLater> {
    let inbox = mailbox.inbox();
    let mut listed: Vec<(i64, Id)> = mailbox.messages.iter().filter(|(_, m)| Some(m.folder) == inbox).map(|(id, m)| (m.date, *id)).collect();
    listed.sort_unstable_by(|a, b| b.cmp(a));
    let mut budget = body_budget();
    let mut later = Vec::new();
    for (_, id) in listed.into_iter().take(OPENING_ROWS) {
        let Some(kept) = mailbox.messages.get(&id).and_then(|m| m.kept.as_deref()) else { continue };
        let len = kept.len();
        if len > budget {
            continue;
        }
        budget -= len;
        later.push(crate::session::BodyLater { id, text: kept.text, html: kept.html, hidden: kept.hidden });
    }
    later
}
