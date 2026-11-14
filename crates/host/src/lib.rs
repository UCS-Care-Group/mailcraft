// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Host services below the window: what a platform (desktop, phone) gives the program, as traits
//! the front ends and the sync driver share. No window, no toolkit: the layer is L5, over the
//! engine and the network crate, and builds for the browser.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod calendar;
pub mod fetch;
pub mod graph;
pub mod init;
#[cfg(test)]
mod init_tests;
pub mod jobs;
pub mod platform;
pub mod secrets;
#[cfg(test)]
mod secrets_tests;
pub mod sync;
#[cfg(test)]
mod sync_tests;
pub mod tasks;
#[cfg(test)]
mod tasks_tests;

pub use init::{Host, InitError, init, memory_budget};
pub use platform::{NetworkState, Notification, NotificationKind, Platform, PlatformError, Redirect, Stream};
pub use secrets::{SecretError, SecretStore, StoreTrouble};
