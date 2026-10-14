//! A Compound File Binary reader ([MS-CFB]): the file system inside a `.msg` file.
//!
//! Read-only and in memory. Every chain of sectors is followed at most once around (a chain that
//! comes back on itself ends where it turned), every index is checked, and what is read is capped;
//! a damaged file gives an error or less content, never a panic and never unbounded work.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use crate::MsgError;

/// The first eight bytes of every compound file.
pub const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Largest stream read; what lies beyond is left out.
pub const MAX_STREAM: usize = 128 << 20;

/// Most directory entries read.
const MAX_ENTRIES: usize = 1 << 18;

/// Most children listed for one storage.
const MAX_CHILDREN: usize = 1 << 16;

const MAXREGSECT: u32 = 0xFFFF_FFFA;
const ENDOFCHAIN: u32 = 0xFFFF_FFFE;
const FREESECT: u32 = 0xFFFF_FFFF;
const NOSTREAM: u32 = 0xFFFF_FFFF;

/// What a directory entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Storage,
    Stream,
    Root,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    left: u32,
    right: u32,
    child: u32,
    start: u32,
    size: u64,
}

/// An opened compound file.
pub struct Cfb<'a> {
    data: &'a [u8],
    sector_size: usize,
    mini_sector_size: usize,
    cutoff: u64,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    mini_stream: Vec<u8>,
    entries: Vec<Option<Entry>>,
    /// Bytes still to be handed out by [`Cfb::stream`]: a few times the file, so that streams
    /// sharing their sectors can't make the work grow past the size of the file.
    budget: Cell<usize>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u32_at(b, at)?) | (u64::from(u32_at(b, at.checked_add(4)?)?) << 32))
}

fn damaged(what: &str) -> MsgError {
    MsgError::Damaged(what.to_string())
}

impl<'a> Cfb<'a> {
    /// Open the compound file `data` holds.
    pub fn open(data: &'a [u8]) -> Result<Cfb<'a>, MsgError> {
        if data.get(..8) != Some(&SIGNATURE[..]) || data.len() < 512 {
            return Err(MsgError::NotCompound);
        }
        let header = data.get(..512).ok_or(MsgError::NotCompound)?;
        let major = u16_at(header, 26).unwrap_or(0);
        let shift = u16_at(header, 30).unwrap_or(0);
        let sector_size = match (major, shift) {
            (3, 9) => 512,
            (4, 12) => 4096,
            // Lenient about a version that disagrees with its sector size, but only the two sizes
            // the specification allows.
            (_, 9) => 512,
            (_, 12) => 4096,
            _ => return Err(damaged("unknown sector size")),
        };
        let mini_shift = u16_at(header, 32).unwrap_or(6);
        let mini_sector_size = if mini_shift == 6 { 64 } else { return Err(damaged("unknown mini sector size")) };
        let cutoff = u64::from(u32_at(header, 56).unwrap_or(4096));
        let mut cfb = Cfb {
            data,
            sector_size,
            mini_sector_size,
            cutoff,
            fat: Vec::new(),
            mini_fat: Vec::new(),
            mini_stream: Vec::new(),
            entries: Vec::new(),
            budget: Cell::new(data.len().saturating_mul(4).saturating_add(16 << 20)),
        };
        cfb.read_fat(header)?;
        cfb.read_directory(u32_at(header, 48).unwrap_or(ENDOFCHAIN))?;
        cfb.mini_fat = cfb.chain_u32s(u32_at(header, 60).unwrap_or(ENDOFCHAIN));
        if let Some(Some(root)) = cfb.entries.first() {
            let (start, size) = (root.start, root.size);
            cfb.mini_stream = cfb.read_chain(start, size, &cfb.fat, cfb.sector_size, false);
        }
        Ok(cfb)
    }

    /// How many whole or partial sectors follow the header.
    fn sector_count(&self) -> usize {
        self.data.len().saturating_sub(self.sector_size).div_ceil(self.sector_size)
    }

    /// Sector `n` (a short last sector is what there is of it).
    fn sector(&self, n: u32) -> Option<&'a [u8]> {
        let start = usize::try_from(n).ok()?.checked_add(1)?.checked_mul(self.sector_size)?;
        let end = start.checked_add(self.sector_size)?.min(self.data.len());
        self.data.get(start..end).filter(|s| !s.is_empty())
    }

    fn read_fat(&mut self, header: &[u8]) -> Result<(), MsgError> {
        let max_sectors = self.sector_count();
        // The FAT's own sectors: 109 in the header, the rest in a chain of DIFAT sectors.
        let mut fat_sectors: Vec<u32> = (0..109).filter_map(|i| u32_at(header, 76 + i * 4)).filter(|s| *s <= MAXREGSECT).collect();
        let mut difat = u32_at(header, 68).unwrap_or(ENDOFCHAIN);
        let per = self.sector_size / 4 - 1;
        let mut seen = vec![false; max_sectors];
        while difat <= MAXREGSECT {
            let Some(slot) = seen.get_mut(difat as usize) else { break };
            if *slot {
                break;
            }
            *slot = true;
            let Some(sector) = self.sector(difat) else { break };
            fat_sectors.extend((0..per).filter_map(|i| u32_at(sector, i * 4)).filter(|s| *s <= MAXREGSECT));
            difat = u32_at(sector, per * 4).unwrap_or(ENDOFCHAIN);
        }
        // No file needs more FAT sectors than it has sectors.
        fat_sectors.truncate(max_sectors);
        let per_fat = self.sector_size / 4;
        let mut fat = Vec::with_capacity(fat_sectors.len().saturating_mul(per_fat).min(max_sectors.saturating_add(per_fat)));
        for s in fat_sectors {
            let Some(sector) = self.sector(s) else { continue };
            fat.extend((0..per_fat).map(|i| u32_at(sector, i * 4).unwrap_or(FREESECT)));
        }
        if fat.is_empty() {
            return Err(damaged("no allocation table"));
        }
        self.fat = fat;
        Ok(())
    }

    /// The sectors of a chain in `table`, in order, each at most once.
    fn chain(&self, start: u32, table: &[u32], limit: usize) -> Vec<u32> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut at = start;
        while at <= MAXREGSECT && out.len() < limit && seen.insert(at) {
            out.push(at);
            // A sector past the end of the table ends its chain.
            at = table.get(at as usize).copied().unwrap_or(ENDOFCHAIN);
        }
        out
    }

    /// The bytes of a chain, `size` of them at most (and never more than [`MAX_STREAM`]).
    fn read_chain(&self, start: u32, size: u64, table: &[u32], unit: usize, mini: bool) -> Vec<u8> {
        let want = usize::try_from(size).unwrap_or(usize::MAX).min(MAX_STREAM);
        let limit = want.div_ceil(unit.max(1)).saturating_add(1);
        let mut out = Vec::with_capacity(want.min(self.data.len()));
        for s in self.chain(start, table, limit) {
            if out.len() >= want {
                break;
            }
            let piece = if mini {
                let at = (s as usize).saturating_mul(unit);
                self.mini_stream.get(at..at.saturating_add(unit).min(self.mini_stream.len()))
            } else {
                self.sector(s)
            };
            let Some(piece) = piece else { break };
            let take = piece.len().min(want - out.len());
            out.extend_from_slice(piece.get(..take).unwrap_or(&[]));
        }
        out
    }

    /// A chain read as little-endian numbers (the mini FAT).
    fn chain_u32s(&self, start: u32) -> Vec<u32> {
        let limit = self.sector_count();
        let mut out = Vec::new();
        for s in self.chain(start, &self.fat, limit) {
            let Some(sector) = self.sector(s) else { break };
            out.extend((0..self.sector_size / 4).filter_map(|i| u32_at(sector, i * 4)));
        }
        out
    }

    fn read_directory(&mut self, start: u32) -> Result<(), MsgError> {
        let limit = self.sector_count();
        let per = self.sector_size / 128;
        let mut entries = Vec::new();
        'sectors: for s in self.chain(start, &self.fat, limit) {
            let Some(sector) = self.sector(s) else { break };
            for i in 0..per {
                if entries.len() >= MAX_ENTRIES {
                    break 'sectors;
                }
                let Some(raw) = sector.get(i * 128..i * 128 + 128) else { break 'sectors };
                entries.push(entry(raw, self.sector_size == 512));
            }
        }
        if !matches!(entries.first(), Some(Some(Entry { kind: Kind::Root, .. }))) {
            return Err(damaged("no root directory entry"));
        }
        self.entries = entries;
        Ok(())
    }

    /// The root storage.
    pub fn root(&self) -> usize {
        0
    }

    pub fn entry(&self, id: usize) -> Option<&Entry> {
        self.entries.get(id).and_then(Option::as_ref)
    }

    /// The children of a storage by upper-case name (the tree of siblings walked once).
    pub fn children(&self, storage: usize) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        let Some(first) = self.entry(storage).map(|e| e.child) else { return out };
        let mut seen = HashSet::new();
        let mut stack = vec![first];
        while let Some(id) = stack.pop() {
            if id == NOSTREAM || out.len() >= MAX_CHILDREN || !seen.insert(id) {
                continue;
            }
            let Some(e) = self.entry(id as usize) else { continue };
            out.entry(e.name.to_uppercase()).or_insert(id as usize);
            stack.push(e.left);
            stack.push(e.right);
        }
        out
    }

    /// The bytes of a stream.
    pub fn stream(&self, id: usize) -> Vec<u8> {
        let Some(e) = self.entry(id).filter(|e| e.kind == Kind::Stream) else { return Vec::new() };
        let budget = self.budget.get();
        let size = e.size.min(budget as u64);
        let bytes = if e.size < self.cutoff {
            self.read_chain(e.start, size, &self.mini_fat, self.mini_sector_size, true)
        } else {
            self.read_chain(e.start, size, &self.fat, self.sector_size, false)
        };
        self.budget.set(budget.saturating_sub(bytes.len().max(1)));
        bytes
    }
}

/// One 128-byte directory entry; `None` for an unused one.
fn entry(raw: &[u8], v3: bool) -> Option<Entry> {
    let kind = match raw.get(66)? {
        1 => Kind::Storage,
        2 => Kind::Stream,
        5 => Kind::Root,
        _ => return None,
    };
    let name_len = usize::from(u16_at(raw, 64)?).min(64);
    let units: Vec<u16> = (0..name_len / 2).filter_map(|i| u16_at(raw, i * 2)).take_while(|u| *u != 0).collect();
    Some(Entry {
        name: String::from_utf16_lossy(&units),
        kind,
        left: u32_at(raw, 68)?,
        right: u32_at(raw, 72)?,
        child: u32_at(raw, 76)?,
        start: u32_at(raw, 116)?,
        // Version 3 files may leave garbage in the high half; no stream of theirs is that big.
        size: if v3 { u64::from(u32_at(raw, 120)?) } else { u64_at(raw, 120)? },
    })
}
