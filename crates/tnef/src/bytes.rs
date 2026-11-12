// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Little-endian numbers and slices from what is left of a stream, never past its end.

/// A place in a stream of bytes.
pub(crate) struct Cursor<'a> {
    rest: &'a [u8],
}

impl<'a> Cursor<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Cursor<'a> {
        Cursor { rest: bytes }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }

    /// The next `n` bytes, or `None` (and nothing taken) when fewer are left.
    pub(crate) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.rest.split_at_checked(n)?;
        self.rest = rest;
        Some(head)
    }

    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N).and_then(|b| <[u8; N]>::try_from(b).ok())
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        self.array::<1>().map(|[b]| b)
    }

    pub(crate) fn u16(&mut self) -> Option<u16> {
        self.array().map(u16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_le_bytes)
    }

    /// The next `n` bytes as a length-counted value, then the padding that brings `n` to a
    /// multiple of four (2.1.3.4). Padding cut short by the end of the stream is no harm.
    pub(crate) fn padded(&mut self, n: usize) -> Option<&'a [u8]> {
        let value = self.take(n)?;
        let pad = n.wrapping_neg() & 3;
        let _ = self.take(pad.min(self.rest.len()));
        Some(value)
    }
}
