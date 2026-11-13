// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Host services below the window: what a platform (desktop, phone) gives the program, as traits
//! the front ends and the sync driver share. No window, no toolkit: the layer is L5, over the
//! engine and the network crate, and builds for the browser.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod init;
#[cfg(test)]
mod init_tests;
pub mod keys;
#[cfg(test)]
mod keys_tests;
pub mod platform;
pub mod secrets;
#[cfg(test)]
mod secrets_tests;
// Signing in on the provider's page needs the OAuth client, which the browser build has no sockets for.
#[cfg(not(target_arch = "wasm32"))]
pub mod signin;
#[cfg(all(test, not(target_arch = "wasm32")))]
mod signin_tests;

pub use init::{Host, InitError, init, memory_budget};
pub use keys::{legacy_secret_key, microsoft_secret_key, secret_key};
pub use platform::{NetworkState, Notification, NotificationKind, Platform, PlatformError, Redirect, Stream};
pub use secrets::{SecretError, SecretStore, StoreTrouble};
