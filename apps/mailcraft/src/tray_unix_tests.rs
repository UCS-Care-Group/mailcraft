// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! Startup must not overwrite a watcher's last callback, even before the handle arrives.

use super::*;
use ksni::Tray as _;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

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
