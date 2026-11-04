// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The file under the reader: any `Read + Seek`, read a page or a block at a time and never
//! whole. Every read is checked against the file's size before anything is allocated for it.

use std::io::{Read, Seek, SeekFrom};

use crate::error::{Cap, Error, Result};

/// The largest single read: a block ([MS-PST] 2.2.2.8: at most 8 KB).
pub(crate) const MAX_READ: usize = 8192;

pub(crate) struct Source<R> {
    inner: R,
    size: u64,
}

impl<R: Read + Seek> Source<R> {
    pub(crate) fn new(mut inner: R, max_size: u64) -> Result<Self> {
        let size = inner.seek(SeekFrom::End(0))?;
        if size > max_size {
            return Err(Error::Cap(Cap::FileSize));
        }
        Ok(Source { inner, size })
    }

    pub(crate) fn size(&self) -> u64 {
        self.size
    }

    /// `len` bytes at `offset`, all of them inside the file.
    pub(crate) fn read_at(&mut self, offset: u64, len: usize) -> Result<Vec<u8>> {
        if len > MAX_READ {
            return Err(Error::Corrupt("a read larger than a block"));
        }
        let end = offset.checked_add(len as u64).ok_or(Error::OutOfBounds)?;
        if end > self.size {
            return Err(Error::OutOfBounds);
        }
        self.inner.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len];
        self.inner.read_exact(&mut buf)?;
        Ok(buf)
    }
}
