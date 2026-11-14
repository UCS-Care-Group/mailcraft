// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What can go wrong reading an Offline Address Book, and the caps that keep a hostile one
//! bounded.

use std::fmt;

/// A failure to read an Offline Address Book file. Nothing in this crate panics: everything a
/// file can get wrong ends here.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The bytes end before what they say is there.
    Truncated(&'static str),
    /// Not a file of the kind asked for: a version field that says otherwise.
    NotOab(&'static str),
    /// A CRC that doesn't match what it guards.
    BadChecksum(&'static str),
    /// A structure that makes no sense.
    Corrupt(&'static str),
    /// Past one of the caps in [`Limits`]: not read.
    Cap(Cap),
}

/// Which cap was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Cap {
    /// [`Limits::max_output`]: the size of the file once unpacked.
    OutputSize,
    /// [`Limits::max_block`]: the size of one block, packed or unpacked.
    BlockSize,
    /// [`Limits::max_records`]: the number of entries.
    Records,
    /// [`Limits::max_record`]: the size of one entry.
    RecordSize,
    /// [`Limits::max_properties`]: the number of properties an entry can have.
    Properties,
    /// [`Limits::max_values`]: the values kept of the whole file.
    Values,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated(what) => write!(f, "the offline address book ends early ({what})"),
            Error::NotOab(what) => write!(f, "not an offline address book of version 4 ({what})"),
            Error::BadChecksum(what) => write!(f, "the offline address book is damaged: a checksum doesn't match ({what})"),
            Error::Corrupt(what) => write!(f, "the offline address book is damaged ({what})"),
            Error::Cap(cap) => write!(f, "the offline address book is larger than allowed ({cap:?})"),
        }
    }
}

impl std::error::Error for Error {}

/// The caps on what one file may make this crate read or hold. Each is checked before what it
/// guards is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The most bytes a compressed file may unpack to (its `ulTargetSize`).
    pub max_output: usize,
    /// The most bytes one block may hold, packed or unpacked (its `ulBlockMax`; the server
    /// sets 256 KB).
    pub max_block: usize,
    /// The most entries a file may have (its `ulTotRecs`).
    pub max_records: usize,
    /// The most bytes one entry may have.
    pub max_record: usize,
    /// The most properties the schema may list for an entry or for the header.
    pub max_properties: usize,
    /// The most values kept of a whole file, the header's and the entries' together.
    pub max_values: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_output: 1 << 30, max_block: 1 << 24, max_records: 4_000_000, max_record: 1 << 20, max_properties: 4096, max_values: 16_000_000 }
    }
}
