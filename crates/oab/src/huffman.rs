// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The canonical Huffman trees of [MS-PATCH] 2.4, rebuilt from their path lengths alone: shorter
//! codes first and, within a length, the lower element first. A tree whose lengths ask for more
//! codes than there are is refused; one with codes to spare is kept, and a spare code read is an
//! error when it is read.

use crate::Error;
use crate::bits::Bits;

/// The longest path a tree has.
pub(crate) const MAX_LEN: usize = 16;

pub(crate) struct Tree {
    /// How many elements have each length (index 0 unused).
    counts: [u16; MAX_LEN + 1],
    /// The elements with a length, by length and then by number.
    symbols: Vec<u16>,
}

impl Tree {
    pub(crate) fn new(lens: &[u8]) -> Result<Tree, Error> {
        let mut counts = [0u16; MAX_LEN + 1];
        for &len in lens {
            let at = counts.get_mut(usize::from(len)).ok_or(Error::Corrupt("a path longer than 16"))?;
            *at = at.saturating_add(1);
        }
        if let Some(zero) = counts.get_mut(0) {
            *zero = 0;
        }
        // Codes left at each length: never below zero.
        let mut left: i64 = 1;
        for &count in counts.iter().skip(1) {
            left = left.saturating_mul(2).saturating_sub(i64::from(count));
            if left < 0 {
                return Err(Error::Corrupt("a tree with more codes than there are"));
            }
        }
        let mut symbols = Vec::with_capacity(lens.len());
        for len in 1..=MAX_LEN {
            for (symbol, &l) in lens.iter().enumerate() {
                if usize::from(l) == len {
                    symbols.push(u16::try_from(symbol).map_err(|_| Error::Corrupt("a tree too large"))?);
                }
            }
        }
        Ok(Tree { counts, symbols })
    }

    /// The next element.
    pub(crate) fn decode(&self, bits: &mut Bits<'_>) -> Result<u16, Error> {
        let v = bits.peek(MAX_LEN as u32);
        let (mut code, mut first, mut index) = (0u32, 0u32, 0usize);
        for (len, &count) in self.counts.iter().enumerate().skip(1) {
            let count = u32::from(count);
            code |= (v >> MAX_LEN.saturating_sub(len)) & 1;
            if code < first.saturating_add(count) {
                bits.skip(len as u32)?;
                let at = index.saturating_add(code.saturating_sub(first) as usize);
                return self.symbols.get(at).copied().ok_or(Error::Corrupt("a code no tree has"));
            }
            index = index.saturating_add(count as usize);
            first = first.saturating_add(count) << 1;
            code <<= 1;
        }
        Err(Error::Corrupt("a code no tree has"))
    }
}
