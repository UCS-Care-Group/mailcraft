//! MailCraft vCard: contacts as vCard text (`.vcf` files).
//!
//! [`parse`] reads vCard 2.1, 3.0 and 4.0, one card or many in a text; [`write()`] writes vCard 3.0,
//! the version every address book reads.
//!
//! Input is hostile. Nothing here panics on any text: line lengths and counts are capped, and what
//! makes no sense is skipped.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod lines;
mod read;
#[cfg(test)]
mod tests;
mod text;
mod write;

pub use read::parse;
pub use write::{write, write_groups};
