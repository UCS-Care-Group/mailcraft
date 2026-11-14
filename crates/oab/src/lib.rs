// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's reader of Exchange Offline Address Books, written from Microsoft's published
//! specifications and nothing else: [MS-OXOAB] for the files (version 4 Full Details, 2.9, and
//! its compressed form, 2.11) and [MS-PATCH] for the LZXD compression of their blocks.
//!
//! An Offline Address Book is the organisation's address list as one file the server publishes
//! (its address from Autodiscover's `OABUrl`, its table of contents `oab.xml`), so that a mail
//! program can list and search every entry without asking the server for each. [`read`] takes
//! the compressed file as it is downloaded and gives back its [`Details`]: the list's name and
//! sequence number, the schema, and a [`Record`] for every entry. [`decompress`] and
//! [`read_details`] are its two halves; [`lzxd::decode`] is the decompressor alone.
//!
//! An Offline Address Book is hostile input: a server, or anything between it and us, can write
//! it. Nothing here panics on any bytes and nothing here does I/O: every size is checked against
//! the bytes there are and the caps of [`Limits`] before anything is allocated, every block's
//! CRC and the file's are checked, and the decompressor's output is exactly what was asked for.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![cfg_attr(not(test), deny(clippy::indexing_slicing, clippy::arithmetic_side_effects))]

mod bits;
mod compressed;
mod crc;
mod details;
mod error;
mod huffman;
pub mod lzxd;

pub use compressed::decompress;
pub use crc::crc32;
pub use details::{Details, PropDef, Record, Value, read_details, read_details_keeping};
pub use error::{Cap, Error, Limits};

/// Reads a compressed version 4 Full Details file, as the server publishes it.
pub fn read(file: &[u8], limits: &Limits) -> Result<Details, Error> {
    read_details(&decompress(file, limits)?, limits)
}

/// The ids of the properties a version 4 file has ([MS-OXOAB] 2.9.2; their types may be either
/// kind of string), for [`Record::text`] and the others.
pub mod tag {
    /// The header's: the address list's name.
    pub const OAB_NAME: u16 = 0x6800;
    /// The header's: its sequence number.
    pub const OAB_SEQUENCE: u16 = 0x6801;
    /// The header's: its container's GUID, as text.
    pub const OAB_CONTAINER_GUID: u16 = 0x6802;
    /// The header's: its DN.
    pub const OAB_DN: u16 = 0x6804;
    /// The properties whose values were cut short or left out.
    pub const TRUNCATED_PROPERTIES: u16 = 0x6805;
    /// The entry's DN (`PidTagEmailAddress`).
    pub const EMAIL_ADDRESS: u16 = 0x3003;
    pub const SMTP_ADDRESS: u16 = 0x39FE;
    pub const DISPLAY_NAME: u16 = 0x3001;
    pub const ACCOUNT: u16 = 0x3A00;
    pub const SURNAME: u16 = 0x3A11;
    pub const GIVEN_NAME: u16 = 0x3A06;
    /// Every address the entry receives at, `SMTP:` before the main one.
    pub const PROXY_ADDRESSES: u16 = 0x800F;
    pub const OFFICE_LOCATION: u16 = 0x3A19;
    /// 0 a user, 1 a distribution list, 2 a public folder, 6 a contact outside the
    /// organisation ([MS-OXOABK] 2.2.3.11).
    pub const DISPLAY_TYPE: u16 = 0x3900;
    pub const DISPLAY_TYPE_EX: u16 = 0x3905;
    pub const OBJECT_TYPE: u16 = 0x0FFE;
    pub const BUSINESS_PHONE: u16 = 0x3A08;
    pub const MOBILE_PHONE: u16 = 0x3A1C;
    pub const HOME_PHONE: u16 = 0x3A09;
    pub const TITLE: u16 = 0x3A17;
    pub const COMPANY: u16 = 0x3A16;
    pub const DEPARTMENT: u16 = 0x3A18;
    pub const STREET: u16 = 0x3A29;
    pub const LOCALITY: u16 = 0x3A27;
    pub const STATE: u16 = 0x3A28;
    pub const POSTAL_CODE: u16 = 0x3A2A;
    pub const COUNTRY: u16 = 0x3A26;
    pub const COMMENT: u16 = 0x3004;
    /// The DNs of a distribution list's members.
    pub const MEMBERS: u16 = 0x8009;
}

#[cfg(test)]
mod tests;
