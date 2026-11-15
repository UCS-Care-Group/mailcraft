// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft vCard: contacts as vCard text (`.vcf` files).
//!
//! [`parse`] reads vCard 2.1, 3.0 and 4.0, one card or many in a text ([`parse_bytes`] in a file,
//! each value in its own `CHARSET`); [`write()`] writes vCard 3.0,
//! the version every address book reads.
//!
//! Input is hostile. Nothing here panics on any text: line lengths and counts are capped, and what
//! makes no sense is skipped.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod key;
#[cfg(test)]
mod key_tests;
mod lines;
mod read;
#[cfg(test)]
mod tests;
mod text;
mod write;

pub use read::{parse, parse_bytes};
pub use write::write;
