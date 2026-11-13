// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! What a review found: a full event queue, the waiters of a queue, shutdown with calls queued,
//! and list ids that are told whole.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use mailcraft_engine::{Session, sample};
use mailcraft_model::{Mailbox, Message, Task};

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

#[test]
fn the_last_notification_does_not_take_the_only_revision() {
    let queue = Queue::<Event>::new();
    queue.push(Event::RevChanged { rev: 1 });
    for n in 0..(MOST_QUEUED as u64 - 1) {
        queue.push(Event::SignInNeeded { account: n });
    }
    queue.push(Event::SignInNeeded { account: 999 });
    assert_eq!(queue.len(), MOST_QUEUED);
    let events: Vec<Event> = (0..MOST_QUEUED).filter_map(|_| block_on(queue.next())).collect();
    assert!(events.contains(&Event::RevChanged { rev: 1 }));
    // The oldest sign-in went instead; the newest stayed.
    assert!(!events.contains(&Event::SignInNeeded { account: 0 }));
    assert!(events.contains(&Event::SignInNeeded { account: 999 }));
}

#[test]
fn a_conversation_with_an_older_unread_message_is_unread() {
    let core = crate::Core::start(FakePlatform::new(), || {
        let mut mailbox = sample::mailbox(now(), 0);
        let inbox = mailbox.inbox().unwrap();
        for (date, read) in [(now() - 100, false), (now() - 10, true)] {
            mailbox.add_message(Message { folder: inbox, subject: "Zebra planning".into(), date, read, ..Default::default() }).unwrap();
        }
        let mut session = Session::at(mailbox, now(), 0);
        session.view.conversations = true;
        session
    })
    .unwrap();
    let folder = block_on(core.call(|s| Ok(s.mailbox.inbox()))).unwrap().unwrap();
    let window = block_on(core.messages(folder, "all".into(), 0, 200)).unwrap();
    let row = window.rows.iter().find(|r| r.subject == "Zebra planning").unwrap();
    assert_eq!(row.members, 2);
    assert!(row.unread);
}

#[test]
fn the_device_budget_caps_what_the_session_keeps() {
    let core = crate::Core::start(FakePlatform::with_budget(1 << 20), Session::empty).unwrap();
    assert_eq!(mailcraft_engine::store::body_budget(), 1 << 20);
    assert_eq!(core.memory_budget(), 1 << 20);
}

/// Hold the session thread inside a job until the returned sender is used.
fn hold_session(core: &Arc<crate::Core>) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let (started_tx, started) = std::sync::mpsc::channel::<()>();
    let c = core.clone();
    let hold = std::thread::spawn(move || {
        let _ = block_on(c.call(move |_| {
            let _ = started_tx.send(());
            let _ = gate_rx.recv();
            Ok(())
        }));
    });
    started.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    (gate_tx, hold)
}

type Waiting<'a> = Vec<std::pin::Pin<Box<dyn Future<Output = Result<(), CoreError>> + 'a>>>;

#[test]
fn a_call_past_the_waiting_bound_is_refused_not_waited_for() {
    let core = sample_core();
    let (gate, hold) = hold_session(&core);
    let mut waiting: Waiting<'_> = Vec::new();
    let mut refused = false;
    for _ in 0..(crate::core::MAX_WAITING_JOBS + 2) {
        let mut future = Box::pin(core.call(|_| Ok(())));
        match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Pending => waiting.push(future),
            Poll::Ready(r) => {
                assert!(matches!(r, Err(CoreError::Failed { .. })));
                refused = true;
                break;
            }
        }
    }
    assert!(refused);
    assert_eq!(waiting.len(), crate::core::MAX_WAITING_JOBS);
    drop(waiting);
    gate.send(()).unwrap();
    hold.join().unwrap();
    wait_until_accepted(&core);
}

/// Cancelled jobs keep their queue slots until the session thread receives them: a call is
/// retried, to a deadline, until the queue has drained.
fn wait_until_accepted(core: &crate::Core) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while block_on(core.folders(0)).is_err() {
        assert!(std::time::Instant::now() < deadline, "the session queue did not drain");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn a_language_set_while_the_queue_is_full_reaches_the_session() {
    let core = sample_core();
    let (gate, hold) = hold_session(&core);
    let mut waiting: Waiting<'_> = Vec::new();
    for _ in 0..crate::core::MAX_WAITING_JOBS {
        let mut future = Box::pin(core.call(|_| Ok(())));
        assert!(future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
        waiting.push(future);
    }
    core.set_language("fr".into());
    drop(waiting);
    gate.send(()).unwrap();
    hold.join().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let lang = loop {
        match block_on(core.call(|s| Ok(s.interface_language))) {
            Ok(l) => break l,
            Err(_) => {
                assert!(std::time::Instant::now() < deadline, "the session queue did not drain");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    };
    assert_eq!(lang, mailcraft_i18n::Lang::from_pref("fr", None));
    assert_eq!(core.t("Inbox".into()), mailcraft_i18n::Lang::from_pref("fr", None).t("Inbox"));
}

#[test]
fn caller_strings_waiting_for_the_session_count_against_the_byte_bound() {
    let core = sample_core();
    let (gate, hold) = hold_session(&core);
    // A long search is cut, a list id or a picture reference past its cap is refused: none of
    // them can be held 256 times over for 8 MB each.
    let huge = "x".repeat(8 * 1024 * 1024);
    assert!(matches!(block_on(core.tasks(0, huge.clone(), false, 0, 10)), Err(CoreError::BadRequest { .. })));
    assert!(matches!(block_on(core.picture(1, huge.clone())), Err(CoreError::BadRequest { .. })));
    // What is kept is counted: searches of the longest size fill the byte bound only through it.
    let mut waiting: Vec<std::pin::Pin<Box<dyn Future<Output = Result<crate::records::PeopleWindow, CoreError>> + '_>>> = Vec::new();
    for _ in 0..crate::core::MAX_WAITING_JOBS {
        let mut future = Box::pin(core.people(huge.clone(), 0, 10));
        if future.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending() {
            waiting.push(future);
        }
    }
    assert!(!waiting.is_empty());
    assert!(core.waiting_bytes() >= waiting.len() * 200);
    assert!(core.waiting_bytes() <= waiting.len() * 800);
    drop(waiting);
    gate.send(()).unwrap();
    hold.join().unwrap();
    wait_until_accepted(&core);
}

#[test]
fn parameters_waiting_past_the_byte_bound_are_refused() {
    let core = sample_core();
    let (gate, hold) = hold_session(&core);
    let big = format!("{{\"k\":\"{}\"}}", "x".repeat(7 * 1024 * 1024));
    let mut waiting: Vec<std::pin::Pin<Box<dyn Future<Output = Result<String, CoreError>> + '_>>> = Vec::new();
    let mut refused = false;
    for _ in 0..8 {
        let mut future = Box::pin(core.execute("view.today".into(), big.clone()));
        match future.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Pending => waiting.push(future),
            Poll::Ready(r) => {
                assert!(matches!(r, Err(CoreError::Failed { .. })));
                refused = true;
                break;
            }
        }
    }
    assert!(refused);
    assert!(!waiting.is_empty());
    drop(waiting);
    gate.send(()).unwrap();
    hold.join().unwrap();
    // What was refused and what was dropped gave their share back.
    assert!(exec_today(&core));
}

fn exec_today(core: &crate::Core) -> bool {
    block_on(core.execute("view.today".into(), String::new())).is_ok()
}

#[test]
fn the_enforced_budget_is_the_lowest_of_the_cores_alive() {
    let mut budgets = crate::core::Budgets::default();
    assert_eq!(budgets.add(1, 8 << 20), Some(8 << 20));
    assert_eq!(budgets.add(2, 2 << 20), Some(2 << 20));
    // A later core with more does not raise it.
    assert_eq!(budgets.add(3, 16 << 20), Some(2 << 20));
    // The one with the least ends: the next lowest stands.
    assert_eq!(budgets.remove(2), Some(8 << 20));
    assert_eq!(budgets.remove(1), Some(16 << 20));
    assert_eq!(budgets.remove(3), None);
}

#[test]
fn a_category_heading_says_it_is_one() {
    let core = crate::Core::start(FakePlatform::new(), || {
        let mut mailbox = sample::mailbox(now(), 0);
        let inbox = mailbox.inbox().unwrap();
        mailbox
            .add_message(Message { folder: inbox, subject: "Tagged".into(), date: now() - 5, categories: vec!["Today".into()], ..Default::default() })
            .unwrap();
        let mut session = Session::at(mailbox, now(), 0);
        session.view.sort = mailcraft_engine::SortBy::Categories;
        session
    })
    .unwrap();
    let folder = block_on(core.call(|s| Ok(s.mailbox.inbox()))).unwrap().unwrap();
    let window = block_on(core.messages(folder, "all".into(), 0, 200)).unwrap();
    let row = window.rows.iter().find(|r| r.subject == "Tagged").unwrap();
    assert_eq!(row.group.as_deref(), Some("Today"));
    assert!(row.group_is_category);
}

#[test]
fn a_selected_message_that_fails_the_filter_is_not_listed() {
    let core = sample_core();
    let folder = block_on(core.call(|s| Ok(s.mailbox.inbox()))).unwrap().unwrap();
    let unread = block_on(core.messages(folder, "unread".into(), 0, 200)).unwrap();
    let id = unread.rows[0].id;
    // Select it in the unread list, then read it: the engine keeps it in that list while selected.
    block_on(core.call(move |s| {
        s.view.folder = folder;
        s.view.filter = mailcraft_engine::Filter::Unread;
        s.sel.messages = vec![id];
        let rows = mailcraft_engine::list::rows(s);
        s.keep_selected(&rows);
        s.run("mail.markRead", &serde_json::json!({"ids": [id]}))?;
        assert!(mailcraft_engine::list::rows(s).iter().any(|r| r.id == id));
        Ok(())
    }))
    .unwrap();
    let after = block_on(core.messages(folder, "unread".into(), 0, 200)).unwrap();
    assert!(!after.rows.iter().any(|r| r.id == id));
    // The session's own selection and list are as they were.
    assert!(block_on(core.call(move |s| Ok(s.kept().is_some_and(|k| k.contains(&id))))).unwrap());
}
