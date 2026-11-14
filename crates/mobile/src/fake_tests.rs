// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A fake `Platform` and a tiny executor, for the tests of the binding.

use std::future::Future;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::Thread;

use mailcraft_engine::{Session, sample};
use mailcraft_time::{Date, DateTime};

use crate::{Core, HostError, NetworkKind, NotificationRecord, Platform, RedirectKind, SecretStore, TroubleKind};

/// Wednesday 2026-10-14 16:00 UTC, as the engine's own tests have it.
pub(crate) fn now() -> i64 {
    DateTime::new(Date::new(2026, 10, 14).unwrap(), 16, 0, 0).unwrap().to_timestamp(0)
}

struct Wake1(Thread);

impl Wake for Wake1 {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Run a future to its end on this thread.
pub(crate) fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let waker = Waker::from(Arc::new(Wake1(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park_timeout(std::time::Duration::from_secs(30));
    }
}

#[derive(Default)]
pub(crate) struct FakeSecrets {
    pub(crate) kept: Mutex<Vec<(String, String)>>,
    /// How many of the next writes of the index of names fail.
    pub(crate) fail_index: std::sync::atomic::AtomicUsize,
    /// Deleting removes nothing (the store refuses, and says nothing).
    pub(crate) stuck: std::sync::atomic::AtomicBool,
    /// Reading a sign-in (not the index of names) fails.
    pub(crate) unreadable: std::sync::atomic::AtomicBool,
    /// Reading, or writing, the entry that keeps a notice still to give fails.
    pub(crate) notice_unreadable: std::sync::atomic::AtomicBool,
    pub(crate) notice_unwritable: std::sync::atomic::AtomicBool,
}

impl SecretStore for FakeSecrets {
    fn get(&self, key: String) -> Result<Option<String>, HostError> {
        if key.contains("notice") && self.notice_unreadable.load(Ordering::Relaxed) {
            return Err(HostError::Native);
        }
        if self.unreadable.load(Ordering::Relaxed) && !key.contains("index") {
            return Err(HostError::Native);
        }
        Ok(self.kept.lock().unwrap().iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()))
    }

    fn set(&self, key: String, secret: String) -> Result<(), HostError> {
        if key.contains("notice") && self.notice_unwritable.load(Ordering::Relaxed) {
            return Err(HostError::Unsupported);
        }
        if key.contains("index") && self.fail_index.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_ok() {
            return Err(HostError::Unsupported);
        }
        let mut kept = self.kept.lock().unwrap();
        kept.retain(|(k, _)| *k != key);
        kept.push((key, secret));
        Ok(())
    }

    fn delete(&self, key: String) -> Result<(), HostError> {
        if self.stuck.load(Ordering::Relaxed) {
            return Err(HostError::Unsupported);
        }
        self.kept.lock().unwrap().retain(|(k, _)| *k != key);
        Ok(())
    }

    fn take_trouble(&self) -> Result<Option<TroubleKind>, HostError> {
        Ok(None)
    }
}

pub(crate) struct FakePlatform {
    pub(crate) budget: Option<u64>,
    pub(crate) secrets: Arc<FakeSecrets>,
    pub(crate) notified: AtomicUsize,
    /// Minutes east of UTC the phone says it is in.
    pub(crate) offset: AtomicI32,
    /// The IANA name of the phone's zone, when it says one.
    pub(crate) zone: Mutex<Option<String>>,
    /// The credential store can't be reached.
    pub(crate) no_store: std::sync::atomic::AtomicBool,
    /// What the network is said to be.
    pub(crate) net: Mutex<NetworkKind>,
    /// The pages the core asked to be opened.
    pub(crate) opened: Mutex<Vec<String>>,
    /// The notifications the core asked to be shown (it raises events instead; none is expected).
    pub(crate) shown: Mutex<Vec<NotificationRecord>>,
}

impl FakePlatform {
    pub(crate) fn new() -> Arc<FakePlatform> {
        FakePlatform::with(None)
    }

    fn with(budget: Option<u64>) -> Arc<FakePlatform> {
        Arc::new(FakePlatform {
            budget,
            secrets: Arc::new(FakeSecrets::default()),
            notified: AtomicUsize::new(0),
            offset: AtomicI32::new(0),
            zone: Mutex::new(None),
            no_store: std::sync::atomic::AtomicBool::new(false),
            net: Mutex::new(NetworkKind::Online { metered: true, data_saving: false }),
            opened: Mutex::new(Vec::new()),
            shown: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn with_budget(budget: u64) -> Arc<FakePlatform> {
        FakePlatform::with(Some(budget))
    }
}

impl Platform for FakePlatform {
    fn secrets(&self) -> Result<Option<Arc<dyn SecretStore>>, HostError> {
        if self.no_store.load(Ordering::Relaxed) {
            return Err(HostError::Unsupported);
        }
        Ok(Some(self.secrets.clone()))
    }

    fn redirect(&self) -> Result<RedirectKind, HostError> {
        Ok(RedirectKind::CustomScheme { scheme: "org.example.mailcraft".to_string() })
    }

    fn open_sign_in_page(&self, url: String) -> Result<(), HostError> {
        self.opened.lock().unwrap().push(url);
        Ok(())
    }

    fn notify(&self, notification: NotificationRecord) -> Result<(), HostError> {
        self.shown.lock().unwrap().push(notification);
        self.notified.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn schedule_reminder(&self, _at: i64, _notification: NotificationRecord) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }

    fn open_file(&self, _name: String, _bytes: Vec<u8>) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }

    fn now(&self) -> Result<i64, HostError> {
        Ok(now())
    }

    fn utc_offset_minutes(&self, _at: i64) -> Result<i32, HostError> {
        Ok(self.offset.load(Ordering::Relaxed))
    }

    fn time_zone_name(&self) -> Result<String, HostError> {
        self.zone.lock().unwrap().clone().ok_or(HostError::Unsupported)
    }

    fn memory_budget(&self) -> Result<Option<u64>, HostError> {
        Ok(self.budget)
    }

    fn network(&self) -> Result<NetworkKind, HostError> {
        Ok(*self.net.lock().unwrap())
    }
}

/// A core over the sample mailbox, the clock standing still at [`now`].
pub(crate) fn sample_core() -> Arc<Core> {
    Core::start(FakePlatform::new(), || Session::at(sample::mailbox(now(), 0), now(), 0)).unwrap()
}
