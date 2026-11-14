// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Startup must not overwrite a watcher's last callback, even before the handle arrives.

use super::*;
use ksni::Tray as _;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

fn guarded_service(stopped: &Arc<AtomicUsize>) -> ShutdownOnDrop<Arc<AtomicUsize>> {
    ShutdownOnDrop {
        handle: stopped.clone(),
        shutdown: |count| {
            count.fetch_add(1, Ordering::Relaxed);
        },
    }
}

#[test]
fn a_handle_delivered_after_an_empty_poll_is_shutdown_when_the_receiver_goes() {
    let stopped = Arc::new(AtomicUsize::new(0));
    let (tx, arriving) = channel();
    let (checked, wait_for_check) = channel();
    let (sent, wait_for_send) = channel();
    let handle = guarded_service(&stopped);
    let sender = std::thread::spawn(move || {
        wait_for_check.recv().unwrap();
        assert!(tx.send(Ok::<_, String>(handle)).is_ok());
        sent.send(()).unwrap();
    });
    assert!(matches!(arriving.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)));
    checked.send(()).unwrap();
    wait_for_send.recv().unwrap();
    assert_eq!(stopped.load(Ordering::Relaxed), 0);
    // Reproduce the old drop race: the send succeeded after try_recv, before receiver drop.
    drop(arriving);
    sender.join().unwrap();
    assert_eq!(stopped.load(Ordering::Relaxed), 1);
}

#[test]
fn a_handle_sent_after_receiver_drop_is_shutdown_even_if_the_send_error_is_ignored() {
    let stopped = Arc::new(AtomicUsize::new(0));
    let (tx, arriving) = channel();
    drop(arriving);
    let _ = tx.send(Ok::<_, String>(guarded_service(&stopped)));
    assert_eq!(stopped.load(Ordering::Relaxed), 1);
}

#[test]
fn an_installed_handle_is_owned_until_drop_and_shutdown_once() {
    let stopped = Arc::new(AtomicUsize::new(0));
    let (tx, arriving) = channel();
    assert!(tx.send(Ok::<_, String>(guarded_service(&stopped))).is_ok());
    let installed = arriving.try_recv().unwrap().unwrap();
    drop(arriving);
    assert_eq!(stopped.load(Ordering::Relaxed), 0);
    drop(installed);
    assert_eq!(stopped.load(Ordering::Relaxed), 1);
}

#[test]
fn watcher_loss_during_spawn_remains_offline_after_success() {
    let signals = Arc::new(Signals { pending: AtomicU8::new(0), online: AtomicBool::new(false), ctx: egui::Context::default() });
    start_item(signals.clone(), |item| {
        assert!(signals.online.load(Ordering::Relaxed));
        // This callback can run on ksni's service thread before spawn returns its handle.
        assert!(item.watcher_offline(ksni::OfflineReason::No));
        Ok(())
    })
    .unwrap();
    assert!(!signals.online.load(Ordering::Relaxed));
}

#[test]
fn failed_startup_clears_the_optimistic_online_state() {
    let signals = Arc::new(Signals { pending: AtomicU8::new(0), online: AtomicBool::new(false), ctx: egui::Context::default() });
    let result = start_item::<()>(signals.clone(), |_| Err("stand-in startup failure".into()));
    assert!(result.is_err());
    assert!(!signals.online.load(Ordering::Relaxed));
}
