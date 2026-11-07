// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
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
//! Everything read is hostile: lengths are checked against the file and capped, and what follows
//! the last commit (a crash's half save, or damage) is reported, never taken. A record whose
//! checksum doesn't match in a later save ends the reading there: every save after the first is
//! a change to the one before it, and a change left out could leave a later one saying what
//! isn't so (a folder's mark of how far its mail was fetched, written only when it moves, says
//! mail is here that the damaged save brought). The saves from there on are counted and reported,
//! and the file is kept aside before it is written again. In the first save, which nothing else
//! stands in for, a damaged record whose length still leads to the next one costs only its item:
//! the rest of the mailbox is read, and the items lost are reported (the host then fetches the
//! mail again: `crate::store`).
//!
//! A file that begins `MailCraft ` but not as this form does, or has a higher format number, was
//! written by a newer MailCraft and is said to be.

use std::collections::HashMap;
use std::io::{Read, Write};

use mailcraft_model::Id;
use mailcraft_model::load_limit::{self, Budget};

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
    /// What it is charged while it is held ([`replay_within`]): its bytes and [`RECORD_COST`].
    pub fn cost(&self) -> u64 {
        (self.payload.len() as u64).saturating_add(RECORD_COST)
    }

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
    /// Finished saves left out: the first one with a damaged record, and every one after it
    /// (their bytes are in `tail`, which is then [`Tail::Bytes`]: the file is kept aside before
    /// it is written again).
    pub lost_saves: u32,
    /// Damaged records of the first save (the whole mailbox) left out: items it is missing.
    /// Then `tail` is [`Tail::Bytes`] too.
    pub lost_records: u32,
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
    /// It isn't a record. `zero`: every byte of it read so far was 0. `framed`: its length was
    /// believable and all of it was read (this many bytes): the next record may follow. `kind`:
    /// what its frame says it is, and how long its payload is, when it is framed.
    Bad { zero: bool, framed: Option<u64>, kind: Option<(u8, u32)> },
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
        return Ok(Err(Stop::Bad { zero, framed: None, kind: None }));
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
    let framed = Some(size);
    if sum != checksum([a, b, c, d], kind, &[&payload]) {
        return Ok(Err(Stop::Bad { zero, framed, kind: Some((kind, len)) }));
    }
    let op = match kind {
        REST => Op::Rest(payload, sum),
        PUT => {
            let json_len = payload.get(9..13).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
            let json_end = json_len.and_then(|j| (j as usize).checked_add(PUT_HEAD as usize)).filter(|end| *end <= payload.len());
            match (key_of(&payload), json_end) {
                (Some(key), Some(json_end)) => Op::Put(key, Item { payload, json_end, fp: sum }),
                _ => return Ok(Err(Stop::Bad { zero, framed, kind: Some((kind, len)) })),
            }
        }
        DELETE => match key_of(&payload).filter(|_| payload.len() == 9) {
            Some(key) => Op::Delete(key),
            None => return Ok(Err(Stop::Bad { zero, framed, kind: Some((kind, len)) })),
        },
        _ => match payload.get(..4).and_then(|b| b.try_into().ok()).filter(|_| payload.len() == 4) {
            Some(count) => return Ok(Ok((Rec::Commit(u64::from(u32::from_le_bytes(count))), size))),
            None => return Ok(Err(Stop::Bad { zero, framed, kind: Some((kind, len)) })),
        },
    };
    Ok(Ok((Rec::Op(op), size)))
}

/// Most records a file is read with: one save's, and the items the mailbox has (a mailbox of
/// four million messages is well inside it). A file with more is refused, not read until memory
/// runs out.
pub const MAX_RECORDS: usize = 20_000_000;

/// Whether the bytes of a file's beginning say it was written by a MailCraft, in any form that
/// isn't JSON: this one, or one a newer MailCraft writes, which begins the same way.
pub fn is_ours(head: &[u8]) -> bool {
    head.starts_with(crate::io::FAMILY)
}

/// Read a format-3 file of `file_len` bytes up to its last commit. An error is a file that
/// isn't one, is from a newer program, or couldn't be read at all; damage after the first commit
/// is a [`Tail`], not an error.
#[cfg(test)]
pub fn replay(r: impl Read, file_len: u64) -> Result<Replayed, String> {
    replay_within(r, file_len, MAX_RECORDS, None)
}

/// [`replay`], with at most `max_records` records held at once.
#[cfg(test)]
pub fn replay_capped(r: impl Read, file_len: u64, max_records: usize) -> Result<Replayed, String> {
    replay_within(r, file_len, max_records, None)
}

/// What a record held in memory costs over its bytes, at the least: its place in the list of a
/// save and then in the map of items (the key, the item, the room a growing map keeps), and the
/// allocation of its bytes.
pub const RECORD_COST: u64 = 160;

/// What `op` is charged while it is held.
fn cost(op: &Op) -> u64 {
    match op {
        Op::Rest(json, _) => (json.len() as u64).saturating_add(RECORD_COST),
        Op::Put(_, item) => item.cost(),
        Op::Delete(_) => RECORD_COST,
    }
}

/// Read a format-3 file (as `replay` does), at most `max_records` records held at once and every
/// record held charged to `budget` (its bytes and [`RECORD_COST`]) as it
/// is read, and given back when a later save replaces or removes it: a file of many small records
/// is refused before it costs more memory than the budget, not read until memory runs out.
pub fn replay_within(mut r: impl Read, file_len: u64, max_records: usize, budget: Option<&Budget>) -> Result<Replayed, String> {
    let mut head = [0u8; HEADER_LEN as usize];
    let got = fill(&mut r, &mut head).map_err(|e| e.to_string())?;
    let head = head.get(..got).unwrap_or(&[]);
    if !head.starts_with(&MAGIC[..]) {
        // Another form of ours (a newer program's) is said to be that, not something else.
        return Err(if is_ours(head) { "this mailbox was written by a newer MailCraft".into() } else { "this isn't a MailCraft mailbox".into() });
    }
    let version = head.get(16..20).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    if let Some(version) = version.filter(|v| *v > VERSION) {
        return Err(format!("this mailbox was written by a newer MailCraft (format {version})"));
    }
    if version != Some(VERSION) || head.get(20..24) != Some(&[0u8; 4][..]) {
        return Err("this mailbox file is damaged (its header)".into());
    }
    let too_many = || TOO_MANY.to_string();
    let no_memory = || NO_MEMORY.to_string();
    let mut out = Replayed { rest: None, items: HashMap::new(), good_len: HEADER_LEN, tail: Tail::Clean, lost_saves: 0, lost_records: 0 };
    let mut at = HEADER_LEN;
    let mut pending: Vec<Op> = Vec::new();
    // Damaged records of the first save, and their bytes: the rest of it is kept (salvaged). Of
    // them, the items lost (puts: a damaged commit or rest record is no item).
    let (mut salvaged, mut salvaged_bytes, mut lost_items) = (0u64, 0u64, 0u64);
    // A record of the first save isn't one (as against: the file ends inside it).
    let mut base_bad = false;
    // The first save was read with damage in it (its commit): the file is kept aside.
    let mut base_damaged = false;
    let mut r = r.take(file_len.saturating_sub(HEADER_LEN));
    loop {
        let left = file_len.saturating_sub(at);
        if left == 0 {
            if !pending.is_empty() || salvaged > 0 {
                out.tail = Tail::Bytes(file_len.saturating_sub(out.good_len));
            }
            break;
        }
        match next(&mut r, left).map_err(|e| e.to_string())? {
            Ok((Rec::Commit(count), size)) => {
                at = at.saturating_add(size);
                let seen = (pending.len() as u64).saturating_add(salvaged);
                // A commit that doesn't close what came before it (damaged records of the first
                // save included) isn't one.
                if count != seen {
                    out.tail = Tail::Bytes(file_len.saturating_sub(out.good_len));
                    break;
                }
                take_in(&mut out, &mut pending, max_records, budget)?;
                out.good_len = at;
                out.lost_records = out.lost_records.saturating_add(u32::try_from(lost_items).unwrap_or(u32::MAX));
                (salvaged, lost_items) = (0, 0);
            }
            Ok((Rec::Op(op), size)) => {
                at = at.saturating_add(size);
                if pending.len() >= max_records {
                    return Err(too_many());
                }
                if budget.is_some_and(|b| !b.take(cost(&op))) {
                    return Err(load_limit::TOO_MUCH.into());
                }
                pending.try_reserve(1).map_err(|_| no_memory())?;
                pending.push(op);
            }
            // The first save's own commit damaged (a commit's frame: no other record is 4
            // bytes long): the file was renamed into place only once all of it was written, so
            // the first save is what came before it, every record of it checked by its own
            // checksum. Nothing of the mailbox is lost, and what follows is read as later saves;
            // the file is kept aside before it is written again.
            Err(Stop::Bad { framed: Some(size), kind: Some((_, 4)), .. })
                if out.good_len == HEADER_LEN && pending.iter().any(|op| matches!(op, Op::Rest(..))) =>
            {
                at = at.saturating_add(size);
                salvaged_bytes = salvaged_bytes.saturating_add(size);
                take_in(&mut out, &mut pending, max_records, budget)?;
                out.good_len = at;
                out.lost_records = u32::try_from(lost_items).unwrap_or(u32::MAX);
                (salvaged, lost_items) = (0, 0);
                base_damaged = true;
            }
            // In the first save (the whole mailbox, which nothing else stands in for), a damaged
            // record whose length leads on costs that item, not the mailbox.
            Err(Stop::Bad { framed: Some(size), kind, .. }) if out.good_len == HEADER_LEN => {
                at = at.saturating_add(size);
                salvaged = salvaged.saturating_add(1);
                salvaged_bytes = salvaged_bytes.saturating_add(size);
                if matches!(kind, Some((PUT, _))) {
                    lost_items = lost_items.saturating_add(1);
                }
            }
            // After it, every save is a change to the one before: a save that can't be read is
            // where the reading stops, for what the saves after it changed may count on it (a
            // folder's mark of how far its mail was fetched, which comes back only in full). They
            // are counted, not read, and the file is kept aside before it is written again.
            Err(Stop::Bad { framed: Some(_), .. }) => {
                let left = file_len.saturating_sub(at);
                out.lost_saves = count_lost(&mut r, left, (pending.len() as u64).saturating_add(1)).map_err(|e| e.to_string())?;
                out.tail = Tail::Bytes(file_len.saturating_sub(out.good_len));
                break;
            }
            Err(stop) => {
                let zero = match stop {
                    Stop::Short => false,
                    Stop::Bad { zero, .. } => zero,
                };
                base_bad = matches!(stop, Stop::Bad { .. }) && out.good_len == HEADER_LEN;
                // Records closed by no commit count for nothing either way; zeros from there to
                // the end are no save at all.
                out.tail = if pending.is_empty() && salvaged == 0 && zero && zeros(&mut r).map_err(|e| e.to_string())? {
                    Tail::Zeros
                } else {
                    Tail::Bytes(file_len.saturating_sub(out.good_len))
                };
                break;
            }
        }
    }
    // A first save with damaged records that no commit closed, good or damaged: the file was
    // renamed into place only once all of it was written, so what it has of the mailbox is
    // taken, as far as it could be read (the host fetches the items lost again).
    if out.good_len == HEADER_LEN && salvaged > 0 && pending.iter().any(|op| matches!(op, Op::Rest(..))) {
        take_in(&mut out, &mut pending, max_records, budget)?;
        out.lost_records = u32::try_from(lost_items).unwrap_or(u32::MAX);
        out.good_len = at.min(file_len);
        out.tail = Tail::Bytes(file_len.saturating_sub(out.good_len).max(salvaged_bytes).max(1));
    }
    if out.good_len == HEADER_LEN {
        return Err(if base_bad || salvaged > 0 {
            "this mailbox file is damaged where it begins, and the mailbox couldn't be read from it".into()
        } else {
            "this mailbox file is damaged (no save in it was finished)".into()
        });
    }
    // Damaged records of the first save are damage too: the file is kept aside before it is
    // written again.
    if (out.lost_records > 0 || base_damaged || salvaged_bytes > 0) && !matches!(out.tail, Tail::Bytes(_)) {
        out.tail = Tail::Bytes(salvaged_bytes.max(1));
    }
    // Records no finished save took in are let go of.
    if let Some(budget) = budget {
        budget.give_back(pending.iter().map(cost).fold(0u64, u64::saturating_add));
    }
    Ok(out)
}

const TOO_MANY: &str = "this mailbox file holds more records than can be opened";
const NO_MEMORY: &str = "not enough memory to open the mailbox";

/// Take in the records of a save that is read, in their order.
fn take_in(out: &mut Replayed, pending: &mut Vec<Op>, max_records: usize, budget: Option<&Budget>) -> Result<(), String> {
    // What a record replaced or removed by a later one cost is given back.
    let mut freed = 0u64;
    for op in pending.drain(..) {
        match op {
            Op::Rest(json, fp) => {
                if let Some((old, _)) = out.rest.replace((json, fp)) {
                    freed = freed.saturating_add((old.len() as u64).saturating_add(RECORD_COST));
                }
            }
            Op::Put(key, item) => {
                if !out.items.contains_key(&key) {
                    if out.items.len() >= max_records {
                        return Err(TOO_MANY.into());
                    }
                    out.items.try_reserve(1).map_err(|_| NO_MEMORY.to_string())?;
                }
                if let Some(old) = out.items.insert(key, item) {
                    freed = freed.saturating_add(old.cost());
                }
            }
            Op::Delete(key) => {
                freed = freed.saturating_add(RECORD_COST);
                if let Some(old) = out.items.remove(&key) {
                    freed = freed.saturating_add(old.cost());
                }
            }
        }
    }
    if let Some(budget) = budget {
        budget.give_back(freed);
    }
    Ok(())
}

/// How many saves follow, `left` bytes before the end, after a damaged record that came
/// `since` records after the last save read (itself included): the save it is part of, and every
/// finished save after it. Only the frames are followed; what isn't a commit is passed over
/// unread.
fn count_lost(r: &mut impl Read, mut left: u64, mut since: u64) -> std::io::Result<u32> {
    let mut lost = 0u32;
    // A record since the last good commit was damaged (maybe a commit).
    let mut damaged_since = true;
    loop {
        let mut head = [0u8; 5];
        if left < FRAME || fill(r, &mut head)? < head.len() {
            break;
        }
        let [a, b, c, d, kind] = head;
        let len = u32::from_le_bytes([a, b, c, d]);
        if len > MAX_RECORD || !matches!(kind, REST | PUT | DELETE | COMMIT) {
            break;
        }
        let size = FRAME + u64::from(len);
        if size > left {
            break;
        }
        left -= size;
        if kind == COMMIT && len == 4 {
            let mut body = [0u8; 20];
            if fill(r, &mut body)? < body.len() {
                break;
            }
            let (count, sum) = body.split_at(4);
            if checksum([a, b, c, d], kind, &[count]) == sum {
                let count = count.try_into().map(u32::from_le_bytes).unwrap_or(0);
                // More records than this commit closes: the save before it lost its commit.
                if since > u64::from(count) && damaged_since {
                    lost = lost.saturating_add(1);
                }
                lost = lost.saturating_add(1);
                (since, damaged_since) = (0, false);
                continue;
            }
            damaged_since = true;
        } else {
            let rest = size - 5;
            if std::io::copy(&mut r.by_ref().take(rest), &mut std::io::sink())? < rest {
                break;
            }
        }
        since = since.saturating_add(1);
    }
    // A save whose commit was the damage, at the end.
    if since > 0 && damaged_since {
        lost = lost.saturating_add(1);
    }
    Ok(lost)
}
