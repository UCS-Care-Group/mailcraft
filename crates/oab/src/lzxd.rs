// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! LZXD decompression, written from [MS-PATCH] (LZX DELTA Compression and Decompression), which
//! is how an Offline Address Book's blocks are packed ([MS-OXOAB] 2.11.2).
//!
//! A stream is chunks of 32 KB of output, each after a 16-bit size (2.2.1); the first chunk
//! starts with the E8 translation flag (2.2.2); then blocks, each verbatim, aligned offset or
//! uncompressed (2.3), with Huffman trees sent as the difference from the block before (2.5),
//! and literals and matches (2.6, 2.7). Reference data (for a patch) is put before the output.
//!
//! Everything is checked: the output is exactly the length asked for and allocated once, a match
//! never reaches before the start, past the end of its block or across a chunk, every chunk ends
//! where its size says, and the stream may not run out.

use crate::Error;
use crate::bits::Bits;
use crate::huffman::Tree;

/// The output a chunk holds (2.2.1).
pub const CHUNK: usize = 32 * 1024;
/// Literals in the main tree.
const LITERALS: usize = 256;
/// Elements of the length tree (2.4).
const LENGTHS: usize = 249;
/// Elements of a pretree (2.5).
const PRETREE: usize = 20;
/// Elements of the aligned offset tree (2.3.2.3).
const ALIGNED: usize = 8;
/// The most position slots (a 32 MB window, 2.1.6).
const MAX_SLOTS: usize = 290;

const NO_TREE: Error = Error::Corrupt("a match in a block without trees");

/// The position slots of a window of `2^bits` bytes (2.1.6), for 17 to 25 bits.
pub fn position_slots(window_bits: u32) -> Option<usize> {
    let slots = [34, 36, 38, 42, 50, 66, 98, 162, 290];
    slots.get(usize::try_from(window_bits.checked_sub(17)?).ok()?).copied()
}

/// The footer bits of a position slot (2.6.2): none for the first four, then one more every
/// second slot, up to 17.
pub(crate) fn footer_bits(slot: usize) -> u32 {
    if slot < 4 { 0 } else { (slot.saturating_sub(2) / 2).min(17) as u32 }
}

/// The base positions of the slots (2.6.2), each the last one plus its footer's range.
const BASE: [u32; MAX_SLOTS] = base_positions();

#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)] // a constant over 290 slots
const fn base_positions() -> [u32; MAX_SLOTS] {
    let mut t = [0u32; MAX_SLOTS];
    let mut slot = 1;
    while slot < MAX_SLOTS {
        let prev = slot - 1;
        let footer = if prev < 4 {
            0
        } else if (prev - 2) / 2 > 17 {
            17
        } else {
            (prev - 2) / 2
        };
        t[slot] = t[prev] + (1 << footer);
        slot += 1;
    }
    t
}

/// The base position of a slot.
pub(crate) fn base_position(slot: usize) -> Option<u32> {
    BASE.get(slot).copied()
}

enum Kind {
    Verbatim,
    Aligned(Tree),
}

enum Block {
    /// Between blocks.
    None,
    Compressed {
        kind: Kind,
        main: Tree,
        length: Option<Tree>,
    },
    /// An uncompressed block's bytes, read from `at`; `odd` when one byte of padding follows.
    Raw {
        at: usize,
        odd: bool,
    },
}

struct Decoder<'a> {
    bits: Bits<'a>,
    out: Vec<u8>,
    /// How much of `out` is reference data.
    start: usize,
    /// The output the caller asked for.
    end: usize,
    slots: usize,
    /// The repeated offsets (2.1.4).
    r: [usize; 3],
    /// The path lengths of the last trees, which the next are sent against.
    main_lens: Vec<u8>,
    length_lens: Vec<u8>,
    block: Block,
    /// What is left of the current block.
    left: usize,
    /// Where the current chunk's compressed bytes end, by its size.
    chunk_end: usize,
}

/// Unpacks an LZXD stream to exactly `out_len` bytes, with a window of `2^window_bits` bytes
/// (17 to 25) and `reference` before the output (empty but for a patch). The caller caps
/// `out_len`: it is allocated at once.
pub fn decode(input: &[u8], reference: &[u8], window_bits: u32, out_len: usize) -> Result<Vec<u8>, Error> {
    let slots = position_slots(window_bits).ok_or(Error::Corrupt("an LZX window size"))?;
    let total = reference.len().checked_add(out_len).ok_or(Error::Corrupt("an LZX output size"))?;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(reference);
    let mut d = Decoder {
        bits: Bits::new(input),
        out,
        start: reference.len(),
        end: total,
        slots,
        r: [1, 1, 1],
        main_lens: vec![0; LITERALS.saturating_add(slots.saturating_mul(8))],
        length_lens: vec![0; LENGTHS],
        block: Block::None,
        left: 0,
        chunk_end: 0,
    };
    if out_len == 0 {
        return Ok(Vec::new());
    }
    d.chunk_start()?;
    let e8 = if d.bits.read(1)? == 1 {
        let high = d.bits.read(16)?;
        let low = d.bits.read(16)?;
        Some((high << 16) | low)
    } else {
        None
    };
    d.run()?;
    let mut out = d.out.split_off(d.start);
    if let Some(size) = e8 {
        undo_e8(&mut out, size);
    }
    Ok(out)
}

impl Decoder<'_> {
    /// Output so far, without the reference data.
    fn produced(&self) -> usize {
        self.out.len().saturating_sub(self.start)
    }

    /// Reads a chunk's size, at a word.
    fn chunk_start(&mut self) -> Result<(), Error> {
        let size = self.bits.read(16)? as usize;
        self.chunk_end = self.bits.byte_pos().saturating_add(size);
        Ok(())
    }

    fn run(&mut self) -> Result<(), Error> {
        while self.out.len() < self.end {
            if self.left == 0 {
                self.block_header()?;
            }
            // What may be written before the chunk (or the output) ends.
            let room = CHUNK.saturating_sub(self.produced() % CHUNK).min(self.end.saturating_sub(self.out.len()));
            match &self.block {
                Block::Raw { .. } => self.raw(room)?,
                _ => self.token(room)?,
            }
            if self.left == 0
                && let Block::Raw { at, odd } = self.block
            {
                self.bits.resume(if odd { at.saturating_add(1) } else { at });
                self.block = Block::None;
            }
            if self.produced().is_multiple_of(CHUNK) && self.out.len() < self.end {
                self.next_chunk()?;
            }
        }
        Ok(())
    }

    /// At 32 KB of output: the bitstream is padded to a word, the chunk must end there, and the
    /// next one's size follows (2.2.1). In an uncompressed block, the size sits between its bytes.
    fn next_chunk(&mut self) -> Result<(), Error> {
        let at = match self.block {
            Block::Raw { at, .. } => at,
            _ => {
                self.bits.align();
                self.bits.byte_pos()
            }
        };
        if at != self.chunk_end {
            return Err(Error::Corrupt("an LZX chunk of another size than it says"));
        }
        match &mut self.block {
            Block::Raw { at, .. } => {
                let size = self.bits.data().get(*at..at.saturating_add(2)).ok_or(Error::Truncated("LZX chunk size"))?;
                let size = usize::from(u16::from_le_bytes([size.first().copied().unwrap_or(0), size.get(1).copied().unwrap_or(0)]));
                *at = at.saturating_add(2);
                self.chunk_end = at.saturating_add(size);
                Ok(())
            }
            _ => self.chunk_start(),
        }
    }

    /// A block's header, its trees or its repeated offsets (2.3).
    fn block_header(&mut self) -> Result<(), Error> {
        let kind = self.bits.read(3)?;
        let size = (self.bits.read(8)? << 16) | (self.bits.read(8)? << 8) | self.bits.read(8)?;
        let size = size as usize;
        if size == 0 || size > self.end.saturating_sub(self.out.len()) {
            return Err(Error::Corrupt("an LZX block of a size the output hasn't room for"));
        }
        self.left = size;
        self.block = match kind {
            1 | 2 => {
                let kind = if kind == 2 {
                    let mut lens = [0u8; ALIGNED];
                    for len in &mut lens {
                        *len = self.bits.read(3)? as u8;
                    }
                    Kind::Aligned(Tree::new(&lens)?)
                } else {
                    Kind::Verbatim
                };
                let split = LITERALS.min(self.main_lens.len());
                let (literals, matches) = self.main_lens.split_at_mut(split);
                read_lengths(&mut self.bits, literals)?;
                read_lengths(&mut self.bits, matches)?;
                read_lengths(&mut self.bits, &mut self.length_lens)?;
                let main = Tree::new(&self.main_lens)?;
                // A block without long matches may send an empty length tree.
                let length = if self.length_lens.iter().any(|&l| l > 0) { Some(Tree::new(&self.length_lens)?) } else { None };
                Block::Compressed { kind, main, length }
            }
            3 => {
                // 1 to 16 bits of padding: a whole word when already at one.
                if self.bits.aligned() {
                    self.bits.read(16)?;
                } else {
                    self.bits.align();
                }
                let at = self.bits.byte_pos();
                let data = self.bits.data();
                for (i, r) in self.r.iter_mut().enumerate() {
                    let from = at.saturating_add(i.saturating_mul(4));
                    let b = data.get(from..from.saturating_add(4)).ok_or(Error::Truncated("LZX repeated offsets"))?;
                    let mut word = [0u8; 4];
                    word.copy_from_slice(b);
                    *r = u32::from_le_bytes(word) as usize;
                }
                Block::Raw { at: at.saturating_add(12), odd: size % 2 == 1 }
            }
            _ => return Err(Error::Corrupt("an LZX block type")),
        };
        Ok(())
    }

    /// Copies an uncompressed block's bytes, up to `room`.
    fn raw(&mut self, room: usize) -> Result<(), Error> {
        let Block::Raw { at, .. } = &mut self.block else { return Ok(()) };
        let n = room.min(self.left);
        let bytes = self.bits.data().get(*at..at.saturating_add(n)).ok_or(Error::Truncated("an uncompressed LZX block"))?;
        self.out.extend_from_slice(bytes);
        *at = at.saturating_add(n);
        self.left = self.left.saturating_sub(n);
        Ok(())
    }

    /// One literal or match (2.7), which must fit in `room` and the block.
    fn token(&mut self, room: usize) -> Result<(), Error> {
        let Block::Compressed { kind, main, length } = &self.block else { return Err(NO_TREE) };
        let element = usize::from(main.decode(&mut self.bits)?);
        if element < LITERALS {
            self.out.push(element as u8);
            self.left = self.left.saturating_sub(1);
            return Ok(());
        }
        let header = element.saturating_sub(LITERALS);
        let (length_header, slot) = (header & 7, header >> 3);
        let mut len = if length_header == 7 {
            let tree = length.as_ref().ok_or(Error::Corrupt("a long match without a length tree"))?;
            usize::from(tree.decode(&mut self.bits)?).saturating_add(9)
        } else {
            length_header.saturating_add(2)
        };
        let offset = match slot {
            0 => self.r[0],
            1 => {
                self.r.swap(0, 1);
                self.r[0]
            }
            2 => {
                self.r.swap(0, 2);
                self.r[0]
            }
            _ => {
                if slot >= self.slots {
                    return Err(Error::Corrupt("an LZX position slot past the window"));
                }
                let footer = footer_bits(slot);
                let base = base_position(slot).ok_or(Error::Corrupt("an LZX position slot"))?;
                // In an aligned offset block, a footer of three bits or more has its lowest three
                // from the aligned offset tree (2.6.3: every formatted offset above 15).
                let extra = match kind {
                    Kind::Aligned(aligned) if footer >= 3 => {
                        let verbatim = self.bits.read(footer.saturating_sub(3))? << 3;
                        verbatim | u32::from(aligned.decode(&mut self.bits)?)
                    }
                    _ => self.bits.read(footer)?,
                };
                let offset = (base.saturating_add(extra) as usize).checked_sub(2).ok_or(Error::Corrupt("an LZX offset"))?;
                self.r = [offset, self.r[0], self.r[1]];
                offset
            }
        };
        if len == 257 {
            let extra = if self.bits.read(1)? == 0 {
                self.bits.read(8)?
            } else if self.bits.read(1)? == 0 {
                self.bits.read(10)?.saturating_add(256)
            } else if self.bits.read(1)? == 0 {
                self.bits.read(12)?.saturating_add(256 + 1024)
            } else {
                self.bits.read(15)?
            };
            len = len.saturating_add(extra as usize);
        }
        if len > room || len > self.left {
            return Err(Error::Corrupt("an LZX match past its block or chunk"));
        }
        if offset == 0 || offset > self.out.len() {
            return Err(Error::Corrupt("an LZX match before the start"));
        }
        let from = self.out.len().saturating_sub(offset);
        for i in 0..len {
            let b = self.out.get(from.saturating_add(i)).copied().ok_or(Error::Corrupt("an LZX match"))?;
            self.out.push(b);
        }
        self.left = self.left.saturating_sub(len);
        Ok(())
    }
}

/// A tree's path lengths, sent through a pretree as the difference from `lens` (2.5).
fn read_lengths(bits: &mut Bits<'_>, lens: &mut [u8]) -> Result<(), Error> {
    let mut pre = [0u8; PRETREE];
    for len in &mut pre {
        *len = bits.read(4)? as u8;
    }
    let pretree = Tree::new(&pre)?;
    let mut i = 0;
    while i < lens.len() {
        let code = pretree.decode(bits)?;
        let (run, value) = match code {
            0..=16 => (1, delta(lens.get(i).copied().unwrap_or(0), code)),
            17 => (4usize.saturating_add(bits.read(4)? as usize), 0),
            18 => (20usize.saturating_add(bits.read(5)? as usize), 0),
            _ => {
                let run = 4usize.saturating_add(bits.read(1)? as usize);
                let code = pretree.decode(bits)?;
                if code > 16 {
                    return Err(Error::Corrupt("an LZX tree length"));
                }
                (run, delta(lens.get(i).copied().unwrap_or(0), code))
            }
        };
        let to = i.saturating_add(run);
        let lens = lens.get_mut(i..to).ok_or(Error::Corrupt("an LZX run past its tree"))?;
        lens.fill(value);
        i = to;
    }
    Ok(())
}

/// `(previous - code + 17) mod 17`.
fn delta(previous: u8, code: u16) -> u8 {
    ((u16::from(previous % 17)).saturating_add(17).saturating_sub(code % 17) % 17) as u8
}

/// Undoes E8 call translation (2.2.2), chunk by chunk, on the finished output.
fn undo_e8(out: &mut [u8], file_size: u32) {
    let size = i64::from(file_size);
    for (index, chunk) in out.chunks_mut(CHUNK).enumerate() {
        let chunk_offset = (index as i64).saturating_mul(CHUNK as i64);
        if chunk_offset >= 0x4000_0000 || chunk.len() <= 10 {
            continue;
        }
        let stop = chunk.len().saturating_sub(10);
        let mut i = 0;
        while i < stop {
            if chunk.get(i) == Some(&0xE8)
                && let Some(field) = chunk.get_mut(i.saturating_add(1)..i.saturating_add(5))
            {
                let current = chunk_offset.saturating_add(i as i64);
                let mut word = [0u8; 4];
                word.copy_from_slice(field);
                let value = i64::from(i32::from_le_bytes(word));
                if value >= current.saturating_neg() && value < size {
                    let displacement = if value >= 0 { value.saturating_sub(current) } else { value.saturating_add(size) };
                    field.copy_from_slice(&(displacement as i32).to_le_bytes());
                }
                i = i.saturating_add(4);
            }
            i = i.saturating_add(1);
        }
    }
}
