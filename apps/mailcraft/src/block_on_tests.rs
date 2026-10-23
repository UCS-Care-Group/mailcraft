// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! `block_on`: ready futures, futures woken at once, from another thread, and many times over.

use super::*;
use std::pin::Pin;
use std::sync::Mutex;

#[test]
fn a_ready_future_returns_its_value() {
    assert_eq!(block_on(async { 7 }), 7);
}

/// Pending `n` times, waking itself each time, then ready.
struct Yields(u32);

impl Future for Yields {
    type Output = &'static str;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.0 == 0 {
            return Poll::Ready("done");
        }
        self.0 -= 1;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[test]
fn a_future_that_wakes_itself_is_polled_again() {
    assert_eq!(block_on(Yields(1000)), "done");
}

/// Ready once another thread has set the value and woken the waker.
struct Slot {
    value: Mutex<(Option<u32>, Option<Waker>)>,
}

struct Wait(Arc<Slot>);

impl Future for Wait {
    type Output = u32;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
        let mut g = match self.0.value.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        match g.0 {
            Some(v) => Poll::Ready(v),
            None => {
                g.1 = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}

#[test]
fn a_wake_from_another_thread_ends_the_wait() {
    let slot = Arc::new(Slot { value: Mutex::new((None, None)) });
    let other = slot.clone();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mut g = other.value.lock().unwrap();
        g.0 = Some(42);
        if let Some(w) = g.1.take() {
            w.wake();
        }
    });
    assert_eq!(block_on(Wait(slot)), 42);
    t.join().unwrap();
}
