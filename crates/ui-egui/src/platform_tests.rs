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
