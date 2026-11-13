// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a review found: a full event queue, the waiters of a queue, shutdown with calls queued,
//! and list ids that are told whole.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use mailcraft_engine::Session;
use mailcraft_model::{Mailbox, Task};

use crate::fake_tests::{FakePlatform, block_on, now, sample_core};
use crate::queue::{MOST_QUEUED, Queue};
use crate::{CoreError, Event};

struct Count(AtomicUsize);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn progress(done: u64) -> Event {
    Event::SyncProgress { account: 1, phase: "fetch".into(), done, total: 0 }
}

#[test]
fn a_flood_of_progress_keeps_the_question_and_the_revision() {
    let core = sample_core();
    let sink = core.sink();
    sink.push(Event::RevChanged { rev: 7 });
    sink.push(Event::PermissionQuestion { account: 1, question: "To Do".into() });
    for n in 0..(MOST_QUEUED as u64 * 2) {
        sink.push(progress(n));
    }
    assert_eq!(sink.queue.len(), MOST_QUEUED);
    assert!(matches!(block_on(core.next_event()), Some(Event::RevChanged { rev: 7 })));
    assert!(matches!(block_on(core.next_event()), Some(Event::PermissionQuestion { account: 1, .. })));
}

#[test]
fn a_newer_revision_replaces_an_older_one_when_the_queue_is_full() {
    let queue = Queue::<Event>::new();
    queue.push(Event::RevChanged { rev: 1 });
    for n in 0..(MOST_QUEUED as u64 - 1) {
        queue.push(Event::SignInNeeded { account: n });
    }
    queue.push(Event::RevChanged { rev: 2 });
    assert_eq!(queue.len(), MOST_QUEUED);
    let events: Vec<Event> = (0..MOST_QUEUED).filter_map(|_| block_on(queue.next())).collect();
    assert!(!events.contains(&Event::RevChanged { rev: 1 }));
    assert!(events.contains(&Event::RevChanged { rev: 2 }));
    // All of the sign-ins stayed.
    assert_eq!(events.iter().filter(|e| matches!(e, Event::SignInNeeded { .. })).count(), MOST_QUEUED - 1);
}

#[test]
fn a_queue_full_of_what_is_acted_on_stays_bounded() {
    let queue = Queue::<Event>::new();
    for n in 0..(MOST_QUEUED as u64 + 5) {
        queue.push(Event::SignInNeeded { account: n });
    }
    assert_eq!(queue.len(), MOST_QUEUED);
}

#[test]
fn every_waiting_future_is_woken_when_the_queue_closes() {
    let queue = Queue::<Event>::new();
    let counts: Vec<Arc<Count>> = (0..40).map(|_| Arc::new(Count(AtomicUsize::new(0)))).collect();
    let mut futures: Vec<_> = counts.iter().map(|_| Box::pin(queue.next())).collect();
    for (f, c) in futures.iter_mut().zip(&counts) {
        let waker = Waker::from(c.clone());
        assert!(matches!(f.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Pending));
    }
    queue.close();
    assert!(counts.iter().all(|c| c.0.load(Ordering::SeqCst) == 1));
    for (f, c) in futures.iter_mut().zip(&counts) {
        let waker = Waker::from(c.clone());
        assert_eq!(f.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Ready(None));
    }
}

#[test]
fn a_dropped_future_leaves_no_waker_behind() {
    let queue = Queue::<Event>::new();
    let count = Arc::new(Count(AtomicUsize::new(0)));
    let waker = Waker::from(count.clone());
    for _ in 0..100 {
        let mut f = Box::pin(queue.next());
        assert!(matches!(f.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Pending));
    }
    let mut live = Box::pin(queue.next());
    assert!(matches!(live.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Pending));
    queue.push(Event::ReminderScheduleChanged);
    // Only the live future is woken: the dropped ones are gone.
    assert_eq!(count.0.load(Ordering::SeqCst), 1);
    assert!(matches!(live.as_mut().poll(&mut Context::from_waker(&waker)), Poll::Ready(Some(Event::ReminderScheduleChanged))));
}

#[test]
fn shutdown_cancels_a_call_that_waits_behind_a_running_one() {
    let core = sample_core();
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let (started_tx, started) = std::sync::mpsc::channel::<()>();
    let holder = core.clone();
    let hold = std::thread::spawn(move || {
        let _ = block_on(holder.call(move |_| {
            let _ = started_tx.send(());
            let _ = gate_rx.recv();
            Ok(())
        }));
    });
    started.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    let mut queued = Box::pin(core.call(move |_| {
        flag.store(true, Ordering::SeqCst);
        Ok(())
    }));
    assert!(matches!(queued.as_mut().poll(&mut Context::from_waker(Waker::noop())), Poll::Pending));
    core.shutdown();
    gate_tx.send(()).unwrap();
    hold.join().unwrap();
    assert_eq!(block_on(queued), Err(CoreError::Cancelled));
    assert!(!ran.load(Ordering::SeqCst));
}

#[test]
fn two_lists_with_a_long_common_start_stay_two() {
    let long = |tail: &str| format!("{}{tail}", "L".repeat(300));
    let core = crate::Core::start(FakePlatform::new(), move || {
        let mut mailbox = Mailbox::default();
        for (list, title) in [(long("a"), "one"), (long("b"), "two"), ("x".repeat(5000), "huge")] {
            mailbox.add_task(Task { title: title.into(), list, ..Default::default() });
        }
        Session::at(mailbox, now(), 0)
    })
    .unwrap();
    let lists = block_on(core.task_lists()).unwrap().lists;
    assert_eq!(lists.len(), 2);
    assert!(lists.iter().all(|l| l.total == 1 && l.list.len() == 301));
    for l in &lists {
        let tasks = block_on(core.tasks(l.account, l.list.clone(), true, 0, 10)).unwrap();
        assert_eq!(tasks.total, 1);
    }
}
