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
}

impl SecretStore for FakeSecrets {
    fn get(&self, key: String) -> Result<Option<String>, HostError> {
        Ok(self.kept.lock().unwrap().iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone()))
    }

    fn set(&self, key: String, secret: String) -> Result<(), HostError> {
        self.kept.lock().unwrap().push((key, secret));
        Ok(())
    }

    fn delete(&self, key: String) -> Result<(), HostError> {
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
}

impl FakePlatform {
    pub(crate) fn new() -> Arc<FakePlatform> {
        Arc::new(FakePlatform { budget: None, secrets: Arc::new(FakeSecrets::default()), notified: AtomicUsize::new(0), offset: AtomicI32::new(0) })
    }

    pub(crate) fn with_budget(budget: u64) -> Arc<FakePlatform> {
        Arc::new(FakePlatform {
            budget: Some(budget),
            secrets: Arc::new(FakeSecrets::default()),
            notified: AtomicUsize::new(0),
            offset: AtomicI32::new(0),
        })
    }
}

impl Platform for FakePlatform {
    fn secrets(&self) -> Result<Option<Arc<dyn SecretStore>>, HostError> {
        Ok(Some(self.secrets.clone()))
    }

    fn redirect(&self) -> Result<RedirectKind, HostError> {
        Ok(RedirectKind::CustomScheme { scheme: "org.example.mailcraft".to_string() })
    }

    fn open_sign_in_page(&self, _url: String) -> Result<(), HostError> {
        Ok(())
    }

    fn notify(&self, _notification: NotificationRecord) -> Result<(), HostError> {
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

    fn memory_budget(&self) -> Result<Option<u64>, HostError> {
        Ok(self.budget)
    }

    fn network(&self) -> Result<NetworkKind, HostError> {
        Ok(NetworkKind::Online { metered: true, data_saving: false })
    }
}

/// A core over the sample mailbox, the clock standing still at [`now`].
pub(crate) fn sample_core() -> Arc<Core> {
    Core::start(FakePlatform::new(), || Session::at(sample::mailbox(now(), 0), now(), 0)).unwrap()
}
