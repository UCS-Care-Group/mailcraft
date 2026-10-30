// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The word index: which messages hold which words, kept beside the mailbox file so that a search
//! answers without reading every message (R6.2).
//!
//! ```text
//! mailbox.mailcraft           the mailbox (store.rs)
//! mailbox.mailcraft.blobs/    its content
//! mailbox.mailcraft.words     this: for each message, the words of its subject, the names and
//!                             addresses it is from, to and copied to, its text and the names of
//!                             its attachments, each with the parts of the message it is in
//! ```
//!
//! - **What a word is** is the search box's ([`mailcraft_model::words`]): letters and digits,
//!   case and accents folded. A search word finds the words that start with it, in the parts of
//!   the message its keyword names (`from:`) or in any of them.
//! - **Derived data.** Everything in it can be worked out again from the mailbox, so it is never
//!   repaired: a file that is damaged, too large, or of another version is thrown away and built
//!   again. Each message is known by its id and a fingerprint of what the index read of it
//!   ([`fingerprint`]): a message whose fingerprint differs is read again; one the mailbox no
//!   longer has is taken out.
//! - **The file** is a header and a log of records, each with its length and a checksum, written
//!   as the index changes (appended; [`Index::flush`]): the words not yet in it, the messages
//!   read (each a list of word numbers and the parts they are in), and the messages taken out.
//!   When what was taken out outweighs what is left, an account is removed, or mail is found
//!   gone that went while the file was closed (removed where no index is kept, as by
//!   `mailcraft-cli`), the file is written again whole without it ([`Index::compact`]), to a
//!   temporary file renamed over. A
//!   record cut short at the end (the program stopped while writing) is left out, and so is
//!   everything after it; any other damage makes the whole file go.
//! - **Private as the mail:** the file is for its owner only (as the mailbox file is), and
//!   nothing of what it holds is ever logged.
//! - **Bounded:** a message counts at most [`MAX_DOC_WORDS`] different words; the index at most
//!   [`MAX_WORDS`] words and [`MAX_SLOTS`] messages; a file is read only up to [`MAX_FILE`], a
//!   record up to [`MAX_RECORD`]. Everything read back is checked before it is believed.
//!
//! Not in the browser, which keeps the mailbox as one file and searches it as it is.

use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use mailcraft_model::{Id, Mailbox, Message, words};

/// The format of the file: another number is another version, thrown away and built again.
pub const VERSION: u32 = 1;

const MAGIC: &[u8; 8] = b"MCWORDS\0";
const HEADER_LEN: u64 = 16;

/// Most different words one message counts (the rest of a very long message is not indexed).
pub const MAX_DOC_WORDS: usize = 20_000;
/// Most different words in an index; past it, new words are not indexed.
pub const MAX_WORDS: usize = 16_000_000;
/// Most messages an index holds, live and taken out.
pub const MAX_SLOTS: usize = 20_000_000;
/// Largest file read (4 GB).
pub const MAX_FILE: u64 = 4 << 30;
/// Largest record read (64 MB).
pub const MAX_RECORD: u32 = 64 << 20;

/// A record's frame: its length, its kind, and after the payload an 8-byte checksum.
const FRAME: u64 = 4 + 1 + 8;

const WORDS: u8 = 1;
const DOC: u8 = 2;
const KILL: u8 = 3;
const SLOTS: u8 = 4;
const POSTINGS: u8 = 5;

/// Records of a whole file written again are at most about this large.
const CHUNK: usize = 1 << 20;

// ---- numbers ----

fn put_varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// A number written by [`put_varint`] at `*at`, moving past it.
fn varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut n: u64 = 0;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        n |= u64::from(byte & 0x7f).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            return Some(n);
        }
    }
    None
}

fn u64_at(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let end = at.checked_add(8)?;
    let raw: [u8; 8] = bytes.get(*at..end)?.try_into().ok()?;
    *at = end;
    Some(u64::from_le_bytes(raw))
}

fn byte_at(bytes: &[u8], at: &mut usize) -> Option<u8> {
    let b = *bytes.get(*at)?;
    *at += 1;
    Some(b)
}

/// FNV-1a, 64 bits: a checksum for records and the fingerprint of a message. The same on every
/// machine and in every version of the program (a fingerprint is kept in the file).
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(mut self, bytes: &[u8]) -> Fnv {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    /// A text and a mark after it, so that `ab` + `c` differs from `a` + `bc`.
    fn text(self, text: &str, mark: u8) -> Fnv {
        self.bytes(text.as_bytes()).bytes(&[0xff, mark])
    }
}

fn checksum(kind: u8, payload: &[u8]) -> u64 {
    Fnv::new().bytes(&[kind]).bytes(payload).0
}

// ---- what is read of a message ----

/// What the index read of a message, as one number: its subject, the names and addresses it is
/// from, to and copied to, the names of its attachments, and its text (by the SHA-256 of the
/// text, whether the text is in memory or kept on disk, so that a body brought in or let go of
/// is the same message). A message whose fingerprint changed is read again.
pub fn fingerprint(m: &Message) -> u64 {
    let mut h = Fnv::new().text(&m.subject, 1).text(&m.from.name, 2).text(&m.from.email, 3);
    for a in &m.to {
        h = h.text(&a.name, 4).text(&a.email, 5);
    }
    for a in &m.cc {
        h = h.text(&a.name, 6).text(&a.email, 7);
    }
    for a in &m.attachments {
        h = h.text(&a.name, 8);
    }
    // What the sender hid is read by the index too (not for a message that hid nothing: its
    // fingerprint is what it was).
    // (By the file it is kept in, as the text is, so that it reads the same in memory or on disk.)
    let kept_hidden = m.kept.as_deref().filter(|k| m.hidden_text.is_empty() || k.failed).and_then(|k| k.hidden);
    if let Some(r) = kept_hidden {
        h = h.text("hidden", 9).bytes(&r.sha256);
    } else if !m.hidden_text.is_empty() {
        h = h.text("hidden", 9).bytes(&sha256(m.hidden_text.as_bytes()));
    }
    let text = match m.kept.as_deref() {
        // Kept on disk (or its reading failed, and its text says so): the file it is kept in.
        Some(k) if k.text.is_some() && (m.body.text.is_empty() || k.failed) => k.text.map(|r| r.sha256),
        _ => None,
    };
    let text = text.unwrap_or_else(|| sha256(m.body.text.as_bytes()));
    h.bytes(&text).0
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let d = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut out = [0u8; 32];
    for (o, b) in out.iter_mut().zip(d.as_ref()) {
        *o = *b;
    }
    out
}

/// The text of a message's body the index reads: `None` when it is kept on disk and must be read
/// from there first.
pub fn body_in_memory(m: &Message) -> Option<&str> {
    let kept_outside = m.kept.as_deref().is_some_and(|k| k.text.is_some() && (m.body.text.is_empty() || k.failed));
    (!kept_outside).then_some(m.body.text.as_str())
}

/// The words of one message, worked out without the index at hand (so that reading many
/// messages holds nobody up): each word once, with the parts of the message it is in.
pub struct Doc {
    id: Id,
    fp: u64,
    words: Vec<(String, u8)>,
}

impl Doc {
    /// The words of `m`, whose body's text is `body` (its own, or read from disk).
    pub fn of(m: &Message, body: &str) -> Doc {
        Doc::with_fingerprint(m, body, fingerprint(m))
    }

    /// [`Doc::of`], with `m`'s [`fingerprint`] worked out already.
    pub fn with_fingerprint(m: &Message, body: &str, fp: u64) -> Doc {
        Doc::with_hidden(m, body, &m.hidden_text, fp)
    }

    /// [`Doc::with_fingerprint`], with `hidden` the words the sender hid, read from disk when
    /// they are kept there.
    pub fn with_hidden(m: &Message, body: &str, hidden: &str, fp: u64) -> Doc {
        let mut seen: HashMap<String, u8> = HashMap::new();
        words::texts_with(m, body, hidden, |field, text| {
            words::each(text, |w| {
                let full = seen.len() >= MAX_DOC_WORDS;
                match seen.get_mut(w) {
                    Some(mask) => *mask |= field,
                    None if !full => {
                        seen.insert(w.to_string(), field);
                    }
                    None => {}
                }
                false
            });
            false
        });
        let mut words: Vec<(String, u8)> = seen.into_iter().collect();
        words.sort_unstable();
        Doc { id: m.id, fp, words }
    }
}

// ---- sets of messages ----

/// A set of slots, one bit each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bits(Vec<u64>);

impl Bits {
    fn new(n: usize) -> Bits {
        Bits(vec![0; n.div_ceil(64)])
    }

    fn set(&mut self, i: u32) {
        if let Some(w) = self.0.get_mut((i / 64) as usize) {
            *w |= 1u64 << (i % 64);
        }
    }

    pub fn has(&self, i: u32) -> bool {
        self.0.get((i / 64) as usize).is_some_and(|w| w & (1u64 << (i % 64)) != 0)
    }

    fn and(&mut self, other: &Bits) {
        for (a, b) in self.0.iter_mut().zip(other.0.iter().chain(std::iter::repeat(&0))) {
            *a &= b;
        }
    }
}

// ---- the index ----

/// The messages that hold one word, in order of slot: each as the distance from the one before
/// and the parts of the message the word is in.
#[derive(Clone, Debug, Default)]
struct Postings {
    bytes: Vec<u8>,
    last: Option<u32>,
}

impl Postings {
    fn push(&mut self, slot: u32, mask: u8) -> bool {
        let gap = match self.last {
            Some(last) if slot <= last => return false,
            Some(last) => slot - last,
            None => slot,
        };
        put_varint(&mut self.bytes, u64::from(gap));
        self.bytes.push(mask);
        self.last = Some(slot);
        true
    }

    /// Each slot and its mask; stops at the first thing that doesn't read.
    fn each(&self, mut f: impl FnMut(u32, u8)) {
        let mut at = 0;
        let mut slot: Option<u32> = None;
        while at < self.bytes.len() {
            let (Some(gap), Some(mask)) = (varint(&self.bytes, &mut at), byte_at(&self.bytes, &mut at)) else { return };
            let next = match slot {
                None => u32::try_from(gap).ok(),
                Some(s) => u32::try_from(gap).ok().and_then(|g| s.checked_add(g)),
            };
            let Some(next) = next else { return };
            slot = Some(next);
            f(next, mask);
        }
    }

    /// Postings read from a file, checked: slots rising and below `slots`, masks of known parts.
    fn read(bytes: &[u8], slots: usize) -> Option<Postings> {
        let mut out = Postings::default();
        let mut at = 0;
        while at < bytes.len() {
            let gap = u32::try_from(varint(bytes, &mut at)?).ok()?;
            let mask = byte_at(bytes, &mut at)?;
            let slot = match out.last {
                None => gap,
                Some(last) if gap > 0 => last.checked_add(gap)?,
                Some(_) => return None,
            };
            if slot as usize >= slots || mask == 0 || mask & !words::ALL != 0 {
                return None;
            }
            out.last = Some(slot);
        }
        out.bytes = bytes.to_vec();
        Some(out)
    }
}

#[derive(Clone, Debug)]
struct Slot {
    id: Id,
    fp: u64,
    alive: bool,
    /// The message as it was last found to be what the index has: while the mailbox holds that
    /// same message (not a copy changed since), nothing of it need be looked at again. Never
    /// written.
    seen: Weak<Message>,
}

/// The word index of one mailbox (see the module's documentation).
#[derive(Debug, Default)]
pub struct Index {
    /// Each word, by its number.
    lookup: BTreeMap<Box<str>, u32>,
    postings: Vec<Postings>,
    slots: Vec<Slot>,
    by_id: HashMap<Id, u32>,
    dead: usize,
    /// The file it is kept in; `None`: memory only.
    file: Option<PathBuf>,
    /// How long the file is as far as it was read or written whole; anything after it is cut off
    /// before the next record goes on.
    file_len: u64,
    /// Words numbered since the file was last written to, to go in before the records that use them.
    new_words: Vec<Box<str>>,
    /// Records waiting to be written.
    pending: Vec<u8>,
    /// Writing to the file failed: the index goes on in memory only.
    broken: bool,
}

impl Index {
    /// An empty index kept in `file` (memory only for `None`); nothing is written until it changes.
    pub fn new(file: Option<PathBuf>) -> Index {
        Index { file, ..Default::default() }
    }

    /// The index kept in `file`, read and checked. A file that isn't there is an empty index; an
    /// error says why the file can't be used (damaged, another version, too large), and it is
    /// for the caller to throw it away.
    pub fn open(file: &Path) -> Result<Index, String> {
        let mut index = Index::new(Some(file.to_path_buf()));
        let f = match std::fs::File::open(file) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(index),
            Err(e) => return Err(format!("it can't be read ({e})")),
        };
        let len = f.metadata().map_err(|e| format!("it can't be read ({e})"))?.len();
        if len > MAX_FILE {
            return Err("it is larger than an index is ever written".into());
        }
        let mut r = std::io::BufReader::with_capacity(1 << 20, f);
        index.read_from(&mut r, len)?;
        Ok(index)
    }

    fn read_from(&mut self, r: &mut impl std::io::Read, len: u64) -> Result<(), String> {
        let mut head = [0u8; HEADER_LEN as usize];
        if len < HEADER_LEN || r.read_exact(&mut head).is_err() {
            // An empty file (created, then stopped before the header): nothing in it.
            return if len == 0 { Ok(()) } else { Err("it is cut short".into()) };
        }
        if head.get(..8) != Some(MAGIC.as_slice()) {
            return Err("it isn't a word index".into());
        }
        let version = head.get(8..12).and_then(|v| <[u8; 4]>::try_from(v).ok()).map_or(0, u32::from_le_bytes);
        if version != VERSION {
            return Err(format!("it is of another version ({version})"));
        }
        let mut at = HEADER_LEN;
        let mut frame = [0u8; 5];
        let mut payload = Vec::new();
        loop {
            if len - at < FRAME {
                // Nothing more, or a record cut short at its start.
                break;
            }
            r.read_exact(&mut frame).map_err(|e| format!("it can't be read ({e})"))?;
            let size = frame.get(..4).and_then(|v| <[u8; 4]>::try_from(v).ok()).map_or(0, u32::from_le_bytes);
            let kind = frame.get(4).copied().unwrap_or(0);
            if size > MAX_RECORD {
                return Err("a record is larger than any written".into());
            }
            if len - at - FRAME < u64::from(size) {
                // The last record was cut short: the program stopped while writing it.
                break;
            }
            payload.clear();
            payload.try_reserve(size as usize).map_err(|_| "not enough memory to read it".to_string())?;
            payload.resize(size as usize, 0);
            let mut sum = [0u8; 8];
            r.read_exact(&mut payload).and_then(|()| r.read_exact(&mut sum)).map_err(|e| format!("it can't be read ({e})"))?;
            if u64::from_le_bytes(sum) != checksum(kind, &payload) {
                return Err("a record is damaged".into());
            }
            self.take(kind, &payload).ok_or("a record doesn't read")?;
            at += FRAME + u64::from(size);
        }
        self.file_len = at;
        Ok(())
    }

    /// Take in one record read back from the file.
    fn take(&mut self, kind: u8, p: &[u8]) -> Option<()> {
        let mut at = 0;
        match kind {
            WORDS => {
                let n = varint(p, &mut at)?;
                for _ in 0..n {
                    let len = usize::try_from(varint(p, &mut at)?).ok()?;
                    let end = at.checked_add(len)?;
                    let word = std::str::from_utf8(p.get(at..end)?).ok()?;
                    at = end;
                    if word.is_empty() || word.chars().count() > words::MAX_WORD || self.lookup.len() >= MAX_WORDS {
                        return None;
                    }
                    let id = u32::try_from(self.postings.len()).ok()?;
                    if self.lookup.insert(word.into(), id).is_some() {
                        return None;
                    }
                    self.postings.push(Postings::default());
                }
            }
            DOC => {
                let id = u64_at(p, &mut at)?;
                let fp = u64_at(p, &mut at)?;
                let n = usize::try_from(varint(p, &mut at)?).ok()?;
                if n > MAX_DOC_WORDS || self.slots.len() >= MAX_SLOTS {
                    return None;
                }
                let slot = u32::try_from(self.slots.len()).ok()?;
                for _ in 0..n {
                    let word = usize::try_from(varint(p, &mut at)?).ok()?;
                    let mask = byte_at(p, &mut at)?;
                    if mask == 0 || mask & !words::ALL != 0 || !self.postings.get_mut(word)?.push(slot, mask) {
                        return None;
                    }
                }
                self.add_slot(id, fp);
            }
            KILL => {
                let n = varint(p, &mut at)?;
                for _ in 0..n {
                    let slot = usize::try_from(varint(p, &mut at)?).ok()?;
                    let s = self.slots.get_mut(slot)?;
                    if s.alive {
                        s.alive = false;
                        self.dead += 1;
                        if self.by_id.get(&s.id) == Some(&(slot as u32)) {
                            self.by_id.remove(&s.id);
                        }
                    }
                }
            }
            SLOTS => {
                let n = varint(p, &mut at)?;
                for _ in 0..n {
                    let (id, fp, alive) = (u64_at(p, &mut at)?, u64_at(p, &mut at)?, byte_at(p, &mut at)?);
                    if self.slots.len() >= MAX_SLOTS || alive > 1 {
                        return None;
                    }
                    let slot = self.add_slot(id, fp);
                    if alive == 0 {
                        self.kill(slot, false);
                    }
                }
            }
            POSTINGS => {
                let n = varint(p, &mut at)?;
                for _ in 0..n {
                    let word = usize::try_from(varint(p, &mut at)?).ok()?;
                    let len = usize::try_from(varint(p, &mut at)?).ok()?;
                    let end = at.checked_add(len)?;
                    let read = Postings::read(p.get(at..end)?, self.slots.len())?;
                    at = end;
                    let target = self.postings.get_mut(word)?;
                    if target.last.is_some() {
                        return None;
                    }
                    *target = read;
                }
            }
            _ => return None,
        }
        (at == p.len()).then_some(())
    }

    /// A new slot for message `id`; an earlier one for the same message is taken out.
    fn add_slot(&mut self, id: Id, fp: u64) -> u32 {
        let slot = self.slots.len() as u32;
        self.slots.push(Slot { id, fp, alive: true, seen: Weak::new() });
        if let Some(old) = self.by_id.insert(id, slot) {
            self.kill(old, false);
        }
        slot
    }

    /// Take slot `slot` out; `log`: and say so in the file.
    fn kill(&mut self, slot: u32, log: bool) {
        let Some(s) = self.slots.get_mut(slot as usize) else { return };
        if !s.alive {
            return;
        }
        s.alive = false;
        s.seen = Weak::new();
        self.dead += 1;
        if self.by_id.get(&s.id) == Some(&slot) {
            self.by_id.remove(&s.id);
        }
        if log {
            let mut p = Vec::new();
            put_varint(&mut p, 1);
            put_varint(&mut p, u64::from(slot));
            record(&mut self.pending, KILL, &p);
        }
    }

    // ---- asking ----

    /// How many messages it holds.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// How many different words it holds.
    pub fn word_count(&self) -> usize {
        self.lookup.len()
    }

    /// Whether message `id` is in it as `fp` says it is now.
    pub fn current(&self, id: Id, fp: u64) -> bool {
        self.by_id.get(&id).and_then(|s| self.slots.get(*s as usize)).is_some_and(|s| s.fp == fp)
    }

    /// Whether `m` is the very message the index was last found to hold as it is (nothing to look
    /// at again).
    pub fn seen(&self, m: &Arc<Message>) -> bool {
        self.by_id.get(&m.id).and_then(|s| self.slots.get(*s as usize)).is_some_and(|s| std::ptr::eq(s.seen.as_ptr(), Arc::as_ptr(m)))
    }

    /// `m` is what the index holds of it.
    pub fn note_seen(&mut self, m: &Arc<Message>) {
        if let Some(s) = self.by_id.get(&m.id).and_then(|s| self.slots.get_mut(*s as usize)) {
            s.seen = Arc::downgrade(m);
        }
    }

    /// The slot message `id` is in, while the index holds it.
    pub fn slot_of(&self, id: Id) -> Option<u32> {
        self.by_id.get(&id).copied()
    }

    /// The messages it holds.
    pub fn ids(&self) -> impl Iterator<Item = Id> + '_ {
        self.by_id.keys().copied()
    }

    /// The slots of the messages holding `wanted` (folded words, [`words::words`]), each at the
    /// start of a word, in one of the parts `fields` names. Several words: messages that hold
    /// every one of them, in any order (a phrase is checked on the message itself).
    pub fn matching(&self, wanted: &[String], fields: u8) -> Bits {
        let n = self.slots.len();
        let mut out: Option<Bits> = None;
        for w in wanted {
            let mut bits = Bits::new(n);
            for (word, id) in self.lookup.range::<str, _>((std::ops::Bound::Included(w.as_str()), std::ops::Bound::Unbounded)) {
                if !word.starts_with(w.as_str()) {
                    break;
                }
                if let Some(p) = self.postings.get(*id as usize) {
                    p.each(|slot, mask| {
                        if mask & fields != 0 {
                            bits.set(slot);
                        }
                    });
                }
            }
            match &mut out {
                Some(acc) => acc.and(&bits),
                None => out = Some(bits),
            }
        }
        out.unwrap_or_else(|| {
            // No words: every message.
            let mut all = Bits::new(n);
            for s in self.by_id.values() {
                all.set(*s);
            }
            all
        })
    }

    // ---- changing ----

    /// Put `doc` in, in the place of what the index held of its message; `seen`: the message it
    /// was read from.
    pub fn insert(&mut self, doc: Doc, seen: Option<&Arc<Message>>) {
        if self.slots.len() >= MAX_SLOTS {
            return;
        }
        if let Some(old) = self.by_id.get(&doc.id).copied() {
            self.kill(old, true);
        }
        let slot = self.slots.len() as u32;
        let mut p = Vec::with_capacity(16 + doc.words.len() * 4);
        p.extend_from_slice(&doc.id.to_le_bytes());
        p.extend_from_slice(&doc.fp.to_le_bytes());
        let mut entries: Vec<(u32, u8)> = Vec::with_capacity(doc.words.len());
        for (word, mask) in doc.words {
            let id = match self.lookup.get(word.as_str()) {
                Some(id) => *id,
                None if self.lookup.len() < MAX_WORDS => {
                    let id = self.postings.len() as u32;
                    let word: Box<str> = word.into();
                    self.lookup.insert(word.clone(), id);
                    self.new_words.push(word);
                    self.postings.push(Postings::default());
                    id
                }
                None => continue,
            };
            entries.push((id, mask));
        }
        put_varint(&mut p, entries.len() as u64);
        for (id, mask) in entries {
            if let Some(postings) = self.postings.get_mut(id as usize) {
                postings.push(slot, mask);
            }
            put_varint(&mut p, u64::from(id));
            p.push(mask);
        }
        record(&mut self.pending, DOC, &p);
        self.add_slot(doc.id, doc.fp);
        if let Some(m) = seen {
            self.note_seen(m);
        }
    }

    /// Take message `id` out.
    pub fn remove(&mut self, id: Id) {
        if let Some(slot) = self.by_id.get(&id).copied() {
            self.kill(slot, true);
        }
    }

    /// Whether so much was taken out that the file is worth writing again ([`Index::compact`]).
    pub fn wants_compacting(&self) -> bool {
        self.dead > 4096 && self.dead > self.by_id.len()
    }

    /// Write what changed to the file, at its end. A file that can't be written leaves the
    /// index in memory only (it is built again the next time).
    pub fn flush(&mut self) {
        if self.new_words.is_empty() && self.pending.is_empty() {
            return;
        }
        let Some(file) = self.file.clone().filter(|_| !self.broken) else {
            self.new_words.clear();
            self.pending.clear();
            return;
        };
        let mut out = Vec::with_capacity(self.pending.len() + self.new_words.len() * 12 + 32);
        for chunk in self.new_words.chunks(16_384) {
            let mut p = Vec::new();
            put_varint(&mut p, chunk.len() as u64);
            for w in chunk {
                put_varint(&mut p, w.len() as u64);
                p.extend_from_slice(w.as_bytes());
            }
            record(&mut out, WORDS, &p);
        }
        out.extend_from_slice(&self.pending);
        self.new_words.clear();
        self.pending.clear();
        match append(&file, self.file_len, &out) {
            Ok(len) => self.file_len = len,
            Err(e) => {
                log::warn!("search: the word index can't be written ({e}); it is kept in memory this time");
                self.broken = true;
            }
        }
    }

    /// Write the whole index again, without what was taken out (renumbered), to a temporary file
    /// renamed over its own: what went is gone from the disk too.
    pub fn compact(&mut self) {
        // Renumber the slots that are alive.
        let mut map: Vec<u32> = Vec::with_capacity(self.slots.len());
        let mut slots: Vec<Slot> = Vec::with_capacity(self.by_id.len());
        for s in &self.slots {
            if s.alive {
                map.push(slots.len() as u32);
                slots.push(s.clone());
            } else {
                map.push(u32::MAX);
            }
        }
        // Each word's messages, renumbered; words no message holds any more go.
        let mut lookup = BTreeMap::new();
        let mut postings = Vec::new();
        for (word, id) in &self.lookup {
            let Some(old) = self.postings.get(*id as usize) else { continue };
            let mut fresh = Postings::default();
            old.each(|slot, mask| {
                if let Some(to) = map.get(slot as usize).copied().filter(|s| *s != u32::MAX) {
                    fresh.push(to, mask);
                }
            });
            if fresh.last.is_some() {
                lookup.insert(word.clone(), postings.len() as u32);
                postings.push(fresh);
            }
        }
        self.by_id = slots.iter().enumerate().map(|(i, s)| (s.id, i as u32)).collect();
        self.slots = slots;
        self.lookup = lookup;
        self.postings = postings;
        self.dead = 0;
        self.new_words.clear();
        self.pending.clear();
        let Some(file) = self.file.clone() else { return };
        match self.write_whole(&file) {
            Ok(len) => {
                self.file_len = len;
                self.broken = false;
            }
            Err(e) => {
                log::warn!("search: the word index can't be written ({e}); it is kept in memory this time");
                self.broken = true;
            }
        }
    }

    fn write_whole(&self, file: &Path) -> std::io::Result<u64> {
        let mut out = crate::store::AtomicFile::create(file)?;
        let mut len = HEADER_LEN;
        out.write_all(&header())?;
        let mut emit = |out: &mut crate::store::AtomicFile, kind: u8, p: &[u8]| -> std::io::Result<()> {
            let mut buf = Vec::with_capacity(p.len() + FRAME as usize);
            record(&mut buf, kind, p);
            len += buf.len() as u64;
            out.write_all(&buf)
        };
        // The words in the order of their numbers.
        let mut by_number: Vec<&str> = vec![""; self.postings.len()];
        for (word, id) in &self.lookup {
            if let Some(slot) = by_number.get_mut(*id as usize) {
                *slot = word;
            }
        }
        for chunk in by_number.chunks(16_384) {
            let mut p = Vec::new();
            put_varint(&mut p, chunk.len() as u64);
            for w in chunk {
                put_varint(&mut p, w.len() as u64);
                p.extend_from_slice(w.as_bytes());
            }
            emit(&mut out, WORDS, &p)?;
        }
        for chunk in self.slots.chunks(CHUNK / 17) {
            let mut p = Vec::new();
            put_varint(&mut p, chunk.len() as u64);
            for s in chunk {
                p.extend_from_slice(&s.id.to_le_bytes());
                p.extend_from_slice(&s.fp.to_le_bytes());
                p.push(u8::from(s.alive));
            }
            emit(&mut out, SLOTS, &p)?;
        }
        let mut p = Vec::new();
        let mut n = 0u64;
        let mut body = Vec::new();
        for (id, postings) in self.postings.iter().enumerate() {
            put_varint(&mut body, id as u64);
            put_varint(&mut body, postings.bytes.len() as u64);
            body.extend_from_slice(&postings.bytes);
            n += 1;
            if body.len() >= CHUNK {
                put_varint(&mut p, n);
                p.append(&mut body);
                emit(&mut out, POSTINGS, &p)?;
                p.clear();
                n = 0;
            }
        }
        if n > 0 {
            put_varint(&mut p, n);
            p.append(&mut body);
            emit(&mut out, POSTINGS, &p)?;
        }
        out.commit()?;
        Ok(len)
    }
}

fn header() -> [u8; HEADER_LEN as usize] {
    let mut h = [0u8; HEADER_LEN as usize];
    for (o, b) in h.iter_mut().zip(MAGIC.iter().chain(&VERSION.to_le_bytes())) {
        *o = *b;
    }
    h
}

fn record(out: &mut Vec<u8>, kind: u8, payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out.extend_from_slice(&checksum(kind, payload).to_le_bytes());
}

/// Append `bytes` to `file` after its first `len` bytes (what follows them, a record cut short,
/// goes), creating it with its header when it isn't there. Returns its length now.
fn append(file: &Path, len: u64, bytes: &[u8]) -> std::io::Result<u64> {
    let (mut f, len) = if len < HEADER_LEN {
        let _ = std::fs::remove_file(file);
        let mut f = crate::store::create_private(file)?;
        f.write_all(&header())?;
        (f, HEADER_LEN)
    } else {
        let f = std::fs::OpenOptions::new().write(true).open(file)?;
        f.set_len(len)?;
        (f, len)
    };
    std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(len))?;
    f.write_all(bytes)?;
    Ok(len + bytes.len() as u64)
}

// ---- a session's word index ----

/// Messages a search reads into the index itself, rather than leave them to the background: a
/// few mails that came since, not a whole account's download.
pub const READ_NOW_MOST: usize = 2_000;

/// Messages the background reads between two looks at whether it should stop, and two writes to
/// the file.
const BATCH: usize = 256;

/// The word index of the mailbox file a session keeps its mailbox in, kept in step with the
/// mailbox ([`Words::in_step`]). Its file is read, and the messages it hasn't got are read into
/// it, on a thread of its own ([`Words::open`]): nothing the window does waits for that. Dropped,
/// it stops the thread at the next message (what was read by then is in the file, and the rest
/// is read the next time).
pub struct Words {
    mailbox: PathBuf,
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The accounts of the mailbox when it was last brought in step: one gone since takes its
    /// words off the disk at once.
    accounts: Vec<Id>,
    /// Brought in step since its file was read: mail gone before that (removed where no index
    /// is kept, as by `mailcraft-cli`) takes its words off the disk at the first step (#325).
    stepped: bool,
    /// Messages the last [`Words::in_step`] left out of the index (for the background to read).
    left: usize,
}

struct Shared {
    /// `None` until the file has been read.
    index: std::sync::Mutex<Option<Index>>,
    /// The background's progress through the messages it was given.
    done: AtomicUsize,
    total: AtomicUsize,
    /// The background is reading (its file, or messages).
    building: AtomicBool,
    /// Stop at the next message.
    cancel: AtomicBool,
    /// Counts the times the background finished: what was searched before may be answered
    /// otherwise now.
    finished: AtomicU64,
}

/// What [`Words::in_step`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InStep {
    /// Messages read into the index.
    pub read: usize,
    /// Messages taken out (gone from the mailbox, or changed and not read again yet).
    pub removed: usize,
    /// Messages the index hasn't got as they are now (being read in the background).
    pub left: usize,
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The text of `m`'s body, from memory or from the content folder beside the mailbox file.
fn text_of(m: &Message, reader: &crate::store::Reader) -> String {
    match body_in_memory(m) {
        Some(text) => text.to_string(),
        None => m.kept.as_deref().and_then(|k| k.text).and_then(|r| reader.read(&r)).map(|b| crate::store::text_of_bytes(&b)).unwrap_or_default(),
    }
}

/// The words the sender hid in `m`, from memory or from the content folder.
fn hidden_of(m: &Message, reader: &crate::store::Reader) -> String {
    match m.kept.as_deref().and_then(|k| k.hidden).filter(|_| m.hidden_text.is_empty()) {
        Some(r) => reader.read(&r).map(|b| crate::store::text_of_bytes(&b)).unwrap_or_default(),
        None => m.hidden_text.clone(),
    }
}

impl Words {
    /// The word index of the mailbox file at `mailbox`, read from beside it in the background,
    /// and then `messages` it hasn't got as they are read into it, newest first. A file that
    /// can't be used is thrown away and the index starts empty.
    pub fn open(mailbox: &Path, messages: Vec<Arc<Message>>) -> Words {
        let shared = Arc::new(Shared {
            index: std::sync::Mutex::new(None),
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(messages.len()),
            building: AtomicBool::new(true),
            cancel: AtomicBool::new(false),
            finished: AtomicU64::new(0),
        });
        // Not complete until it has been brought in step with the mailbox as it is once read.
        let mut words = Words { mailbox: mailbox.to_path_buf(), shared, thread: None, accounts: Vec::new(), stepped: false, left: 1 };
        words.build(messages, true);
        words
    }

    /// Read `messages` into the index on the background's thread (its file first, `open`).
    fn build(&mut self, messages: Vec<Arc<Message>>, open: bool) {
        let shared = self.shared.clone();
        let mailbox = self.mailbox.clone();
        shared.done.store(0, Ordering::Relaxed);
        shared.total.store(messages.len(), Ordering::Relaxed);
        shared.building.store(true, Ordering::Relaxed);
        let work = move || {
            build(&shared, &mailbox, messages, open);
            shared.building.store(false, Ordering::Relaxed);
            shared.finished.fetch_add(1, Ordering::Relaxed);
        };
        match std::thread::Builder::new().name("word index".into()).spawn(work) {
            Ok(thread) => self.thread = Some(thread),
            Err(e) => {
                // No thread to be had: the index is read here, at the next search.
                log::warn!("search: the word index can't be built in the background ({e})");
                let file = crate::store::words_file(&self.mailbox);
                let mut index = lock(&self.shared.index);
                if index.is_none() {
                    *index = Some(open_or_new(&file));
                }
                self.shared.building.store(false, Ordering::Relaxed);
            }
        }
    }

    /// The mailbox file it is the index of.
    pub fn mailbox(&self) -> &Path {
        &self.mailbox
    }

    /// The index, while nothing else changes it; `None` while its file is being read.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Option<Index>> {
        lock(&self.shared.index)
    }

    /// Whether the background is reading the file or messages.
    pub fn building(&self) -> bool {
        self.shared.building.load(Ordering::Relaxed)
    }

    /// While the background reads messages: how many of how many it has got through.
    pub fn progress(&self) -> Option<(usize, usize)> {
        self.building().then(|| (self.shared.done.load(Ordering::Relaxed), self.shared.total.load(Ordering::Relaxed)))
    }

    /// Counts the times the background finished.
    pub fn finished(&self) -> u64 {
        self.shared.finished.load(Ordering::Relaxed)
    }

    /// Whether the index holds every message of the mailbox as it was when it was last brought
    /// in step, with nothing being read.
    pub fn complete(&self) -> bool {
        !self.building() && self.left == 0 && self.lock().is_some()
    }

    /// Wait for the background to finish (tests, and the bench).
    pub fn wait(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Whether an account of the mailbox it was last brought in step with is gone from `mb`.
    pub fn account_gone(&self, mb: &Mailbox) -> bool {
        self.accounts.iter().any(|a| mb.account(*a).is_none())
    }

    /// Bring the index in step with `mb`: a message it hasn't got, or has otherwise than it is
    /// now, is read into it (its text through `read` when it is kept on disk), and one that is
    /// gone from `mb` is taken out. What a command did, an undo, mail that arrived, all come out
    /// the same: the index follows the mailbox as it is, not the way it got there. An account
    /// gone since the last time has its words taken off the disk at once ([`Index::compact`]).
    ///
    /// While the background is reading, or when more than [`READ_NOW_MOST`] messages are to be
    /// read (a whole account downloaded), they are left out of the index (taken out, if it had
    /// them otherwise) for the background, and counted in [`InStep::left`]: a search reads those
    /// itself, as it would without an index.
    pub fn in_step(&mut self, mb: &Mailbox, read: &dyn Fn(&Message) -> Option<String>) -> InStep {
        let gone_account = self.account_gone(mb);
        self.accounts = mb.accounts.iter().map(|a| a.id).collect();
        let mut out = InStep::default();
        let building = self.building();
        let mut later: Vec<Arc<Message>> = Vec::new();
        {
            let mut guard = lock(&self.shared.index);
            let Some(index) = guard.as_mut() else {
                self.left = mb.messages.len();
                out.left = self.left;
                return out;
            };
            let first = !self.stepped;
            self.stepped = true;
            let mut held = 0;
            let mut went = false;
            let mut left: Vec<(&Arc<Message>, u64)> = Vec::new();
            for m in mb.messages.values() {
                if index.seen(m) {
                    held += 1;
                    continue;
                }
                let fp = fingerprint(m);
                if index.current(m.id, fp) {
                    index.note_seen(m);
                    held += 1;
                } else {
                    left.push((m, fp));
                }
            }
            if index.len() > held {
                let gone: Vec<Id> = index.ids().filter(|id| !mb.messages.contains_key(id)).collect();
                went = !gone.is_empty();
                out.removed += gone.len();
                for id in gone {
                    index.remove(id);
                }
            }
            if building || left.len() > READ_NOW_MOST {
                for (m, _) in &left {
                    if index.slot_of(m.id).is_some() {
                        index.remove(m.id);
                        out.removed += 1;
                    }
                }
                out.left = left.len();
                if !building {
                    later = left.into_iter().map(|(m, _)| m.clone()).collect();
                }
            } else {
                for (m, fp) in left {
                    // What the sender hid, kept on disk, is read by the background beside the text.
                    if m.kept.as_deref().is_some_and(|k| k.hidden.is_some()) && m.hidden_text.is_empty() {
                        later.push(m.clone());
                        out.left += 1;
                        continue;
                    }
                    let text = match body_in_memory(m) {
                        Some(text) => std::borrow::Cow::Borrowed(text),
                        None => std::borrow::Cow::Owned(read(m).unwrap_or_default()),
                    };
                    index.insert(Doc::with_fingerprint(m, &text, fp), Some(m));
                    out.read += 1;
                }
            }
            // Mail gone while the file was closed: its words leave the disk now, as an account
            // removed here does, not only once enough has been taken out.
            if gone_account || (first && went) || index.wants_compacting() {
                index.compact();
            } else {
                index.flush();
            }
        }
        if !later.is_empty() {
            self.wait();
            self.build(later, false);
        }
        self.left = out.left;
        out
    }
}

impl Drop for Words {
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.wait();
    }
}

/// The index in `file`, or an empty one in its place when it can't be used.
fn open_or_new(file: &Path) -> Index {
    match Index::open(file) {
        Ok(index) => index,
        Err(why) => {
            log::info!("search: the mailbox's word index is built again: {why}");
            let _ = std::fs::remove_file(file);
            Index::new(Some(file.to_path_buf()))
        }
    }
}

/// The background's work: the file read (`open`), then each of `messages` the index hasn't got as
/// it is read into it, newest first, a batch at a time. Only one body is in memory at a time, and
/// the content cache is left alone.
fn build(shared: &Shared, mailbox: &Path, mut messages: Vec<Arc<Message>>, open: bool) {
    if open {
        let index = open_or_new(&crate::store::words_file(mailbox));
        *lock(&shared.index) = Some(index);
    }
    messages.sort_by_key(|m| std::cmp::Reverse((crate::list::when(m), m.id)));
    let reader = crate::store::Reader::new(mailbox);
    let mut rest = messages.into_iter();
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let batch: Vec<Arc<Message>> = rest.by_ref().take(BATCH).collect();
        if batch.is_empty() {
            break;
        }
        let n = batch.len();
        let marked: Vec<(Arc<Message>, u64)> = batch
            .into_iter()
            .map(|m| {
                let fp = fingerprint(&m);
                (m, fp)
            })
            .collect();
        // Which of them the index has as they are (a file read back), looked at in one go.
        let wanted: Vec<(Arc<Message>, u64)> = {
            let mut guard = lock(&shared.index);
            let Some(index) = guard.as_mut() else { break };
            marked
                .into_iter()
                .filter(|(m, fp)| {
                    let have = index.current(m.id, *fp);
                    if have {
                        index.note_seen(m);
                    }
                    !have
                })
                .collect()
        };
        let mut docs = Vec::with_capacity(wanted.len());
        for (m, fp) in wanted {
            if shared.cancel.load(Ordering::Relaxed) {
                break;
            }
            docs.push((Doc::with_hidden(&m, &text_of(&m, &reader), &hidden_of(&m, &reader), fp), m));
        }
        {
            let mut guard = lock(&shared.index);
            let Some(index) = guard.as_mut() else { break };
            for (doc, m) in docs {
                index.insert(doc, Some(&m));
            }
            index.flush();
        }
        shared.done.fetch_add(n, Ordering::Relaxed);
    }
    let mut guard = lock(&shared.index);
    if let Some(index) = guard.as_mut()
        && index.wants_compacting()
        && !shared.cancel.load(Ordering::Relaxed)
    {
        index.compact();
    }
}

/// Remove the word index kept beside the mailbox file at `mailbox`, as a wipe of the mailbox
/// does (#177).
pub fn remove(mailbox: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(crate::store::words_file(mailbox)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        done => done,
    }
}

#[cfg(test)]
#[path = "words_tests.rs"]
mod tests;
