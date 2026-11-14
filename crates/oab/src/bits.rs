// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The LZXD bitstream ([MS-PATCH] 2.1.1): 16-bit little-endian words, each read from its most
//! significant bit down. Bits past the end read as zero so that a code can be looked at whole,
//! but taking one of them is [`Error::Truncated`].

use crate::Error;

pub(crate) struct Bits<'a> {
    data: &'a [u8],
    /// The byte where the next word is loaded from (past the end once the data has run out).
    next: usize,
    /// The bits loaded and not yet taken: the low `n` bits, the oldest highest.
    buf: u64,
    n: u32,
    /// How many of the lowest bits of `buf` were loaded past the end of the data.
    phantom: u32,
}

const END: Error = Error::Truncated("LZX bitstream");

impl<'a> Bits<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Bits { data, next: 0, buf: 0, n: 0, phantom: 0 }
    }

    /// Makes sure there are at least `k` bits (at most 32) to look at.
    fn fill(&mut self, k: u32) {
        while self.n < k {
            let word = match self.data.get(self.next..self.next.saturating_add(2)) {
                Some(&[lo, hi]) if self.phantom == 0 => u64::from(u16::from_le_bytes([lo, hi])),
                _ => {
                    self.phantom = self.phantom.saturating_add(16);
                    0
                }
            };
            self.next = self.next.saturating_add(2);
            self.buf = (self.buf << 16) | word;
            self.n = self.n.saturating_add(16);
        }
    }

    /// The next `k` bits (at most 32), not taken.
    pub(crate) fn peek(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        self.fill(k);
        let shift = self.n.saturating_sub(k);
        ((self.buf >> shift) & (u64::MAX >> 64u32.saturating_sub(k))) as u32
    }

    /// Takes `k` bits that [`Bits::peek`] has looked at.
    pub(crate) fn skip(&mut self, k: u32) -> Result<(), Error> {
        self.fill(k);
        self.n = self.n.saturating_sub(k);
        if self.n < self.phantom { Err(END) } else { Ok(()) }
    }

    /// Takes the next `k` bits (at most 32).
    pub(crate) fn read(&mut self, k: u32) -> Result<u32, Error> {
        let v = self.peek(k);
        self.skip(k)?;
        Ok(v)
    }

    /// Whether the next bit is the first of a word.
    pub(crate) fn aligned(&self) -> bool {
        self.n.is_multiple_of(16)
    }

    /// Drops what is left of the current word.
    pub(crate) fn align(&mut self) {
        self.n = self.n.saturating_sub(self.n % 16);
    }

    /// After [`Bits::align`]: the byte the next word starts at.
    pub(crate) fn byte_pos(&self) -> usize {
        self.next.saturating_sub((self.n / 8) as usize)
    }

    /// After [`Bits::align`]: leaves the words for bytes to be read at [`Bits::byte_pos`],
    /// with [`Bits::resume`] where the bitstream goes on.
    pub(crate) fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Goes on with the bitstream at byte `at`.
    pub(crate) fn resume(&mut self, at: usize) {
        self.next = at;
        self.buf = 0;
        self.n = 0;
        self.phantom = 0;
    }
}
