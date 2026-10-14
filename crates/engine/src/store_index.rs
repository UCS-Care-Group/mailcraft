//! The mailbox file in format 3: a log of records, so that a save appends what changed instead of
//! writing everything again.
//!
//! ```text
//! header   16 bytes  "MailCraft index\n" (crate::io::INDEX_MAGIC)
//!           4 bytes  the format number, little-endian: 3
//!           4 bytes  0
//! record    4 bytes  n, the length of the payload, little-endian (at most MAX_RECORD)
//!           1 byte   what kind of record it is
//!           n bytes  the payload
//!          16 bytes  the first 16 bytes of the SHA-256 of the length, the kind and the payload
//! ```
//!
//! | kind | payload |
//! |---|---|
//! | 1 rest | the mailbox without its messages, contacts, events and tasks (JSON, as the model writes it) |
//! | 2 put | 1 byte collection (0 message, 1 contact, 2 event, 3 task), 8 bytes id, 4 bytes `j`, `j` bytes of the item as JSON, then (messages only) the names of the content it keeps outside, as format 2 has them under `blobs` (JSON; nothing when it keeps nothing outside) |
//! | 3 delete | 1 byte collection, 8 bytes id |
//! | 4 commit | 4 bytes: how many records came since the last commit |
//!
//! Records count only once a commit closes them: a save appends its records and a commit in one
//! write, flushed to the disk, so a save cut short by a crash leaves records without a commit,
//! which are not read. The first commit closes the whole mailbox (a rest record and every item);
//! everything after it is a change. When the changes outgrow what they change, the whole file is
//! written again with just the mailbox as it is ("compacted"), to a temporary file renamed over.
//!
//! Everything read is hostile: lengths are checked against the file and capped, a record whose
//! checksum doesn't match ends the reading, and what follows the last commit (a crash's half
//! save, or damage) is reported, never taken.

use std::collections::HashMap;
use std::io::{Read, Write};

use mailcraft_model::Id;

pub use crate::io::INDEX_MAGIC as MAGIC;

/// This form's format number.
pub const VERSION: u32 = 3;

/// Bytes before the first record.
pub const HEADER_LEN: u64 = 24;

/// Largest payload of one record.
pub const MAX_RECORD: u32 = 512 << 20;

/// What a record adds to its payload: its length, its kind, its checksum.
pub const FRAME: u64 = 4 + 1 + 16;

/// What a put record adds to an item's JSON and content names.
pub const PUT_HEAD: u64 = 1 + 8 + 4;

const REST: u8 = 1;
const PUT: u8 = 2;
const DELETE: u8 = 3;
const COMMIT: u8 = 4;

/// Which of the mailbox's collections an item is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Coll {
    Message,
    Contact,
    Event,
    Task,
}

impl Coll {
    fn byte(self) -> u8 {
        match self {
            Coll::Message => 0,
            Coll::Contact => 1,
            Coll::Event => 2,
            Coll::Task => 3,
        }
    }

    fn from_byte(b: u8) -> Option<Coll> {
        match b {
            0 => Some(Coll::Message),
            1 => Some(Coll::Contact),
            2 => Some(Coll::Event),
            3 => Some(Coll::Task),
            _ => None,
        }
    }
}

/// An item of the mailbox: which collection, which id.
pub type Key = (Coll, Id);

/// A record's checksum: the first 16 bytes of the SHA-256 of its length, kind and payload. It is
/// also what is remembered of what a record says about an item (or about the rest of the
/// mailbox), to know without keeping it whether it says it still.
pub type Fp = [u8; 16];

pub fn header() -> [u8; 24] {
    let mut out = [0u8; 24];
    for (o, b) in out.iter_mut().zip(MAGIC.iter().chain(&VERSION.to_le_bytes())) {
        *o = *b;
    }
    out
}

/// The length field and the checksum of a record whose payload is `parts` one after another.
fn frame(kind: u8, parts: &[&[u8]]) -> std::io::Result<([u8; 4], Fp)> {
    let n: usize = parts.iter().map(|p| p.len()).sum();
    let len = u32::try_from(n).ok().filter(|n| *n <= MAX_RECORD).ok_or_else(|| std::io::Error::other("an item is too large to be saved"))?;
    let len = len.to_le_bytes();
    Ok((len, checksum(len, kind, parts)))
}

fn checksum(len: [u8; 4], kind: u8, payload: &[&[u8]]) -> Fp {
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    ctx.update(&len);
    ctx.update(&[kind]);
    for p in payload {
        ctx.update(p);
    }
    let mut out = [0u8; 16];
    for (o, b) in out.iter_mut().zip(ctx.finish().as_ref()) {
        *o = *b;
    }
    out
}

/// Write one record whose payload is `parts` one after another. Returns its bytes on disk and
/// its checksum.
fn record(w: &mut impl Write, kind: u8, parts: &[&[u8]]) -> std::io::Result<(u64, Fp)> {
    let (len, sum) = frame(kind, parts)?;
    w.write_all(&len)?;
    w.write_all(&[kind])?;
    let mut n = 0u64;
    for p in parts {
        w.write_all(p)?;
        n += p.len() as u64;
    }
    w.write_all(&sum)?;
    Ok((FRAME + n, sum))
}

pub fn rest(w: &mut impl Write, json: &[u8]) -> std::io::Result<(u64, Fp)> {
    record(w, REST, &[json])
}

/// The checksum the rest record of `json` has.
pub fn rest_fp(json: &[u8]) -> std::io::Result<Fp> {
    frame(REST, &[json]).map(|(_, sum)| sum)
}

/// The payload of a put record, in parts.
fn put_parts<'a>(key: &'a [u8; 9], j: &'a [u8; 4], json: &'a [u8], refs: &'a [u8]) -> [&'a [u8]; 4] {
    [key, j, json, refs]
}

fn put_head(key: Key, json: &[u8]) -> std::io::Result<([u8; 9], [u8; 4])> {
    let j = u32::try_from(json.len()).map_err(|_| std::io::Error::other("an item is too large to be saved"))?;
    let mut k = [0u8; 9];
    for (o, b) in k.iter_mut().zip(std::iter::once(key.0.byte()).chain(key.1.to_le_bytes())) {
        *o = b;
    }
    Ok((k, j.to_le_bytes()))
}

pub fn put(w: &mut impl Write, key: Key, json: &[u8], refs: &[u8]) -> std::io::Result<(u64, Fp)> {
    let (k, j) = put_head(key, json)?;
    record(w, PUT, &put_parts(&k, &j, json, refs))
}

/// The checksum the put record of this item has.
pub fn put_fp(key: Key, json: &[u8], refs: &[u8]) -> std::io::Result<Fp> {
    let (k, j) = put_head(key, json)?;
    frame(PUT, &put_parts(&k, &j, json, refs)).map(|(_, sum)| sum)
}

pub fn delete(w: &mut impl Write, key: Key) -> std::io::Result<u64> {
    record(w, DELETE, &[&[key.0.byte()], &key.1.to_le_bytes()]).map(|(size, _)| size)
}

pub fn commit(w: &mut impl Write, records: u32) -> std::io::Result<u64> {
    record(w, COMMIT, &[&records.to_le_bytes()]).map(|(size, _)| size)
}

// ---- reading ----

/// An item as the file has it now.
pub struct Item {
    payload: Vec<u8>,
    /// Where its JSON ends in `payload` (and its content names begin).
    json_end: usize,
    /// Its record's checksum.
    pub fp: Fp,
}

impl Item {
    pub fn json(&self) -> &[u8] {
        self.payload.get(PUT_HEAD as usize..self.json_end).unwrap_or(&[])
    }

    pub fn refs(&self) -> &[u8] {
        self.payload.get(self.json_end..).unwrap_or(&[])
    }

    /// Its record's bytes on disk.
    pub fn size(&self) -> u64 {
        FRAME + self.payload.len() as u64
    }
}

/// What follows the last commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tail {
    /// Nothing: the file ends where its last save did.
    Clean,
    /// Only zeros (a crash on a file system that had made room for a write it never did).
    Zeros,
    /// This many bytes that aren't a finished save: one cut short, or damage. Never thrown away
    /// unseen: the file is kept aside before it is written again.
    Bytes(u64),
}

/// A file read to its last commit.
pub struct Replayed {
    /// The rest of the mailbox (JSON).
    pub rest: Option<(Vec<u8>, Fp)>,
    pub items: HashMap<Key, Item>,
    /// Where the last commit ends.
    pub good_len: u64,
    pub tail: Tail,
}

enum Op {
    Rest(Vec<u8>, Fp),
    Put(Key, Item),
    Delete(Key),
}

enum Rec {
    Op(Op),
    /// Closes this many records.
    Commit(u64),
}

/// Why a record couldn't be read.
enum Stop {
    /// The file ended inside it.
    Short,
    /// It isn't a record. `zero`: every byte of it read so far was 0.
    Bad { zero: bool },
}

/// Read into `buf` until it is full or the input ends. Returns the bytes read.
pub fn fill(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while let Some(rest) = buf.get_mut(got..).filter(|b| !b.is_empty()) {
        match r.read(rest) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

/// Whether everything left to read is 0.
fn zeros(r: &mut impl Read) -> std::io::Result<bool> {
    let mut buf = vec![0u8; 64 << 10];
    loop {
        let n = fill(r, &mut buf)?;
        if !buf.get(..n).unwrap_or(&[]).iter().all(|b| *b == 0) {
            return Ok(false);
        }
        if n < buf.len() {
            return Ok(true);
        }
    }
}

fn id_of(bytes: &[u8]) -> Option<Id> {
    Some(Id::from_le_bytes(bytes.get(..8)?.try_into().ok()?))
}

fn key_of(payload: &[u8]) -> Option<Key> {
    Some((Coll::from_byte(*payload.first()?)?, id_of(payload.get(1..)?)?))
}

/// The next record, `left` bytes before the end of the file.
fn next(r: &mut impl Read, left: u64) -> std::io::Result<Result<(Rec, u64), Stop>> {
    let mut head = [0u8; 5];
    let got = fill(r, &mut head)?;
    if got < head.len() {
        return Ok(Err(Stop::Short));
    }
    let [a, b, c, d, kind] = head;
    let len = u32::from_le_bytes([a, b, c, d]);
    let zero = head == [0; 5];
    if len > MAX_RECORD || !matches!(kind, REST | PUT | DELETE | COMMIT) {
        return Ok(Err(Stop::Bad { zero }));
    }
    let size = FRAME + u64::from(len);
    if size > left {
        return Ok(Err(Stop::Short));
    }
    let n = len as usize;
    let mut payload = Vec::new();
    // Memory that isn't there is a file that can't be opened, not a crash.
    if payload.try_reserve_exact(n).is_err() {
        return Err(std::io::Error::other("not enough memory to open the mailbox"));
    }
    payload.resize(n, 0);
    let mut sum = [0u8; 16];
    if fill(r, &mut payload)? < n || fill(r, &mut sum)? < sum.len() {
        return Ok(Err(Stop::Short));
    }
    let zero = zero && payload.iter().all(|b| *b == 0) && sum == [0; 16];
    if sum != checksum([a, b, c, d], kind, &[&payload]) {
        return Ok(Err(Stop::Bad { zero }));
    }
    let op = match kind {
        REST => Op::Rest(payload, sum),
        PUT => {
            let json_len = payload.get(9..13).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
            let json_end = json_len.and_then(|j| (j as usize).checked_add(PUT_HEAD as usize)).filter(|end| *end <= payload.len());
            match (key_of(&payload), json_end) {
                (Some(key), Some(json_end)) => Op::Put(key, Item { payload, json_end, fp: sum }),
                _ => return Ok(Err(Stop::Bad { zero })),
            }
        }
        DELETE => match key_of(&payload).filter(|_| payload.len() == 9) {
            Some(key) => Op::Delete(key),
            None => return Ok(Err(Stop::Bad { zero })),
        },
        _ => match payload.get(..4).and_then(|b| b.try_into().ok()).filter(|_| payload.len() == 4) {
            Some(count) => return Ok(Ok((Rec::Commit(u64::from(u32::from_le_bytes(count))), size))),
            None => return Ok(Err(Stop::Bad { zero })),
        },
    };
    Ok(Ok((Rec::Op(op), size)))
}

/// Read a format-3 file of `file_len` bytes up to its last commit. An error is a file that
/// isn't one, is from a newer program, or couldn't be read at all; damage after the first commit
/// is a [`Tail`], not an error.
pub fn replay(mut r: impl Read, file_len: u64) -> Result<Replayed, String> {
    let mut head = [0u8; HEADER_LEN as usize];
    if fill(&mut r, &mut head).map_err(|e| e.to_string())? < head.len() || head.get(..MAGIC.len()) != Some(&MAGIC[..]) {
        return Err("this isn't a MailCraft mailbox".into());
    }
    let version = head.get(16..20).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes);
    if version > VERSION {
        return Err(format!("this mailbox was written by a newer MailCraft (format {version})"));
    }
    if version != VERSION || head.get(20..24) != Some(&[0u8; 4][..]) {
        return Err("this mailbox file is damaged (its header)".into());
    }
    let mut out = Replayed { rest: None, items: HashMap::new(), good_len: HEADER_LEN, tail: Tail::Clean };
    let mut at = HEADER_LEN;
    let mut pending: Vec<Op> = Vec::new();
    let mut r = r.take(file_len.saturating_sub(HEADER_LEN));
    loop {
        let left = file_len.saturating_sub(at);
        if left == 0 {
            if !pending.is_empty() {
                out.tail = Tail::Bytes(file_len.saturating_sub(out.good_len));
            }
            break;
        }
        match next(&mut r, left).map_err(|e| e.to_string())? {
            Ok((Rec::Commit(count), size)) => {
                at = at.saturating_add(size);
                if count != pending.len() as u64 {
                    // A commit that doesn't close what came before it isn't one.
                    out.tail = Tail::Bytes(file_len.saturating_sub(out.good_len));
                    break;
                }
                for op in pending.drain(..) {
                    match op {
                        Op::Rest(json, fp) => out.rest = Some((json, fp)),
                        Op::Put(key, item) => {
                            out.items.insert(key, item);
                        }
                        Op::Delete(key) => {
                            out.items.remove(&key);
                        }
                    }
                }
                out.good_len = at;
            }
            Ok((Rec::Op(op), size)) => {
                at = at.saturating_add(size);
                pending.push(op);
            }
            Err(stop) => {
                let zero = match stop {
                    Stop::Short => false,
                    Stop::Bad { zero } => zero,
                };
                // Records closed by no commit count for nothing either way; zeros from there to
                // the end are no save at all.
                out.tail = if pending.is_empty() && zero && zeros(&mut r).map_err(|e| e.to_string())? {
                    Tail::Zeros
                } else {
                    Tail::Bytes(file_len.saturating_sub(out.good_len))
                };
                break;
            }
        }
    }
    if out.good_len == HEADER_LEN {
        return Err("this mailbox file is damaged (no save in it was finished)".into());
    }
    Ok(out)
}
