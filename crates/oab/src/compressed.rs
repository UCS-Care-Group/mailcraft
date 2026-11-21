// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A compressed details or template file ([MS-OXOAB] 2.11): an `LZX_HDR` and then `LZX_BLK`s,
//! each stored or packed with LZXD on its own, each with the CRC of what it unpacks to.

use crate::lzxd;
use crate::{Cap, Error, Limits, crc32};

/// `ulVersionHi` and `ulVersionLo` of a compressed file.
const VERSION: (u32, u32) = (3, 1);
const HEADER: usize = 16;
const BLOCK_HEADER: usize = 16;

/// The smallest LZXD window: 128 KB.
const MIN_WINDOW_BITS: u32 = 17;

/// The four little-endian words at `at`.
fn words(b: &[u8], at: usize, what: &'static str) -> Result<[u32; 4], Error> {
    let bytes = b.get(at..at.saturating_add(16)).ok_or(Error::Truncated(what))?;
    let mut w = [0u32; 4];
    for (word, chunk) in w.iter_mut().zip(bytes.as_chunks::<4>().0) {
        *word = u32::from_le_bytes(*chunk);
    }
    Ok(w)
}

/// The smallest window that holds `size` bytes, from 128 KB. [MS-OXOAB] doesn't say which window
/// a block is packed with; a server's file of one block of 77 KB, with `ulBlockMax` 256 KB, is
/// packed with 128 KB: the window that holds the block, not the largest block.
pub(crate) fn window_bits(size: usize) -> Result<u32, Error> {
    let mut bits = MIN_WINDOW_BITS;
    while (1usize << bits) < size {
        bits = bits.saturating_add(1);
        if lzxd::position_slots(bits).is_none() {
            return Err(Error::Corrupt("an LZX block larger than a window"));
        }
    }
    Ok(bits)
}

/// Unpacks a compressed details or template file, checking every block's CRC.
pub fn decompress(file: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let [hi, lo, block_max, target] = words(file, 0, "LZX_HDR")?;
    if (hi, lo) != VERSION {
        return Err(Error::NotOab("LZX_HDR version"));
    }
    let (block_max, target) = (block_max as usize, target as usize);
    if target > limits.max_output {
        return Err(Error::Cap(Cap::OutputSize));
    }
    if block_max > limits.max_block {
        return Err(Error::Cap(Cap::BlockSize));
    }
    let largest = window_bits(block_max)?;
    // Room is made as blocks come, never past what the file says it unpacks to: a header alone
    // reserves nothing.
    let mut out: Vec<u8> = Vec::new();
    let mut at = HEADER;
    // The window the last packed block unpacked with: the server's, as a rule, so it is tried
    // first for the next.
    let mut window = None;
    while out.len() < target {
        let [flags, packed, unpacked, crc] = words(file, at, "LZX_BLK")?;
        let (packed, unpacked) = (packed as usize, unpacked as usize);
        if unpacked == 0 || unpacked > block_max || unpacked > target.saturating_sub(out.len()) {
            return Err(Error::Corrupt("an LZX_BLK of a size the file hasn't room for"));
        }
        if packed > block_max.saturating_mul(2).max(1 << 16) || packed > limits.max_block {
            return Err(Error::Cap(Cap::BlockSize));
        }
        let from = at.saturating_add(BLOCK_HEADER);
        let data = file.get(from..from.saturating_add(packed)).ok_or(Error::Truncated("LZX_BLK data"))?;
        grow(&mut out, unpacked, target);
        match flags {
            0 if packed == unpacked => {
                if crc32(data) != crc {
                    return Err(Error::BadChecksum("LZX_BLK"));
                }
                out.extend_from_slice(data);
            }
            0 => return Err(Error::Corrupt("a stored LZX_BLK of two sizes")),
            // Its CRC is checked there, for each window tried.
            1 => out.extend_from_slice(&unpack(data, unpacked, crc, largest, &mut window)?),
            _ => return Err(Error::Corrupt("LZX_BLK flags")),
        }
        at = from.saturating_add(packed);
    }
    Ok(out)
}

/// Room in `out` for `more` bytes: twice what it holds, as a `Vec` grows, but never past `target`
/// (what the file unpacks to), so that the last block doesn't double a large output.
fn grow(out: &mut Vec<u8>, more: usize, target: usize) {
    let need = out.len().saturating_add(more);
    if out.capacity() < need {
        let room = out.capacity().saturating_mul(2).max(need).min(target.max(need));
        out.reserve_exact(room.saturating_sub(out.len()));
    }
}

/// A packed block: with the window the block before unpacked with (`window`), then with the one
/// that holds it, and, should those fail, with each other window up to the one that holds the
/// largest block; only output with the block's CRC is taken, so a wrong window is never believed.
/// The window that worked is kept in `window` for the next block. A failure with the window that
/// holds the block is the one named.
fn unpack(data: &[u8], unpacked: usize, crc: u32, largest: u32, window: &mut Option<u32>) -> Result<Vec<u8>, Error> {
    let first = window_bits(unpacked)?;
    let try_with = |bits: u32| match lzxd::decode(data, &[], bits, unpacked) {
        Ok(block) if crc32(&block) == crc => Ok(block),
        Ok(_) => Err(Error::BadChecksum("LZX_BLK")),
        Err(e) => Err(e),
    };
    let last = window.filter(|&bits| bits != first && (MIN_WINDOW_BITS..=largest).contains(&bits));
    if let Some(bits) = last
        && let Ok(block) = try_with(bits)
    {
        return Ok(block);
    }
    let failure = match try_with(first) {
        Ok(block) => {
            *window = Some(first);
            return Ok(block);
        }
        Err(e) => e,
    };
    for bits in (MIN_WINDOW_BITS..=largest).filter(|&b| b != first && Some(b) != last) {
        if let Ok(block) = try_with(bits) {
            *window = Some(bits);
            return Ok(block);
        }
    }
    Err(failure)
}
