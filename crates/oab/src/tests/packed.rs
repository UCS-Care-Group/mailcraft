// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Compressed files whose blocks are packed by the test encoder.

use super::encoder::{Options, encode};
use crate::crc32;

/// `data` as a compressed file of blocks of `block_max` bytes; every `stored`th block is stored
/// rather than packed (never, for 0). Each block is packed with the window that holds it, as a
/// server does, unless `largest_window`: then with the one that holds `block_max`.
pub fn compressed_with(data: &[u8], block_max: usize, stored: usize, opts: &Options, largest_window: bool) -> Vec<u8> {
    let mut file = Vec::new();
    for word in [3u32, 1, block_max as u32, data.len() as u32] {
        file.extend_from_slice(&word.to_le_bytes());
    }
    for (i, block) in data.chunks(block_max).enumerate() {
        let store = stored > 0 && (i + 1) % stored == 0;
        let size = if largest_window { block_max } else { block.len() };
        let opts = Options { window_bits: crate::compressed::window_bits(size).unwrap(), ..opts.clone() };
        let packed = if store { block.to_vec() } else { encode(block, &opts) };
        for word in [u32::from(!store), packed.len() as u32, block.len() as u32, crc32(block)] {
            file.extend_from_slice(&word.to_le_bytes());
        }
        file.extend_from_slice(&packed);
    }
    file
}

/// [`compressed_with`], each block packed with the window that holds it.
pub fn compressed(data: &[u8], block_max: usize, stored: usize, opts: &Options) -> Vec<u8> {
    compressed_with(data, block_max, stored, opts, false)
}
