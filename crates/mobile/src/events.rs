// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! The event stream the app pulls with `Core::next_event` (it becomes a Kotlin `Flow` or a Swift
//! `AsyncStream` on the native side). Events say that something changed or is asked; what changed
//! is read again with the typed queries.

use std::sync::Arc;

use crate::NotificationRecord;
use crate::queue::{Queue, Shed, Weight};

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Event {
    /// The mailbox changed: `rev` is the revision every query names its answer by, so a window
    /// read at an older one is stale.
    RevChanged { rev: u64 },
    /// A synchronisation moved on. `total` is 0 when it isn't known.
    SyncProgress { account: u64, phase: String, done: u64, total: u64 },
    /// A Send/Receive round is over (`finished` is false when it was called off for its time
    /// running out): `new_mail` is what it brought that notifications count.
    SyncFinished { new_mail: u64, finished: bool },
    /// The account's sign-in has to be done again: the app opens the provider's page.
    SignInNeeded { account: u64 },
    /// The owner is asked to allow something before it is done (a permission for To Do, a shared
    /// mailbox, sending as one): `question` is the program's name for it. Never answered for them.
    PermissionQuestion { account: u64, question: String },
    /// Raise this notification now.
    Notification { notification: NotificationRecord },
    /// The reminders to schedule changed: ask for the schedule again.
    ReminderScheduleChanged,
    /// The mailbox couldn't be saved (once per run of failures; it is tried again): `reason` is
    /// for the log and for support, the app says it in its own words.
    SaveFailed { reason: String },
    /// Something about the mailbox file the owner is told when the app starts.
    StorageNotice { notice: StorageNotice, detail: String },
}

/// What the owner is told about the mailbox file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StorageNotice {
    /// Parts of the file were damaged and left out (`detail` says which); the mail missing comes
    /// again at the next Send/Receive.
    PartlyRead,
    /// The file couldn't be read: it was kept aside (`detail` is its new name) and a new, empty
    /// mailbox started.
    SetAside,
    /// The file couldn't be read and couldn't be kept aside: nothing is saved this run.
    NotKept,
}

impl Shed for Event {
    /// What a full queue keeps: questions and sign-ins go last; notifications and the reminders'
    /// change before them (the schedule is asked for again, a notification is a moment's);
    /// a revision is replaced by a newer one; progress by the next.
    fn weight(&self) -> Weight {
        match self {
            Event::SyncProgress { .. } => Weight::Progress,
            Event::RevChanged { .. } => Weight::Replaced,
            Event::Notification { .. } | Event::ReminderScheduleChanged | Event::SyncFinished { .. } => Weight::Notice,
            Event::SignInNeeded { .. } | Event::PermissionQuestion { .. } | Event::StorageNotice { .. } | Event::SaveFailed { .. } => {
                Weight::Actionable
            }
        }
    }
}

/// Where the Rust side puts events: the session thread for `RevChanged`, and the sync driver
/// (through `Core::sink`) for the rest. Cheap to clone and to hold.
#[derive(Clone)]
pub struct EventSink {
    pub(crate) queue: Arc<Queue<Event>>,
}

impl EventSink {
    pub fn push(&self, event: Event) {
        self.queue.push(event);
    }
}
