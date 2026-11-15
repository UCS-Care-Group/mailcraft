// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! **Device limits** (MB3.16): what a device that can't hold a whole mailbox (a phone) keeps of
//! it. A computer has none ([`crate::Mailbox::device_limits`] is `None`) and keeps everything as
//! it always did.
//!
//! - **Listing.** Mail older than an account's mail to keep offline isn't listed from the server
//!   at all, not even its headers ([`crate::Mailbox::listed_since`]): only the window's mail is
//!   here, and older mail is the server's until it is asked for.
//! - **Caps.** At most [`DeviceLimits::storage`] bytes of mail and [`DeviceLimits::messages`]
//!   messages are kept. Past either, the oldest mail goes back to its headers first, then leaves
//!   this device altogether: never the server, and never mail the server hasn't got as it is
//!   here (the Outbox, Drafts, anything not sent or not uploaded yet).

use serde::{Deserialize, Serialize};

use crate::{DownloadInFull, KeepOffline};

/// What a phone keeps unless its owner says otherwise.
pub const PHONE_KEEP: KeepOffline = KeepOffline::Month1;
/// Which messages a phone fetches whole in the background.
pub const PHONE_DOWNLOAD: DownloadInFull = DownloadInFull::UpTo1Mb;
/// How much mail a phone keeps on its disk.
pub const PHONE_STORAGE: u64 = 500 << 20;
/// The least and the most storage an owner may choose.
pub const MIN_STORAGE: u64 = 50 << 20;
pub const MAX_STORAGE: u64 = 64 << 30;
/// The fewest and the most messages a cap may be.
pub const MIN_MESSAGES: u64 = 2_000;
pub const MAX_MESSAGES: u64 = 500_000;
/// The cap of messages when the device names no memory budget.
pub const DEFAULT_MESSAGES: u64 = 50_000;
/// What a message is counted as in memory, beyond its content, when a cap is worked out from a
/// budget: its headers, preview, flags and the indexes that find it.
pub const BYTES_PER_MESSAGE: u64 = 4 << 10;
/// The version of the phone's defaults: a mailbox whose limits say less was set up before them
/// and is moved to them once.
pub const PHONE_DEFAULTS: u32 = 1;

/// A device's limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DeviceLimits {
    /// The most bytes of mail kept on this device.
    pub storage: u64,
    /// The most messages kept on this device.
    pub messages: u64,
    /// The version of the phone's defaults this mailbox was moved to ([`PHONE_DEFAULTS`]).
    pub defaults: u32,
}

impl Default for DeviceLimits {
    fn default() -> Self {
        DeviceLimits { storage: PHONE_STORAGE, messages: DEFAULT_MESSAGES, defaults: 0 }
    }
}

impl DeviceLimits {
    /// A phone's limits, with its cap of messages worked out from what it may hold in memory.
    pub fn phone(memory_budget: Option<u64>) -> DeviceLimits {
        DeviceLimits { storage: PHONE_STORAGE, messages: messages_for(memory_budget), defaults: PHONE_DEFAULTS }
    }

    /// The same, each value within its bounds (a file is hostile).
    pub fn clamped(self) -> DeviceLimits {
        DeviceLimits {
            storage: self.storage.clamp(MIN_STORAGE, MAX_STORAGE),
            messages: self.messages.clamp(MIN_MESSAGES, MAX_MESSAGES),
            defaults: self.defaults,
        }
    }
}

/// The cap of messages for a memory budget: the budget over [`BYTES_PER_MESSAGE`], within bounds.
pub fn messages_for(memory_budget: Option<u64>) -> u64 {
    match memory_budget {
        Some(bytes) => (bytes / BYTES_PER_MESSAGE).clamp(MIN_MESSAGES, MAX_MESSAGES),
        None => DEFAULT_MESSAGES,
    }
}
