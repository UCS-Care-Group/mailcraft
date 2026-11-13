// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mailcraft_host::{Notification, NotificationKind, Platform, PlatformError};

use crate::Services;
use crate::platform::DesktopPlatform;

fn note(kind: NotificationKind) -> Notification {
    Notification { key: 1, kind, title: "t".into(), body: "b".into() }
}

/// The platform for `s`, and how many times it woke the window.
fn platform_of(s: &Services) -> (Arc<DesktopPlatform>, Arc<AtomicUsize>) {
    let woke = Arc::new(AtomicUsize::new(0));
    let w = woke.clone();
    let p = s.platform_calls.platform(
        s.secrets.clone(),
        Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }),
    );
    (Arc::new(p), woke)
}

/// Run `ask` on a thread of its own (a worker of the host) while this thread, the window's,
/// answers it.
fn from_a_worker<T: Send + 'static>(s: &Services, ask: impl FnOnce() -> T + Send + 'static) -> T {
    let worker = std::thread::spawn(ask);
    while !worker.is_finished() {
        s.serve_platform();
        std::thread::yield_now();
    }
    worker.join().ok().unwrap()
}

#[test]
fn the_desktop_platform_can_be_held_by_the_host_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DesktopPlatform>();
}

#[test]
fn services_with_nothing_attached_say_so() {
    let s = Services::default();
    let (p, _) = platform_of(&s);
    assert!(p.secrets().is_none());
    let q = p.clone();
    assert_eq!(from_a_worker(&s, move || q.open_sign_in_page("https://login.example.org/")), Err(PlatformError::Unsupported));
    let q = p.clone();
    assert_eq!(from_a_worker(&s, move || q.open_file("a.txt", b"x")), Err(PlatformError::Unsupported));
    let q = p.clone();
    assert_eq!(from_a_worker(&s, move || q.notify(&note(NotificationKind::Reminder))), Err(PlatformError::Unsupported));
}

#[test]
fn a_worker_asks_the_window_and_the_window_does_what_it_always_did() {
    let opened = Rc::new(RefCell::new(Vec::new()));
    let seen = opened.clone();
    let s = Services {
        open_page: Some(Box::new(move |url| {
            seen.borrow_mut().push(url.to_string());
            url.starts_with("https://")
        })),
        open_file: Some(Box::new(|_, _| Err("nothing opens it".to_string()))),
        ..Default::default()
    };
    let (p, woke) = platform_of(&s);
    let q = p.clone();
    assert_eq!(from_a_worker(&s, move || q.open_sign_in_page("https://login.example.org/")), Ok(()));
    let q = p.clone();
    assert!(matches!(from_a_worker(&s, move || q.open_sign_in_page("nope")), Err(PlatformError::Failed(_))));
    assert_eq!(opened.borrow().len(), 2);
    assert_eq!(woke.load(Ordering::SeqCst), 2);
    let q = p.clone();
    assert_eq!(from_a_worker(&s, move || q.open_file("a.txt", b"x")), Err(PlatformError::Failed("the file could not be opened")));
    // Mail alerts are the window's own.
    assert_eq!(p.notify(&note(NotificationKind::Mail)), Err(PlatformError::Unsupported));
}

#[test]
fn the_window_asking_itself_is_refused_at_once() {
    let s = Services::default();
    let (p, _) = platform_of(&s);
    assert_eq!(p.open_sign_in_page("https://login.example.org/"), Err(PlatformError::Failed("the window cannot ask itself")));
}

#[test]
fn a_window_that_does_not_answer_times_the_worker_out() {
    let s = Services::default();
    let p = Arc::new(s.platform_calls.platform(s.secrets.clone(), Arc::new(|| {})).patient(std::time::Duration::from_millis(50)));
    let worker = std::thread::spawn(move || p.open_file("a.txt", b"x"));
    // Nobody serves the window's side.
    assert_eq!(worker.join().ok().unwrap(), Err(PlatformError::Failed("the window did not answer")));
}

#[test]
fn a_page_that_waits_does_not_hold_up_the_window_s_frame() {
    use std::sync::mpsc::channel;
    let (release, released) = channel::<()>();
    let released = std::sync::Mutex::new(released);
    let s = Services {
        // Would stop the frame if it ran on the window's thread.
        open_page: Some(Box::new(|_| false)),
        open_page_waiting: Some(Arc::new(move |_| {
            let _ = released.lock().map(|r| r.recv_timeout(std::time::Duration::from_secs(5)));
            true
        })),
        ..Default::default()
    };
    let (p, _) = platform_of(&s);
    let worker = std::thread::spawn(move || p.open_sign_in_page("https://login.example.org/"));
    let started = std::time::Instant::now();
    // The frame keeps being served while the page waits; it never blocks.
    while started.elapsed() < std::time::Duration::from_millis(200) {
        let frame = std::time::Instant::now();
        s.serve_platform();
        assert!(frame.elapsed() < std::time::Duration::from_millis(100), "the frame was held up");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!worker.is_finished());
    let _ = release.send(());
    while !worker.is_finished() {
        s.serve_platform();
        std::thread::yield_now();
    }
    assert_eq!(worker.join().ok().unwrap(), Ok(()));
}

#[test]
fn a_page_that_timed_out_before_it_was_served_is_never_opened() {
    let opened = Rc::new(RefCell::new(0));
    let seen = opened.clone();
    let s = Services {
        open_page: Some(Box::new(move |_| {
            *seen.borrow_mut() += 1;
            true
        })),
        ..Default::default()
    };
    let p = Arc::new(s.platform_calls.platform(s.secrets.clone(), Arc::new(|| {})).patient(std::time::Duration::from_millis(50)));
    let worker = std::thread::spawn(move || p.open_sign_in_page("https://login.example.org/"));
    // Nobody serves until the worker has given up.
    assert_eq!(worker.join().ok().unwrap(), Err(PlatformError::Failed("the window did not answer")));
    s.serve_platform();
    assert_eq!(*opened.borrow(), 0);
}

#[test]
fn a_page_that_began_before_the_timeout_reports_what_happened() {
    use std::sync::mpsc::channel;
    let (release, released) = channel::<()>();
    let released = std::sync::Mutex::new(released);
    let s = Services {
        open_page_waiting: Some(Arc::new(move |_| {
            let _ = released.lock().map(|r| r.recv_timeout(std::time::Duration::from_secs(5)));
            true
        })),
        ..Default::default()
    };
    let p = Arc::new(s.platform_calls.platform(s.secrets.clone(), Arc::new(|| {})).patient(std::time::Duration::from_millis(300)));
    let worker = std::thread::spawn(move || p.open_sign_in_page("https://login.example.org/"));
    // Served at once, but the page takes longer than the worker's first wait (300 ms): it is
    // released inside the second.
    let started = std::time::Instant::now();
    let mut sent = false;
    while !worker.is_finished() {
        s.serve_platform();
        std::thread::sleep(std::time::Duration::from_millis(10));
        if !sent && started.elapsed() > std::time::Duration::from_millis(400) {
            sent = release.send(()).is_ok();
        }
    }
    assert_eq!(worker.join().ok().unwrap(), Ok(()));
}

#[test]
fn a_full_batch_asks_for_another_frame() {
    let s = Services::default();
    let (p, woke) = platform_of(&s);
    let workers: Vec<_> = (0..20)
        .map(|_| {
            let q = p.clone();
            std::thread::spawn(move || q.open_file("a.txt", b"x"))
        })
        .collect();
    // Every call is queued once its worker has woken the window.
    while woke.load(Ordering::SeqCst) < 20 {
        std::thread::yield_now();
    }
    assert!(s.serve_platform(), "a full batch leaves more waiting");
    assert!(!s.serve_platform(), "the rest fits in one more frame");
    for w in workers {
        assert_eq!(w.join().ok().unwrap(), Err(PlatformError::Unsupported));
    }
}
