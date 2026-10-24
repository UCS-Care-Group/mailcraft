// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure DAV document helpers. These modules open no sockets, read no files, and hold no secrets.
//! Discovery, sync, account settings and writes are deliberately not wired into this change.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
pub use mailcraft_model::dav::{Collection, Resource};
pub mod document;
pub mod xml;
