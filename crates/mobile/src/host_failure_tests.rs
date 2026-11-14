// SPDX-FileCopyrightText: MailCraft contributors
// SPDX-License-Identifier: MIT OR Apache-2.0
//! A platform whose every method fails (the native code threw): the core answers with stand-ins
//! and never panics.

use std::sync::Arc;

use mailcraft_engine::Session;
use mailcraft_host::Platform as _;

use crate::fake_tests::{block_on, now};
use crate::platform::Native;
use crate::{Core, CoreError, HostError, NetworkKind, NotificationRecord, Platform, RedirectKind, SecretStore, TroubleKind};

struct Throws;

impl SecretStore for Throws {
    fn get(&self, _key: String) -> Result<Option<String>, HostError> {
        Err(HostError::Native)
    }

    fn set(&self, _key: String, _secret: String) -> Result<(), HostError> {
        Err(HostError::Native)
    }

    fn delete(&self, _key: String) -> Result<(), HostError> {
        Err(HostError::Native)
    }

    fn take_trouble(&self) -> Result<Option<TroubleKind>, HostError> {
        Err(HostError::Native)
    }
}

impl Platform for Throws {
    fn secrets(&self) -> Result<Option<Arc<dyn SecretStore>>, HostError> {
        Err(HostError::Native)
    }

    fn redirect(&self) -> Result<RedirectKind, HostError> {
        Err(HostError::Native)
    }

    fn open_sign_in_page(&self, _url: String) -> Result<(), HostError> {
        Err(HostError::Native)
    }

    fn notify(&self, _notification: NotificationRecord) -> Result<(), HostError> {
        Err(HostError::Native)
    }

    fn schedule_reminder(&self, _at: i64, _notification: NotificationRecord) -> Result<(), HostError> {
        Err(HostError::Native)
    }

    fn open_file(&self, _name: String, _bytes: Vec<u8>) -> Result<(), HostError> {
        Err(HostError::Native)
    }

    fn now(&self) -> Result<i64, HostError> {
        Err(HostError::Native)
    }

    fn utc_offset_minutes(&self, _at: i64) -> Result<i32, HostError> {
        Err(HostError::Native)
    }

    fn time_zone_name(&self) -> Result<String, HostError> {
        Err(HostError::Native)
    }

    fn memory_budget(&self) -> Result<Option<u64>, HostError> {
        Err(HostError::Native)
    }

    fn network(&self) -> Result<NetworkKind, HostError> {
        Err(HostError::Native)
    }
}

#[test]
fn a_platform_that_throws_gets_stand_ins_and_no_panic() {
    let native = Native { inner: Arc::new(Throws) };
    assert!(native.secrets().is_none());
    assert_eq!(native.redirect(), mailcraft_host::Redirect::Loopback);
    assert!(native.open_sign_in_page("https://example.org/").is_err());
    assert!(native.now() > 0);
    assert_eq!(native.utc_offset_minutes(0), 0);
    assert_eq!(native.memory_budget(), None);
    assert_eq!(native.network(), mailcraft_host::NetworkState::Unknown);
    let store = crate::platform::NativeSecrets::new(Arc::new(Throws));
    assert!(mailcraft_host::SecretStore::get(&store, "password:a@example.org").is_none());
    assert!(mailcraft_host::SecretStore::set(&store, "password:a@example.org", "x").is_err());
    mailcraft_host::SecretStore::delete(&store, "password:a@example.org");
    // The fallible ones say the store failed, rather than "nothing there" or "removed".
    assert!(mailcraft_host::SecretStore::read(&store, "password:a@example.org").is_err());
    assert!(mailcraft_host::SecretStore::remove(&store, "password:a@example.org").is_err());
    assert_eq!(mailcraft_host::SecretStore::take_trouble(&store), Some(mailcraft_host::StoreTrouble::Unavailable));
}

#[test]
fn a_core_over_a_platform_that_throws_still_starts_and_answers() {
    let core = Core::start(Arc::new(Throws), || Session::at(mailcraft_model::Mailbox::default(), now(), 0));
    // No memory budget given is not a refusal (the host rejects only a budget of 0): it starts.
    assert!(core.is_ok());
    if let Ok(core) = core {
        assert_eq!(core.network(), NetworkKind::Unknown);
        assert!(block_on(core.folders(0)).is_ok());
    }
}

#[test]
fn a_session_that_cannot_be_made_is_an_error_at_start() {
    let r = Core::start(crate::fake_tests::FakePlatform::new(), || -> Session { panic!("no session") });
    assert!(matches!(r, Err(CoreError::Init { .. })));
}
