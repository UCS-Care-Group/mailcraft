// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! MailCraft's binding for the phone apps (ADR 0023): the Rust core as one [`Core`] object that
//! Kotlin (Android) and Swift (iOS) hold, exported through UniFFI. No UI toolkit, no web view:
//! the apps draw mail from the display blocks of [`blocks`].
//!
//! - [`Core::execute`] runs a command by id through `Session::run`, the path of the CLI, the
//!   control channel and MCP, so the command registry stays the one way to change anything.
//! - The typed queries ([`queries`], [`message`]) read windows of the mailbox for the screens,
//!   each stamped with the revision it was read at.
//! - [`Core::next_event`] is the stream the app pulls: revision changes, sync progress, sign-in
//!   and permission questions, notifications, the reminder schedule.
//! - [`Platform`] is what the native side gives the core, as a UniFFI foreign trait.
//!
//! Never crash: every export returns a `Result` of a type that has `From<UnexpectedUniFFICallbackError>`,
//! a panic inside a job is caught and is an error, and everything a message, a server or the app
//! passes in is bounded.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

uniffi::setup_scaffolding!();

pub mod android;
#[cfg(target_os = "android")]
mod android_jni;
pub mod blocks;
mod core;
mod error;
mod events;
mod message;
mod platform;
mod queries;
mod queue;
mod records;
pub mod text;

#[cfg(test)]
mod fake_tests;
#[cfg(test)]
mod host_failure_tests;
#[cfg(test)]
mod junk_tests;
#[cfg(test)]
mod preview_tests;
#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod zone_tests;

pub use core::Core;
pub use error::{CoreError, HostError};
pub use events::{Event, EventSink};
pub use platform::{NetworkKind, NotificationRecord, NotificationTopic, Platform, RedirectKind, SecretStore, TroubleKind};
pub use records::*;
