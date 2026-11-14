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

/// Most items waiting in a queue. An item past it makes room by shedding what costs least (see
/// [`Shed`]): events are about the present, and a UI that fell behind only needs to know it did.
pub(crate) const MOST_QUEUED: usize = 1024;

/// What a full queue may do with an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Weight {
    /// A progress report: the next one says it all, so the oldest is shed first.
    Progress,
    /// A state that a later item of the same kind replaces (a revision): only the newest matters.
    Replaced,
    /// Something to show that, once past, is little loss (a notification, a change of the
    /// reminders' schedule, which is asked for again): shed before anything the app must answer.
    Notice,
    /// Something the app has to act on (a sign-in, a question): shed last.
    Actionable,
}

/// How an item weighs when the queue is full.
pub(crate) trait Shed {
    fn weight(&self) -> Weight;
}

/// A reply to one call: the queue holds one, never reaches its bound.
impl<T, E> Shed for Result<T, E> {
    fn weight(&self) -> Weight {
        Weight::Actionable
    }
}

struct Inner<T> {
    items: VecDeque<T>,
    /// The waker of each future that waits, by the number of the future: one entry each, gone
    /// when the future is ready or dropped.
    wakers: Vec<(u64, Waker)>,
    next_id: u64,
    closed: bool,
}

pub(crate) struct Queue<T> {
    inner: Mutex<Inner<T>>,
}

impl<T: Shed> Queue<T> {
    pub(crate) fn new() -> Arc<Queue<T>> {
        Arc::new(Queue { inner: Mutex::new(Inner { items: VecDeque::new(), wakers: Vec::new(), next_id: 0, closed: false }) })
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
            if inner.items.len() >= MOST_QUEUED && !make_room(&mut inner.items, &item) {
                return;
            }
            inner.items.push_back(item);
            std::mem::take(&mut inner.wakers)
        };
        wakers.into_iter().for_each(|(_, w)| w.wake());
    }

    /// No more items will come: what is waiting is still handed out, then the future says `None`.
    pub(crate) fn close(&self) {
        let wakers = {
            let mut inner = self.lock();
            inner.closed = true;
            std::mem::take(&mut inner.wakers)
        };
        wakers.into_iter().for_each(|(_, w)| w.wake());
    }

    /// The next item, waiting for one; `None` once the queue is closed and empty.
    pub(crate) fn next(self: &Arc<Self>) -> Next<T> {
        let id = {
            let mut inner = self.lock();
            inner.next_id = inner.next_id.wrapping_add(1);
            inner.next_id
        };
        Next { queue: self.clone(), id }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().items.len()
    }
}

/// Make room in a full queue for `incoming`; false when `incoming` itself is the one to shed.
fn make_room<T: Shed>(items: &mut VecDeque<T>, incoming: &T) -> bool {
    let first = |items: &VecDeque<T>, w: Weight| items.iter().position(|i| i.weight() == w);
    // The oldest progress report goes first; a progress report with none before it is the one shed.
    if let Some(at) = first(items, Weight::Progress) {
        items.remove(at);
        return true;
    }
    if incoming.weight() == Weight::Progress {
        return false;
    }
    // A replaced state: the oldest goes when a newer one stands (the incoming one, or another).
    let replaced = items.iter().filter(|i| i.weight() == Weight::Replaced).count();
    if let Some(at) = first(items, Weight::Replaced)
        && (incoming.weight() == Weight::Replaced || replaced > 1)
    {
        items.remove(at);
        return true;
    }
    // Notices go before what must be answered: a flood of notifications never costs a sign-in.
    if let Some(at) = first(items, Weight::Notice) {
        items.remove(at);
        return true;
    }
    // A notice never costs what must be acted on: with no notice queued, it is the one shed.
    if incoming.weight() == Weight::Notice {
        return false;
    }
    // Only what must be acted on is left, more of it than anybody can have meant to leave
    // unread: the memory stays bounded and the oldest goes.
    // The one revision stands for every cached query: it is kept, and the oldest other event goes.
    if let Some(at) = items.iter().position(|i| i.weight() != Weight::Replaced) {
        items.remove(at);
        return true;
    }
    items.pop_front().is_some()
}

pub(crate) struct Next<T> {
    queue: Arc<Queue<T>>,
    id: u64,
}

impl<T> Drop for Next<T> {
    fn drop(&mut self) {
        let mut inner = self.queue.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.wakers.retain(|(id, _)| *id != self.id);
    }
}

impl<T> Future for Next<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let id = self.id;
        let mut inner = self.queue.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(item) = inner.items.pop_front() {
            inner.wakers.retain(|(i, _)| *i != id);
            return Poll::Ready(Some(item));
        }
        if inner.closed {
            inner.wakers.retain(|(i, _)| *i != id);
            return Poll::Ready(None);
        }
        // One entry for each waiting future, the latest waker it was polled with.
        match inner.wakers.iter_mut().find(|(i, _)| *i == id) {
            Some((_, w)) => w.clone_from(cx.waker()),
            None => inner.wakers.push((id, cx.waker().clone())),
        }
        Poll::Pending
    }
}
