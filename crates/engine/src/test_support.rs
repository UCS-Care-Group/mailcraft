// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Small limits for tests of how a mailbox file is opened (the `test-support` feature, which only
//! the engine's own tests and the fuzz targets turn on): what a file may cost in memory, and
//! whether a file is read as a stream whatever its size. Set on a thread, for what that thread
//! opens.

use std::cell::Cell;

/// What a test opens mailbox files with on this thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Bytes of parsed values a file may cost (`load_limit`), in place of the machine's share.
    pub element_bytes: u64,
    /// A one-file mailbox is read as a stream however small it is.
    pub stream: bool,
}

thread_local! {
    static LIMITS: Cell<Option<Limits>> = const { Cell::new(None) };
}

/// Open mailboxes on this thread with `limits` while `f` runs.
pub fn with_limits<T>(limits: Limits, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Limits>);
    impl Drop for Restore {
        fn drop(&mut self) {
            LIMITS.with(|l| l.set(self.0));
        }
    }
    let _restore = Restore(LIMITS.with(|l| l.replace(Some(limits))));
    f()
}

/// The limits a test set on this thread.
pub(crate) fn limits() -> Option<Limits> {
    LIMITS.with(Cell::get)
}
