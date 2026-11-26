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

mod accounts;
pub mod android;
#[cfg(target_os = "android")]
mod android_jni;
mod attachments;
pub mod blocks;
mod calendar_queries;
mod compose;
mod core;
#[cfg(feature = "e2e")]
mod e2e;
mod error;
mod events;
mod fetch;
mod fetch_api;
mod jobs;
mod keep;
mod limits;
mod links;
mod list;
mod message;
mod people_tasks;
mod pictures;
mod platform;
mod queries;
mod queue;
mod records;
mod reminders;
pub mod settings;
mod sync;
mod sync_api;
pub mod text;

#[cfg(test)]
mod accounts_tests;
#[cfg(test)]
mod attachments_tests;
#[cfg(test)]
mod blocks_tests;
#[cfg(test)]
mod calendar_tests;
#[cfg(test)]
mod compose_share_tests;
#[cfg(test)]
mod compose_tests;
#[cfg(test)]
mod fake_tests;
#[cfg(test)]
mod fetch_review_tests;
#[cfg(test)]
mod fetch_tests;
#[cfg(test)]
mod host_failure_tests;
#[cfg(test)]
mod jobs_tests;
#[cfg(test)]
mod junk_tests;
#[cfg(test)]
mod keep_tests;
#[cfg(test)]
mod limits_tests;
#[cfg(test)]
mod links_tests;
#[cfg(test)]
mod list_tests;
#[cfg(test)]
mod people_tasks_tests;
#[cfg(test)]
mod pictures_tests;
#[cfg(test)]
mod preview_tests;
#[cfg(test)]
mod queue_tests;
#[cfg(test)]
mod quote_choice_tests;
#[cfg(test)]
mod reminders_tests;
#[cfg(test)]
mod review_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod sync_stop_tests;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod zone_tests;

pub use accounts::{Permission, PermissionOutcome, PermissionState, SignInOutcome};
pub use attachments::{AttachmentCheck, AttachmentFile, CalendarImported, OpenVerdict, attachment_check};
pub use compose::{
    ComposeFile, ComposeFormat, ComposeKind, ComposeParagraph, ComposePicture, ComposeSpan, ComposeStart, ComposeView, FromChoice, ListMark,
    RecipientLine, RecipientRecord, SentRecord, SuggestionRecord,
};
pub use core::Core;
pub use error::{CoreError, HostError};
pub use events::{Event, EventSink, StorageNotice};
pub use fetch::{FetchFailure, OpenState, ServerSearchAnswer};
pub use limits::{DeviceStorage, DownloadSettings, KeepWindow};
pub use platform::{NetworkKind, NotificationRecord, NotificationTopic, Platform, RedirectKind, SecretStore, TroubleKind};
pub use records::*;
pub use reminders::{MAX_SCHEDULED, MAX_SKIPPED, ReminderKind, ReminderSchedule, ScheduledReminder};
pub use settings::{AccountSignature, DeviceSettings, KeepOfflineChange, OfflineFreed, ThinResult};
pub use sync::{SyncAnswer, SyncStatus, SyncSummary, UnsupportedAccount};

/// For the end-to-end test (MB3.15): the recipients of the messages in the Graph stand-in's Sent
/// Items whose subject holds `subject`, as the stand-in filed them, so the test sees what the
/// server got and parses nothing itself. Empty while there is none. Outside the `e2e` build
/// (every build that ships) it asks nothing and says so; it is exported there only so that the
/// instrumented test compiles in every build.
#[uniffi::export]
pub fn stand_in_sent_to(subject: String) -> Result<Vec<String>, CoreError> {
    #[cfg(feature = "e2e")]
    return e2e::sent_to(&subject);
    #[cfg(not(feature = "e2e"))]
    {
        let _ = subject;
        Err(CoreError::Failed { reason: "not an end-to-end build".into() })
    }
}
