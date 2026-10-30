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
use std::rc::Rc;

use serde::de::{DeserializeSeed, Error as _, MapAccess, Visitor};
use serde_json::Value;

use super::*;

/// The most bytes one item of the file may come to (a message with all its attachments as base64,
/// say), and one value of the rest of the mailbox: past it the file is refused, not read on.
pub(super) const MAX_ITEM: u64 = if cfg!(test) { 4 << 20 } else { 768 << 20 };

/// The most messages, contacts, events and tasks together a file may hold.
pub(super) const MAX_ITEMS: usize = if cfg!(test) { 5_000 } else { MAX_ENTRIES };

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
    /// Why reading stopped, when it was not the JSON.
    failure: Option<String>,
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

    /// Move the content of `m` over [`INLINE_MAX`] to the content folder, and leave in its place
    /// what a message opened from the form with a log of records has ([`fill`]).
    fn move_out(&mut self, mut m: Message) -> Result<Message, String> {
        let dir = self.dir.clone();
        let (mut moved, mut made) = (0usize, self.made);
        let mut outside = |bytes: &[u8]| -> Result<Option<ContentRef>, String> {
            if bytes.len() <= INLINE_MAX {
                return Ok(None);
            }
            if !made {
                own_dir(&dir).map_err(io_error)?;
                made = true;
            }
            let r = Ref::of(bytes);
            put(&dir, &r, bytes).map_err(io_error)?;
            moved += 1;
            Ok(Some(r.to_model()))
        };
        let mut kept = KeptBody::default();
        if let Some(r) = outside(m.body.text.as_bytes())? {
            kept.text = Some(r);
            m.body.text = String::new();
        }
        if let Some(r) = m.body.html.as_deref().map(|h| outside(h.as_bytes())).transpose()?.flatten() {
            kept.html = Some(r);
            m.body.html = None;
        }
        // (More than a hidden text can be stays where it is, as in a mailbox opened the old way.)
        if m.hidden_text.len() <= mailcraft_model::MAX_HIDDEN_TEXT
            && let Some(r) = outside(m.hidden_text.as_bytes())?
        {
            kept.hidden = Some(r);
            m.hidden_text = String::new();
        }
        // (A log of records names no more attachments of a message than this.)
        for a in m.attachments.iter_mut().take(MAX_PARTS) {
            if let Some(r) = outside(a.data.as_slice())? {
                a.kept = Some(r);
                a.data = Arc::default();
            }
        }
        kept.text_hash = mailcraft_model::text_hash(&m.body.text);
        m.kept = (!kept.is_empty()).then(|| Box::new(kept));
        self.moved = self.moved.saturating_add(moved);
        self.made = made;
        Ok(m)
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
            match kind {
                Kind::Messages => {
                    let m: Message = map.next_value()?;
                    match build.move_out(m) {
                        Ok(m) => build.messages.insert(id, Arc::new(m)),
                        Err(why) => return Err(build.fail(why)),
                    };
                }
                Kind::Contacts => {
                    build.contacts.insert(id, Arc::new(map.next_value()?));
                }
                Kind::Events => {
                    build.events.insert(id, Arc::new(map.next_value()?));
                }
                Kind::Tasks => {
                    build.tasks.insert(id, Arc::new(map.next_value()?));
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
                    let value: Value = map.next_value()?;
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
    let Build { rest, messages, contacts, events, tasks, moved, version, .. } = build;
    let rest = serde_json::to_vec(&Value::Object(rest)).map_err(|e| format!("{show}: this isn't a MailCraft mailbox ({e})"))?;
    let mut mailbox: Mailbox =
        mailcraft_model::from_json(&rest, max_json_items()).map_err(|e| format!("{show}: this isn't a MailCraft mailbox ({e})"))?;
    drop(rest);
    (mailbox.messages, mailbox.contacts, mailbox.events, mailbox.tasks) = (messages, contacts, events, tasks);
    mailbox.version = 1;
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
        unchecked: false,
    }))
}
