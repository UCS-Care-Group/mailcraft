// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A queue a thread pushes to and a future pulls from, with no runtime under it: the foreign
//! side (Kotlin's coroutines, Swift's concurrency) polls the future UniFFI makes of an `async fn`
//! and is woken here. Used for the reply to one call (`execute`) and for the event stream.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};

/// Most items waiting in a queue; an item past it replaces the oldest (events are about the
/// present, and a UI that fell behind only needs to know it did).
pub(crate) const MOST_QUEUED: usize = 1024;

struct Inner<T> {
    items: VecDeque<T>,
    wakers: Vec<Waker>,
    closed: bool,
}

pub(crate) struct Queue<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Queue<T> {
    pub(crate) fn new() -> Arc<Queue<T>> {
        Arc::new(Queue { inner: Mutex::new(Inner { items: VecDeque::new(), wakers: Vec::new(), closed: false }) })
    }

    fn lock(&self) -> MutexGuard<'_, Inner<T>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Put `item` at the back and wake whoever waits. Nothing is kept once the queue is closed.
    pub(crate) fn push(&self, item: T) {
        let wakers = {
            let mut inner = self.lock();
            if inner.closed {
                return;
            }
            if inner.items.len() >= MOST_QUEUED {
                inner.items.pop_front();
            }
            inner.items.push_back(item);
            std::mem::take(&mut inner.wakers)
        };
        wakers.into_iter().for_each(Waker::wake);
    }

    /// No more items will come: what is waiting is still handed out, then the future says `None`.
    pub(crate) fn close(&self) {
        let wakers = {
            let mut inner = self.lock();
            inner.closed = true;
            std::mem::take(&mut inner.wakers)
        };
        wakers.into_iter().for_each(Waker::wake);
    }

    /// The next item, waiting for one; `None` once the queue is closed and empty.
    pub(crate) fn next(self: &Arc<Self>) -> Next<T> {
        Next { queue: self.clone() }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().items.len()
    }
}

pub(crate) struct Next<T> {
    queue: Arc<Queue<T>>,
}

impl<T> Future for Next<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut inner = self.queue.lock();
        if let Some(item) = inner.items.pop_front() {
            return Poll::Ready(Some(item));
        }
        if inner.closed {
            return Poll::Ready(None);
        }
        if !inner.wakers.iter().any(|w| w.will_wake(cx.waker())) {
            // (A future polled over and over by one task keeps one entry; at most a few tasks wait.)
            if inner.wakers.len() >= 16 {
                inner.wakers.remove(0);
            }
            inner.wakers.push(cx.waker().clone());
        }
        Poll::Pending
    }
}
