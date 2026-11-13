// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Host services below the window: what a platform (desktop, phone) gives the program, as traits
//! the front ends and the sync driver share. No window, no toolkit: the layer is L5, over the
//! engine and the network crate, and builds for the browser.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod secrets;
#[cfg(test)]
mod secrets_tests;

pub use secrets::{SecretError, SecretStore, StoreTrouble};
