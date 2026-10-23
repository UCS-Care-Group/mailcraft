// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Waiting for a future on the calling thread: the file dialogs' background thread waits for
//! `rfd`'s answer, and the Windows graphics probe for the adapter list.
//!
//! The thread polls the future, and when it is not ready parks until the future's waker unparks it
//! (`std::task::Wake`). A wake that comes before the park is not lost (`park` returns at once if
//! `unpark` came first) and a spurious wake only polls again. No `unsafe`, no extra threads.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

/// Wakes by unparking the thread that waits.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `future` to completion on this thread.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(out) => return out,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
#[path = "block_on_tests.rs"]
mod block_on_tests;
