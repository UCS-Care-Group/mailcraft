// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Running a job on a thread of the host's: the one way the desktop and the phones start the
//! work the sync driver decided on. What to run and when is the engine's
//! ([`mailcraft_engine::sync_driver`]); this only runs it, with the platform in hand.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use crate::{Host, Platform};

impl Host {
    /// Run `work` on a thread of its own called `name`, with the platform. The answer comes back
    /// through the channel, and `done` is called after it is sent (the window wakes itself in it).
    /// A thread that couldn't start is logged and leaves the sender dropped: the receiver then
    /// reads as disconnected, which callers take for "stopped unexpectedly".
    pub fn run_job<T: Send + 'static>(
        &self,
        name: &str,
        work: impl FnOnce(&dyn Platform) -> T + Send + 'static,
        done: impl FnOnce() + Send + 'static,
    ) -> Receiver<T> {
        let (tx, rx) = channel();
        let platform: Arc<dyn Platform> = self.platform.clone();
        let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
            let _ = tx.send(work(platform.as_ref()));
            done();
        });
        if let Err(e) = spawned {
            log::error!("couldn't start {name}: {e}");
        }
        rx
    }
}
