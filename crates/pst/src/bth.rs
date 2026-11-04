// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The B-tree-on-heap ([MS-PST] 2.3.2). Every heap item of the tree is read at most once and the
//! tree is at most [`MAX_LEVELS`] index levels deep, so a hostile tree costs no more than its heap.

use std::collections::HashSet;

use crate::bytes::{slice, u8_at, u32_at};
use crate::error::{Error, Result};
use crate::heap::{CLIENT_BTH, Heap};

pub(crate) const MAX_LEVELS: u8 = 8;

pub(crate) struct Bth<'h> {
    pub key_size: usize,
    pub data_size: usize,
    /// Leaf records, `key_size + data_size` bytes each, in the order of the tree.
    pub records: Vec<&'h [u8]>,
}

pub(crate) fn read(heap: &Heap, header_hid: u32) -> Result<Bth<'_>> {
    let h = heap.item(header_hid)?;
    if u8_at(h, 0)? != CLIENT_BTH {
        return Err(Error::Corrupt("a B-tree-on-heap's signature"));
    }
    let key_size = usize::from(u8_at(h, 1)?);
    let data_size = usize::from(u8_at(h, 2)?);
    let levels = u8_at(h, 3)?;
    let root = u32_at(h, 4)?;
    if !matches!(key_size, 2 | 4 | 8 | 16) || data_size == 0 || data_size > 32 {
        return Err(Error::Corrupt("a B-tree-on-heap's record sizes"));
    }
    if levels > MAX_LEVELS {
        return Err(Error::Corrupt("a B-tree-on-heap too deep"));
    }
    let mut records = Vec::new();
    if root == 0 {
        return Ok(Bth { key_size, data_size, records });
    }
    let mut seen = HashSet::new();
    let mut stack = vec![(root, levels)];
    while let Some((hid, level)) = stack.pop() {
        if !seen.insert(hid) {
            return Err(Error::Cycle("a B-tree-on-heap"));
        }
        let item = heap.item(hid)?;
        if level == 0 {
            let size = key_size.saturating_add(data_size);
            if !item.len().is_multiple_of(size) {
                return Err(Error::Corrupt("a B-tree-on-heap's records"));
            }
            records.extend(item.chunks_exact(size));
            continue;
        }
        let size = key_size.saturating_add(4);
        if !item.len().is_multiple_of(size) {
            return Err(Error::Corrupt("a B-tree-on-heap's index"));
        }
        let below = level.saturating_sub(1);
        let n = item.len().checked_div(size).unwrap_or(0);
        let mut children = Vec::with_capacity(n);
        for i in 0..n {
            let r = slice(item, i.saturating_mul(size), size)?;
            children.push((u32_at(r, key_size)?, below));
        }
        // Last-first, so that the leaves come out in key order.
        stack.extend(children.into_iter().rev());
    }
    Ok(Bth { key_size, data_size, records })
}
